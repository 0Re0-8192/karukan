# Checks to run before pushing (this repo has no CI): format, clippy -D warnings, unit tests.
#   .\check.ps1          everything
#   .\check.ps1 -NoTest  format and clippy only
# Same toolchain and target as build.ps1, so the llama.cpp build is shared with it.
# Log: <Target>\check.log. Exit code 1 if any step failed.
[CmdletBinding()]
param(
    [switch]$NoTest,
    [string]$Target = 'D:\cargo-target\karukan',
    [string]$Temp = 'E:\Temp'
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..\..\..')).Path
Set-Location $root

$env:CARGO_TARGET_DIR = $Target
New-Item -ItemType Directory -Force $Temp | Out-Null
$env:TEMP = $Temp
$env:TMP = $Temp
if (-not $env:LIBCLANG_PATH) { $env:LIBCLANG_PATH = 'D:\Tools\libclang\clang\native' }
# the toolchain and registry on D: (the default cargo home is on C:, and its registry is stale here)
if (-not $env:CARGO_HOME -and (Test-Path 'D:\Tools\cargo-home')) { $env:CARGO_HOME = 'D:\Tools\cargo-home'; $env:PATH = "D:\Tools\cargo-home\bin;$env:PATH" }
if (-not $env:RUSTUP_HOME -and (Test-Path 'D:\Tools\rustup')) { $env:RUSTUP_HOME = 'D:\Tools\rustup' }
$ninja = 'C:\Program Files (x86)\Microsoft Visual Studio\2022\Preview\Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja'
if (Test-Path $ninja) {
    $env:PATH = "$ninja;$env:PATH"
    $env:CMAKE_GENERATOR = 'Ninja'
}

$log = Join-Path $Target 'check.log'
Set-Content $log '' -Encoding utf8
$failed = @()
function Step([string]$name, [string[]]$a) {
    "== $name" | Add-Content $log
    $ErrorActionPreference = 'Continue'
    & cargo +1.96.0 @a 2>&1 | ForEach-Object { "$_" } | Add-Content $log
    $code = $LASTEXITCODE
    "$name exit=$code" | Add-Content $log
    "$name exit=$code"
    if ($code -ne 0) { $script:failed += $name }
}

Step 'fmt' @('fmt', '--all', '--', '--check')
Step 'clippy' @('clippy', '--release', '-p', 'karukan-engine', '-p', 'karukan-im', '-p', 'karukan-tsf',
    '-p', 'karukan-tsf-loader', '-p', 'karukan-setup', '--all-targets', '--', '-D', 'warnings')
if (-not $NoTest) {
    Step 'test-engine' @('test', '--release', '-p', 'karukan-engine', '--lib', '--bins')
    Step 'test-im' @('test', '--release', '-p', 'karukan-im', '--lib')
    Step 'test-tsf' @('test', '--release', '-p', 'karukan-tsf', '--lib')
    Step 'test-setup' @('test', '--release', '-p', 'karukan-setup')
}

if ($failed.Count) { "CHECK-FAILED $($failed -join ', ') (log: $log)"; exit 1 }
"CHECK-OK (log: $log)"
