#!/usr/bin/env pwsh
<#
.SYNOPSIS
    和 GitHub 同步，并只部署需要部署的部分：网页改了只更新网页，Rust 改了才重新编译。

.DESCRIPTION
    1. git fetch；落后就快进（git pull），本地有未推送的提交就先 rebase 到远端再推送；
       工作区有未提交的改动、或者 rebase 冲突时停下，什么都不部署。
    2. 读服务器上的 /root/xmuhub/DEPLOYED（deploy.ps1 每次部署后写入：bin=程序对应的提交，
       web=网页对应的提交），和当前 HEAD 比较改了哪些文件：
         crates/、Cargo.toml、Cargo.lock、编译脚本 → build-alinux3.ps1 + deploy.ps1（程序和网页一起更新）
         web/、deploy.ps1                         → deploy.ps1 -WebOnly
         其他（README、文档…）                    → 不部署
       记录缺失、对不上号（比如有人强推过）或带 -dirty 的，按「需要部署」处理。
    3. 部署失败时停下；-Watch 模式下同一个提交失败后不再重试，等有新提交再说。

.EXAMPLE
    pwsh scripts/sync.ps1              # 同步一次
    pwsh scripts/sync.ps1 -DryRun      # 只看会做什么
    pwsh scripts/sync.ps1 -Watch 300   # 每 5 分钟检查一次，一直运行
    pwsh scripts/sync.ps1 -Force       # 不管记录，重新编译并完整部署
