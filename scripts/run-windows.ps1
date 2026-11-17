[CmdletBinding()]
param(
    [string]$Config = "config.toml",
    [switch]$Release
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

Write-Host "[1/4] Checking Zig and cargo-zigbuild..." -ForegroundColor Cyan
if (-not (Get-Command zig -ErrorAction SilentlyContinue)) {
    throw "zig was not found. Install Zig and add zig.exe to PATH: https://ziglang.org/download/"
}
$cargoZigbuild = Get-Command cargo-zigbuild -ErrorAction SilentlyContinue
if (-not $cargoZigbuild) {
    Write-Host "cargo-zigbuild is missing; installing it now (one-time)..." -ForegroundColor Yellow
    cargo install cargo-zigbuild
    if ($LASTEXITCODE -ne 0) {
        throw "cargo install cargo-zigbuild failed."
    }
}

Write-Host "[2/4] Ensuring the Windows GNU Rust target..." -ForegroundColor Cyan
rustup target add x86_64-pc-windows-gnu
if ($LASTEXITCODE -ne 0) {
    throw "Could not install x86_64-pc-windows-gnu. Check rustup and network access."
}

# Keep Cargo/Zig temporary files inside the workspace. This avoids odd
# Windows permission issues in the user profile and makes cleanup predictable.
$tmpDir = Join-Path $root ".tmp"
$zigCache = Join-Path $root ".zig-cache"
New-Item -ItemType Directory -Force -Path $tmpDir, $zigCache | Out-Null
$env:TMP = $tmpDir
$env:TEMP = $tmpDir
$env:TMPDIR = $tmpDir
$env:ZIG_GLOBAL_CACHE_DIR = $zigCache

Write-Host "[3/4] Building the Windows GNU binary (MSVC is not used)..." -ForegroundColor Cyan
if ($Release) {
    cargo zigbuild --release --target x86_64-pc-windows-gnu
} else {
    cargo zigbuild --target x86_64-pc-windows-gnu
}
if ($LASTEXITCODE -ne 0) {
    throw "Build failed. See the Cargo output above."
}

$profileDir = if ($Release) { "release" } else { "debug" }
$exePath = Join-Path $root "target\x86_64-pc-windows-gnu\$profileDir\mcsm-auto-deployer.exe"
if (-not (Test-Path $exePath)) {
    throw "Expected binary was not produced: $exePath"
}

Write-Host "[4/4] Starting MCSM-Auto-Deployer with $Config..." -ForegroundColor Green
Write-Host "Binary: $exePath" -ForegroundColor DarkGray
& $exePath --config $Config
exit $LASTEXITCODE
