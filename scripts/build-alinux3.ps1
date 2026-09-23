#!/usr/bin/env pwsh
<#
.SYNOPSIS
    在 Alibaba Cloud Linux 3 容器里编译 XMUHub，产出可直接部署的 `run`。

.DESCRIPTION
    1. 构建（或复用）alinux3 编译镜像；
    2. 把项目挂载进容器，以 release（fat LTO、x86-64-v3）编译 `xmuhub`；
    3. 产物复制为项目根目录下的 `run`，并检查它除 glibc 外没有别的动态库依赖。
    容器内的 target 目录（target-alinux3/）与 cargo 缓存卷会被复用，二次编译很快。
    部署请接着运行 scripts/deploy.ps1。

.EXAMPLE
    pwsh scripts/build-alinux3.ps1
    pwsh scripts/build-alinux3.ps1 -NoCache -Clean
    pwsh scripts/build-alinux3.ps1 -Proxy ""        # 不使用代理
#>
param(
    [string]$ImageName = "xmuhub-alinux3",
    [string]$Proxy = "http://host.docker.internal:7890",
    [string]$TargetCpu = "x86-64-v3",
    [switch]$NoCache,
    [switch]$SkipImageBuild,
    [switch]$Clean
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = (Resolve-Path (Join-Path $ScriptDir "..")).Path
$Dockerfile = Join-Path $ScriptDir "Dockerfile.alinux3"

docker version --format '{{.Server.Version}}' | Out-Null
if ($LASTEXITCODE -ne 0) { throw "docker 不可用，请先启动 Docker Desktop" }

$proxyEnv = @()
if ($Proxy -ne "") {
    $proxyEnv = @("HTTP_PROXY=$Proxy", "HTTPS_PROXY=$Proxy", "ALL_PROXY=$Proxy", "http_proxy=$Proxy", "https_proxy=$Proxy",
        "NO_PROXY=localhost,127.0.0.1,::1,host.docker.internal", "no_proxy=localhost,127.0.0.1,::1,host.docker.internal")
}

# ---- 1. build image ----
if (-not $SkipImageBuild) {
    Write-Host "===== 构建编译镜像 $ImageName =====" -ForegroundColor Cyan
    $buildArgs = @("build", "--progress=plain", "-f", $Dockerfile, "-t", $ImageName)
    if ($Proxy -ne "") {
        $buildArgs += @("--build-arg", "HTTP_PROXY=$Proxy", "--build-arg", "HTTPS_PROXY=$Proxy", "--build-arg", "ALL_PROXY=$Proxy",
            "--build-arg", "NO_PROXY=localhost,127.0.0.1,::1,host.docker.internal")
    }
    if ($NoCache) { $buildArgs += "--no-cache" }
    $buildArgs += $ScriptDir
    docker @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "docker build 失败" }
}

# ---- 2. inner build script (LF, no BOM) ----
$Inner = @'
#!/usr/bin/env bash
set -euo pipefail
cd /work
export CARGO_TARGET_DIR=/work/target-alinux3
if [ "${CLEAN:-0}" = "1" ]; then cargo clean; fi

# C parts (mimalloc, aws-lc) build with clang; aws-lc's gcc probe trips on alinux3's gcc.
export CC=clang CXX=clang++ AR=ar
export CMAKE_C_COMPILER=clang CMAKE_CXX_COMPILER=clang++
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=clang
export RUSTFLAGS="-C target-cpu=${TARGET_CPU} -C link-arg=-fuse-ld=lld"

echo "===== toolchain ====="
rustc --version; cargo --version; clang --version | head -n1
echo "RUSTFLAGS=$RUSTFLAGS"

echo "===== cargo build --release ====="
cargo build --release --locked --bin xmuhub

cp -f "$CARGO_TARGET_DIR/release/xmuhub" ./run
chmod +x ./run

echo "===== ldd ====="
ldd ./run | tee /tmp/ldd.txt
if grep -q "not found" /tmp/ldd.txt; then echo "ERROR: missing shared libraries"; exit 1; fi
if grep -vE "linux-vdso|ld-linux|libc\.so|libm\.so|libgcc_s|libpthread|libdl|librt" /tmp/ldd.txt | grep -q "=>"; then
  echo "ERROR: unexpected non-glibc dependency"; exit 1
fi
echo "===== GLIBC symbols required ====="
objdump -T ./run | grep -oE 'GLIBC_[0-9.]+' | sort -Vu | tail -n 3
ls -lh ./run
echo "OK"
'@
$InnerPath = Join-Path $ScriptDir ".build_alinux3_inner.sh"
[System.IO.File]::WriteAllText($InnerPath, ($Inner -replace "`r`n", "`n"), (New-Object System.Text.UTF8Encoding($false)))

# ---- 3. run build ----
Write-Host "===== 容器内编译 =====" -ForegroundColor Cyan
$runArgs = @("run", "--rm",
    "-e", "CLEAN=$(if ($Clean) { '1' } else { '0' })",
    "-e", "TARGET_CPU=$TargetCpu",
    "--mount", "type=bind,source=$Root,target=/work",
    "-v", "xmuhub-cargo-registry:/root/.cargo/registry",
    "-v", "xmuhub-cargo-git:/root/.cargo/git",
    "-w", "/work")
foreach ($e in $proxyEnv) { $runArgs += @("-e", $e) }
$runArgs += @($ImageName, "bash", "/work/scripts/.build_alinux3_inner.sh")
docker @runArgs
if ($LASTEXITCODE -ne 0) { throw "容器内编译失败" }

$Run = Join-Path $Root "run"
if (-not (Test-Path $Run)) { throw "没有生成 run：$Run" }
Write-Host ""
Write-Host "===== 编译完成 =====" -ForegroundColor Green
Write-Host ("run => {0} ({1:N1} MB)" -f $Run, ((Get-Item $Run).Length / 1MB))
Write-Host "下一步：pwsh scripts/deploy.ps1"