#>
param(
    [string]$Branch = "main",
    [string]$User = "root",
    [string]$HostName = "vintces.icu",
    [string]$RemoteBase = "/root/xmuhub",
    # Seconds between checks; 0 = run once.
    [int]$Watch = 0,
    [switch]$DryRun,
    [switch]$Force,
    # Don't push local commits to GitHub (still pulls and deploys).
    [switch]$NoPush
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = (Resolve-Path (Join-Path $ScriptDir "..")).Path
# Files whose change needs a new binary / a web-only deploy.
$BinPaths = @("crates", "Cargo.toml", "Cargo.lock", "scripts/build-alinux3.ps1", "scripts/Dockerfile.alinux3")
$WebPaths = @("web", "scripts/deploy.ps1")

function Log([string]$Msg, [string]$Color = "Gray") {
    Write-Host ("[{0}] {1}" -f (Get-Date -Format "MM-dd HH:mm:ss"), $Msg) -ForegroundColor $Color
}

function Invoke-Git {
    $out = & git -C $Root @args 2>&1
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') 失败：$($out -join "`n")" }
    return $out
}

function Read-Deployed {
    $text = & ssh -o BatchMode=yes -o ConnectTimeout=10 "$User@$HostName" "cat '$RemoteBase/DEPLOYED' 2>/dev/null || true"
    if ($LASTEXITCODE -ne 0) { throw "连不上服务器 $HostName" }
    $h = @{ bin = ""; web = "" }
    foreach ($line in $text) { if ($line -match '^(bin|web)=(.*)$') { $h[$Matches[1]] = $Matches[2].Trim() } }
    return $h
}

function Short([string]$Rev) { if ($Rev -and $Rev.Length -ge 7) { $Rev.Substring(0, 7) + $(if ($Rev.EndsWith("-dirty")) { "-dirty" } else { "" }) } else { "（无记录）" } }

function Sync-Once {
    # A token in the environment makes git push authenticate as the wrong account.
    Remove-Item Env:GITHUB_TOKEN -ErrorAction SilentlyContinue

    # ---- 1. git
    $branch = (Invoke-Git rev-parse --abbrev-ref HEAD).Trim()
    if ($branch -ne $Branch) { throw "当前在分支 $branch，不是 $Branch；请先切回去" }
    $dirty = Invoke-Git status --porcelain --untracked-files=no
    if ($dirty) { throw "工作区有未提交的改动，先提交或还原：`n$($dirty -join "`n")" }

    Invoke-Git fetch --quiet origin $Branch | Out-Null
    $counts = (Invoke-Git rev-list --left-right --count "HEAD...origin/$Branch").Trim() -split '\s+'
    $ahead, $behind = [int]$counts[0], [int]$counts[1]
    if ($behind -gt 0) {
        $log = Invoke-Git log --format="  %h %an: %s" "HEAD..origin/$Branch"
        Log "远端有 $behind 个新提交：`n$($log -join "`n")" Cyan
        if ($DryRun) { Log "（DryRun：不拉取；下面按拉取后的版本判断）" }
        elseif ($ahead -eq 0) { Invoke-Git merge --ff-only --quiet "origin/$Branch" | Out-Null }
        else {
            Log "本地也有 $ahead 个未推送的提交，rebase 到远端之上..." Yellow
            & git -C $Root rebase --quiet "origin/$Branch"
            if ($LASTEXITCODE -ne 0) {
                & git -C $Root rebase --abort 2>$null
                throw "rebase 有冲突，已还原；请手动合并后再运行"
            }
        }
    }
    if ($ahead -gt 0 -and -not $NoPush) {
        if ($DryRun) { Log "（DryRun：会推送 $ahead 个本地提交）" }
        else {
            Log "推送 $ahead 个本地提交到 GitHub..." Cyan
            Invoke-Git push --quiet origin "HEAD:$Branch" | Out-Null
        }
    }
    $head = if ($DryRun -and $behind -gt 0) { (Invoke-Git rev-parse "origin/$Branch").Trim() } else { (Invoke-Git rev-parse HEAD).Trim() }

    # ---- 2. what the server runs
    $dep = Read-Deployed
    # (Dry run judges the fetched remote head without checking it out.)
    $needBin = $Force -or -not (Up-To-Date-At $dep.bin $BinPaths $head)
    $needWeb = $Force -or $needBin -or -not (Up-To-Date-At $dep.web $WebPaths $head)
    Log ("服务器：程序 {0}，网页 {1}；目标 {2}" -f (Short $dep.bin), (Short $dep.web), (Short $head))

    if (-not $needBin -and -not $needWeb) { Log "已是最新，不用部署" Green; return $head }
    $plan = if ($needBin) { "重新编译并完整部署" } else { "只更新网页（-WebOnly）" }
    if ($DryRun) { Log "（DryRun）会：$plan" Yellow; return $head }

    # ---- 3. deploy
    Log $plan Cyan
    if ($needBin) {
        $built = if (Test-Path (Join-Path $Root "run.commit")) { (Get-Content (Join-Path $Root "run.commit") -Raw).Trim() } else { "" }
        if ($built -eq $head -and (Test-Path (Join-Path $Root "run"))) { Log "本地的 run 已经是这个版本编译的，跳过编译" }
        else {
            & (Join-Path $ScriptDir "build-alinux3.ps1")
            if (-not $?) { throw "编译失败" }
        }
        & (Join-Path $ScriptDir "deploy.ps1") -User $User -HostName $HostName -RemoteBase $RemoteBase
    }
    else {
        & (Join-Path $ScriptDir "deploy.ps1") -User $User -HostName $HostName -RemoteBase $RemoteBase -WebOnly
    }
    if (-not $?) { throw "部署失败" }
    Log "部署完成：$(Short $head)" Green
    return $head
}

# Is `$Rev` a commit that `$Target` builds on, with nothing under `$Paths` changed since?
function Up-To-Date-At([string]$Rev, [string[]]$Paths, [string]$Target) {
    if (-not $Rev -or $Rev -eq "unknown" -or $Rev.EndsWith("-dirty")) { return $false }
    & git -C $Root cat-file -e "$Rev^{commit}" 2>$null
    if ($LASTEXITCODE -ne 0) { return $false }
    & git -C $Root merge-base --is-ancestor $Rev $Target 2>$null
    if ($LASTEXITCODE -ne 0) { return $false }
    $changed = & git -C $Root diff --name-only $Rev $Target -- @Paths
    return -not $changed
}

# One sync at a time (a scheduled run and a manual one must not deploy together).
$lockPath = Join-Path $Root ".git/xmuhub-sync.lock"
try { $lock = [System.IO.File]::Open($lockPath, 'OpenOrCreate', 'ReadWrite', 'None') }
catch { Log "另一个 sync.ps1 正在运行，退出" Yellow; exit 0 }

try {
    if ($Watch -le 0) {
        try { Sync-Once | Out-Null }
        catch { Log $_.Exception.Message Red; exit 1 }
        exit 0
    }
    Log "每 $Watch 秒检查一次（Ctrl+C 退出）"
    # After a failure, wait until something changes (remote, local HEAD or the working tree)
    # instead of rebuilding the same broken commit every few minutes.
    $state = { "{0}|{1}|{2}" -f ((& git -C $Root ls-remote origin "refs/heads/$Branch") -split '\s+')[0], (& git -C $Root rev-parse HEAD), ((& git -C $Root status --porcelain --untracked-files=no) -join ';') }
    $failedAt = ""
    while ($true) {
        try {
            if ($failedAt -and (& $state) -eq $failedAt) { }
            else {
                $failedAt = ""
                Sync-Once | Out-Null
            }
        }
        catch {
            Log $_.Exception.Message Red
            $failedAt = & $state
            Log "先不重试，等有新提交或改动后再处理" Yellow
        }
        Start-Sleep -Seconds $Watch
    }
}
finally { $lock.Dispose() }
