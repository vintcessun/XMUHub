#!/usr/bin/env pwsh
<#
.SYNOPSIS
    部署 XMUHub：上传 `run` 与 `web/` 到服务器、更新 nginx 中转路由并重启服务。

.DESCRIPTION
    前置：先运行 scripts/build-alinux3.ps1 生成 `run`。密钥读自 .secrets/*.env（不入库）：
      .secrets/github.env      GH_STORE_USER, GH_STORE_TOKEN
      .secrets/cloudflare.env  CF_ACCOUNT_ID, CF_API_TOKEN
      .secrets/upload.env      UPLOAD_TICKET_SECRET, UPLOAD_WORKER_NAME

    步骤：
      1. 以 .new 临时名上传 run、web 包、环境文件（上传期间服务照常运行）；
      2. 安装/更新 systemd 服务与 nginx 中转路由（仅在内容变化时）；
      3. 停服务 → 原子替换（旧版本保留为 run.bak / web.bak）→ 启动 → 健康检查。
    上传默认经本服务器流式中转到 GitHub（不落盘）；-OnlyWorker 部署 Cloudflare Worker，
    给它绑定 CF 自定义域名后用 -UploadWorkerUrl 切换过去即可让服务器不再承担上传流量。
    `data/`（数据库）永远不会被覆盖或删除。

.EXAMPLE
    pwsh scripts/deploy.ps1
    pwsh scripts/deploy.ps1 -OnlyWorker          # 只更新 Cloudflare Worker（需配合自定义域名才在国内可用）
    pwsh scripts/deploy.ps1 -WebOnly             # 只更新网页文件（不需要重新编译）
#>
param(
    [string]$User = "root",
    [string]$HostName = "vintces.icu",
    [int]$Port = 22,
    [string]$RemoteBase = "/root/xmuhub",
    [string]$Service = "xmuhub.service",
    [int]$AppPort = 8089,
    [string]$PublicUrl = "https://xmu.vintces.icu",
    [string]$IdentityFile = "",
    # BT panel per-site include directory (server-level directives for this vhost).
    [string]$NginxExtDir = "/www/server/panel/vhost/nginx/extension/xmu.vintces.icu",
    # Empty = uploads stream through this server (workers.dev is blocked in mainland China).
    # Set to a Worker URL on a Cloudflare custom domain to take the server out of the path.
    [string]$UploadWorkerUrl = "",
    [switch]$OnlyWorker,
    [switch]$WebOnly,
    # Issue a new admin (L4) token on the server and print it; nothing is deployed.
    [switch]$NewAdminToken,
    [string]$TokenLabel = "管理员"
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = (Resolve-Path (Join-Path $ScriptDir "..")).Path
$Remote = "$User@$HostName"

function Read-EnvFile([string]$Path) {
    if (-not (Test-Path $Path)) { throw "缺少密钥文件：$Path" }
    $h = @{}
    foreach ($line in Get-Content $Path) {
        if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)\s*$') { $h[$Matches[1]] = $Matches[2] }
    }
    return $h
}

$gh = Read-EnvFile (Join-Path $Root ".secrets/github.env")
$cf = Read-EnvFile (Join-Path $Root ".secrets/cloudflare.env")
$up = Read-EnvFile (Join-Path $Root ".secrets/upload.env")
$WorkerName = if ($up.UPLOAD_WORKER_NAME) { $up.UPLOAD_WORKER_NAME } else { "xmuhub-upload" }

# ================================================================ 1. Cloudflare Worker
function Deploy-Worker {
    Write-Host "===== 部署上传中转 Worker：$WorkerName =====" -ForegroundColor Cyan
    $api = "https://api.cloudflare.com/client/v4/accounts/$($cf.CF_ACCOUNT_ID)/workers"
    $headers = @{ Authorization = "Bearer $($cf.CF_API_TOKEN)" }

    $meta = @{
        main_module        = "upload.js"
        compatibility_date = "2025-09-01"
        bindings           = @(
            @{ type = "secret_text"; name = "GH_TOKEN"; text = $gh.GH_STORE_TOKEN },
            @{ type = "secret_text"; name = "TICKET_SECRET"; text = $up.UPLOAD_TICKET_SECRET },
            @{ type = "plain_text"; name = "GH_OWNER"; text = $gh.GH_STORE_USER },
            @{ type = "plain_text"; name = "ALLOWED_ORIGINS"; text = "$PublicUrl,http://127.0.0.1:18089" }
        )
    } | ConvertTo-Json -Depth 5 -Compress
    $tmp = New-Item -ItemType Directory -Force (Join-Path ([System.IO.Path]::GetTempPath()) "xmuhub-worker")
    $metaPath = Join-Path $tmp "metadata.json"
    [System.IO.File]::WriteAllText($metaPath, $meta, (New-Object System.Text.UTF8Encoding($false)))
    $js = Join-Path $Root "worker/upload.js"

    try {
        # curl.exe handles per-part content types; the module part must be application/javascript+module.
        $out = & curl.exe -sS -X PUT "$api/scripts/$WorkerName" -H "Authorization: Bearer $($cf.CF_API_TOKEN)" `
            -F "metadata=@$metaPath;type=application/json" `
            -F "upload.js=@$js;type=application/javascript+module"
        $res = $out | ConvertFrom-Json
        if (-not $res.success) { throw "Worker 上传失败：$($res.errors | ConvertTo-Json -Compress)" }
    }
    finally {
        Remove-Item $metaPath -Force -ErrorAction SilentlyContinue
    }

    $sub = Invoke-RestMethod -Method Post -Uri "$api/scripts/$WorkerName/subdomain" -Headers $headers `
        -ContentType "application/json" -Body '{"enabled":true,"previews_enabled":false}'
    if (-not $sub.success) { throw "启用 workers.dev 失败" }
    $account = (Invoke-RestMethod -Uri "$api/subdomain" -Headers $headers).result.subdomain
    $script:WorkerUrl = "https://$WorkerName.$account.workers.dev"
    Write-Host "Worker 地址：$script:WorkerUrl" -ForegroundColor Green
}

# The Worker is only useful behind a Cloudflare custom domain; deploy it when asked.
if ($OnlyWorker) {
    Deploy-Worker
    Write-Host "===== 完成（仅 Worker）=====" -ForegroundColor Green
    return
}

# ================================================================ 2. 服务器
foreach ($exe in @("ssh", "scp", "tar")) {
    if (-not (Get-Command $exe -ErrorAction SilentlyContinue)) { throw "未找到 $exe" }
}
$LocalRun = Join-Path $Root "run"
if (-not $WebOnly -and -not (Test-Path $LocalRun)) { throw "未找到 run，请先运行 scripts/build-alinux3.ps1" }

$commonOpts = @("-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10")
$sshOpts = $commonOpts + @("-p", "$Port")
$scpOpts = $commonOpts + @("-P", "$Port")
if ($IdentityFile -ne "") {
    $id = (Resolve-Path $IdentityFile).Path
    $sshOpts += @("-i", $id); $scpOpts += @("-i", $id)
}
function Invoke-Remote([string]$Script, [string]$What) {
    # Normalise to LF so bash on the server doesn't see stray \r.
    & ssh @sshOpts $Remote ($Script -replace "`r`n", "`n")
    if ($LASTEXITCODE -ne 0) { throw "远端步骤失败（$What），exit=$LASTEXITCODE" }
}

if ($NewAdminToken) {
    # redb holds an exclusive lock while the service runs, so stop it for the moment it takes.
    Invoke-Remote @"
set -e
cd '$RemoteBase'
systemctl stop $Service
set -a; . ./xmuhub.env; set +a
./run token --level 4 --label '$TokenLabel' 2>/dev/null | tail -n 2 || true
systemctl start $Service
"@ "签发管理员令牌"
    Write-Host "请妥善保存上面的令牌，在网站「我的」页面粘贴即可成为管理员。" -ForegroundColor Green
    return
}

Write-Host "===== 部署到 ${Remote}:$RemoteBase =====" -ForegroundColor Cyan
Invoke-Remote "echo connected as `$(whoami) on `$(hostname)" "连接测试"

# ---- 打包网页、生成环境文件 ----
$stage = New-Item -ItemType Directory -Force (Join-Path ([System.IO.Path]::GetTempPath()) "xmuhub-deploy")
$webTar = Join-Path $stage "web.tar.gz"
Remove-Item $webTar -Force -ErrorAction SilentlyContinue
& tar -czf $webTar -C (Join-Path $Root "web") .
if ($LASTEXITCODE -ne 0) { throw "打包 web 失败" }

$envText = @"
# Managed by scripts/deploy.ps1 — edit .secrets/*.env locally instead.
XMUHUB_STORAGE=github
XMUHUB_BIND=127.0.0.1:$AppPort
XMUHUB_DATA=$RemoteBase/data
XMUHUB_WEB=$RemoteBase/web
XMUHUB_DB_CACHE_MB=32
GH_STORE_USER=$($gh.GH_STORE_USER)
GH_STORE_TOKEN=$($gh.GH_STORE_TOKEN)
GH_REPO_PREFIX=XMUHub-store
GH_BACKUP_REPO=XMUHub-backup
UPLOAD_WORKER_URL=$UploadWorkerUrl
RELAY_DAILY_MB=5120
RELAY_CONCURRENCY=4
UPLOAD_TICKET_SECRET=$($up.UPLOAD_TICKET_SECRET)
RUST_LOG=info,tantivy=warn
MIMALLOC_PURGE_DELAY=0
"@
$envPath = Join-Path $stage "xmuhub.env"
[System.IO.File]::WriteAllText($envPath, ($envText -replace "`r`n", "`n"), (New-Object System.Text.UTF8Encoding($false)))

$unit = @"
[Unit]
Description=XMUHub 厦门大学学生资料共享平台
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=$RemoteBase
EnvironmentFile=$RemoteBase/xmuhub.env
ExecStart=$RemoteBase/run serve
Restart=always
RestartSec=3
LimitNOFILE=65535
# Backstop: normal RSS is tens of MB; never let it crowd out the rest of the host.
MemoryMax=400M
MemorySwapMax=0

[Install]
WantedBy=multi-user.target
"@
# Upload relay: parts up to ~95 MB stream straight through to the app (no request
# buffering, so nginx never spools them to the small disk). Everything else keeps the
# site's normal proxy rule and 50 MB global body limit.
$nginxConf = @'
# Managed by XMUHub scripts/deploy.ps1
location ^~ /api/relay/ {
    proxy_pass http://127.0.0.1:__PORT__;
    proxy_http_version 1.1;
    proxy_set_header Host $http_host;
    proxy_set_header X-Real-IP $remote_addr;
    proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    proxy_set_header X-Forwarded-Proto $scheme;
    client_max_body_size 100m;
    proxy_request_buffering off;
    proxy_send_timeout 1800s;
    proxy_read_timeout 1800s;
}
'@ -replace '__PORT__', "$AppPort"
$nginxPath = Join-Path $stage "xmuhub-relay.conf"
[System.IO.File]::WriteAllText($nginxPath, ($nginxConf -replace "`r`n", "`n"), (New-Object System.Text.UTF8Encoding($false)))

$unitPath = Join-Path $stage "xmuhub.service"
[System.IO.File]::WriteAllText($unitPath, ($unit -replace "`r`n", "`n"), (New-Object System.Text.UTF8Encoding($false)))

try {
    Invoke-Remote "set -e; mkdir -p '$RemoteBase/data'; rm -rf '$RemoteBase/run.new' '$RemoteBase/web.new' '$RemoteBase/web.tar.gz'" "准备目录"
    Push-Location $stage
    try {
        Write-Host "==> 上传 web 包、环境文件、服务单元..." -ForegroundColor DarkGray
        & scp @scpOpts "web.tar.gz" "xmuhub.env" "xmuhub.service" "xmuhub-relay.conf" "${Remote}:$RemoteBase/"
        if ($LASTEXITCODE -ne 0) { throw "上传 web/env 失败" }
    }
    finally { Pop-Location }
    if (-not $WebOnly) {
        Push-Location $Root
        try {
            $mb = [math]::Round((Get-Item $LocalRun).Length / 1MB, 1)
            Write-Host "==> 上传 run ($mb MB) -> run.new ..." -ForegroundColor DarkGray
            & scp @scpOpts "run" "${Remote}:$RemoteBase/run.new"
            if ($LASTEXITCODE -ne 0) { throw "上传 run 失败" }
        }
        finally { Pop-Location }
    }
}
finally {
    Remove-Item $envPath -Force -ErrorAction SilentlyContinue
}

# ---- 替换并重启（停机只在这一小段）----
$swapRun = if ($WebOnly) { "" } else { @"
[ -f run ] && cp -f run run.bak || true
mv -f run.new run
chmod +x run
"@ }
$deploy = @"
set -e
cd '$RemoteBase'
chmod 600 xmuhub.env
mkdir -p web.new && tar -xzf web.tar.gz -C web.new && rm -f web.tar.gz
if ! cmp -s xmuhub.service /etc/systemd/system/$Service; then
  cp -f xmuhub.service /etc/systemd/system/$Service
  systemctl daemon-reload
  systemctl enable $Service >/dev/null 2>&1 || true
  echo '[remote] systemd 单元已更新'
fi
rm -f xmuhub.service
NGX_EXT='$NginxExtDir'
if [ -d "`$NGX_EXT" ] && ! cmp -s xmuhub-relay.conf "`$NGX_EXT/xmuhub-relay.conf"; then
  cp -f xmuhub-relay.conf "`$NGX_EXT/xmuhub-relay.conf"
  if nginx -t >/dev/null 2>&1; then nginx -s reload && echo '[remote] nginx 中转路由已更新'; else rm -f "`$NGX_EXT/xmuhub-relay.conf"; nginx -t; echo '[remote] nginx 配置测试失败，已回退'; exit 1; fi
fi
rm -f xmuhub-relay.conf
systemctl stop $Service 2>/dev/null || true
$swapRun
rm -rf web.bak; [ -d web ] && mv web web.bak || true
mv web.new web
systemctl start $Service
for i in 1 2 3 4 5 6 7 8 9 10; do
  if curl -fsS -o /dev/null http://127.0.0.1:$AppPort/api/meta; then echo '[remote] 健康检查通过'; break; fi
  sleep 1
  if [ `$i = 10 ]; then echo '[remote] 健康检查失败'; journalctl -u $Service -n 40 --no-pager; exit 1; fi
done
pid=`$(systemctl show -p MainPID --value $Service)
echo "[remote] 内存 `$(grep VmRSS /proc/`$pid/status)"
systemctl --no-pager --full status $Service 2>&1 | head -n 8
"@
Write-Host "==> 停服务 -> 替换 -> 启动 ..." -ForegroundColor DarkGray
Invoke-Remote $deploy "替换/重启"

Write-Host ""
Write-Host "===== 部署完成：$PublicUrl =====" -ForegroundColor Green
Write-Host "回滚：ssh $Remote `"cd $RemoteBase && systemctl stop $Service && mv -f run.bak run && rm -rf web && mv web.bak web && systemctl start $Service`""
Write-Host "签发管理员令牌：pwsh scripts/deploy.ps1 -NewAdminToken"
