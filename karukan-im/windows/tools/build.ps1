# Build the Windows pieces: karukan-imserver.exe and karukan_tsf.dll (release).
#   .\build.ps1          incremental
#   .\build.ps1 -Clean   also rebuilds llama.cpp from scratch
# Output goes to D:\cargo-target\karukan\release (install.ps1 -Source points there).
# Why these settings (each one fixed a failure on this machine):
#   - target outside the repo: lib.exe / MSBuild could not create temp files for
#     build scripts run from <repo>\target (LNK1104, MSB6003)
#   - Ninja for llama.cpp: the Visual Studio generator still failed at MSB6003
#   - TEMP on D:, LIBCLANG_PATH for bindgen, rustc >= 1.95 for sysinfo
[CmdletBinding()]
param(
    [switch]$Clean,
    [string]$Target = 'D:\cargo-target\karukan'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..\..\..')).Path
Set-Location $root

$env:CARGO_TARGET_DIR = $Target
New-Item -ItemType Directory -Force 'D:\claude-tmp' | Out-Null
$env:TEMP = 'D:\claude-tmp'
$env:TMP = 'D:\claude-tmp'
if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = 'D:\Tools\libclang\clang\native' }
# the toolchain and registry on D: (the default cargo home is on C:, and its registry is stale here)
if (-not $env:CARGO_HOME -and (Test-Path 'D:\Tools\cargo-home')) { $env:CARGO_HOME = 'D:\Tools\cargo-home'; $env:PATH = "D:\Tools\cargo-home\bin;$env:PATH" }
if (-not $env:RUSTUP_HOME -and (Test-Path 'D:\Tools\rustup')) { $env:RUSTUP_HOME = 'D:\Tools\rustup' }
$ninja = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\Preview\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja'
if (Test-Path $ninja) {
    $env:PATH = "$ninja;$env:PATH"
    $env:CMAKE_GENERATOR = 'Ninja'
}

if ($Clean) {
    Get-ChildItem (Join-Path $Target 'release\build') -Directory -Filter 'llama-cpp-sys-2-*' -ErrorAction SilentlyContinue |
        ForEach-Object { Remove-Item (Join-Path $_.FullName 'out') -Recurse -Force -ErrorAction SilentlyContinue }
}

$t0 = Get-Date
cargo +1.96.0 build --release -p karukan-im --bin karukan-imserver --offline
if ($LASTEXITCODE -ne 0) { "BUILD-FAILED imserver exit=$LASTEXITCODE"; exit 1 }
cargo build --release -p karukan-tsf -p karukan-tsf-loader --offline
if ($LASTEXITCODE -ne 0) { "BUILD-FAILED tsf exit=$LASTEXITCODE"; exit 1 }

$dir = Join-Path $Target 'release'
Get-Item (Join-Path $dir 'karukan-imserver.exe'), (Join-Path $dir 'karukan_tsf.dll') | Select-Object Name, Length, LastWriteTime | Format-Table | Out-String
"BUILD-DONE seconds=$([int]((Get-Date) - $t0).TotalSeconds)"
