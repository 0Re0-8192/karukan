# Builds karukan for Windows as an installer (release\karukan-setup-<version>.exe: install, update in place,
# uninstall from Settings > Apps) and as a portable zip (unzip anywhere, run setup.cmd once). No build tools,
# no network needed on the target. The program files come from an installed, working dist; the models are laid
# out as a HuggingFace cache so the server finds them without touching the network.
#   .\package.ps1                  -> <repo>\release\karukan-setup-<version>.exe and karukan-windows-<date>.zip
#   .\package.ps1 -NoZip           -> the installer only
#   .\package.ps1 -Dist D:\Karukan -Out E:\out
[CmdletBinding()]
param(
    [string]$Dist,
    [string]$Out,
    [string]$SmallGguf = 'D:\Projects\train-data\gguf9b_0.5',
    [string]$Build = 'D:\cargo-target\karukan\release',
    [string]$Setup = 'D:\cargo-target\karukan\release\karukan-setup.exe',
    [switch]$NoZip
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
if (-not $Dist) { $Dist = Join-Path $root 'dist' }
if (-not $Out) { $Out = Join-Path $root 'release' }

# what goes in, pinned (the published small model, the public xsmall, the v0.1.0 dictionary)
$SmallRepo = 'OReOOreoaaaa/jinen-v2-small-karukan.gguf'; $SmallRev = 'c02dc9bdd408b3b33491428ab9d3c211929f8c67'
$SmallFile = 'jinen-v2-small-karukan-r9b-Q8_0.gguf'
$XsmallRepo = 'togatogah/jinen-v2-xsmall.gguf'; $XsmallRev = '3910fd01bf4ba86eca89617f18db9b0c1c5b2283'
$Sha = @{
    $SmallFile                  = '0ec64db059dde94e9f56803bda2593c8e1922d9e89ac255d585ee1af9beac60a'
    'jinen-v2-xsmall-Q8_0.gguf' = 'bc75a02512bdbb2a4786fec281a57d4d58ac3c2f118c3a29c2a9b41f2a4898b7'
    'tokenizer.json'            = '477c91a788bf97e7ed993a4441cfb2c4d60033e505ade17d6c367bc09ed6e6ba'
    'dict.bin'                  = 'd85fed3c7e408e67f4a5dbe2314338366ed6183e0498d2726bc5a4dcaf5bb83b'
}
function Copy-Checked($src, $dst) {
    $name = Split-Path $src -Leaf
    $h = (Get-FileHash $src -Algorithm SHA256).Hash.ToLower()
    if ($Sha.ContainsKey($name) -and $h -ne $Sha[$name]) { throw "$src`: sha256 $h, expected $($Sha[$name])" }
    New-Item -ItemType Directory -Force (Split-Path $dst) | Out-Null
    Copy-Item $src $dst
}
function Add-HfCache($hub, $repo, $rev, $files) {
    $d = Join-Path $hub ('models--' + $repo.Replace('/', '--'))
    New-Item -ItemType Directory -Force (Join-Path $d 'refs') | Out-Null
    [IO.File]::WriteAllText((Join-Path $d 'refs\main'), $rev)
    foreach ($f in $files) { Copy-Checked $f (Join-Path $d "snapshots\$rev\$(Split-Path $f -Leaf)") }
}

$stage = Join-Path $Out 'karukan'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force $stage | Out-Null

# 1. program: the loader (the registered DLL) from the installed dist, so an update that keeps it needs no
#    admin; the implementation DLL and the server from the build (build.ps1), the DLL under a name of its
#    build time as install.ps1 names it (an app holding the old one keeps it; new apps load the new one)
Copy-Item (Join-Path $Dist 'karukan_tsf.dll') $stage
$implSrc = Get-Item (Join-Path $Build 'karukan_tsf.dll')
Copy-Item $implSrc.FullName (Join-Path $stage ('karukan_tsf_impl_{0}.dll' -f $implSrc.LastWriteTime.ToString('yyMMddHHmmss')))
Copy-Item (Join-Path $Build 'karukan-imserver.exe') $stage
New-Item -ItemType File (Join-Path $stage 'portable.flag') | Out-Null
# the server's C++ runtime, app-local: llama.cpp is built with VS 2022 17.10+, whose std::mutex crashes in
# an older msvcp140.dll (0xc0000005 seen with 14.28 on a Windows 10 PC). Only the server loads these.
$redist = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\*\VC\Redist\MSVC\14.*\x64" -Directory -ErrorAction SilentlyContinue |
    Sort-Object { [version]($_.Parent.Name) } | Select-Object -Last 1
if (-not $redist) { throw 'Visual Studio 2022 VC redist folder not found' }
foreach ($f in 'Microsoft.VC143.CRT\msvcp140.dll', 'Microsoft.VC143.CRT\vcruntime140.dll', 'Microsoft.VC143.CRT\vcruntime140_1.dll', 'Microsoft.VC143.OpenMP\vcomp140.dll') {
    Copy-Item (Join-Path $redist.FullName $f) $stage
}
"VC runtime from $($redist.FullName)"

# 2. data: the system dictionary only (no learning, no user dictionaries)
Copy-Checked (Join-Path $Dist 'data\dict.bin') (Join-Path $stage 'data\dict.bin')

# 3. models as a HuggingFace cache under hf\ (the server is started with HF_HOME there)
$hub = Join-Path $stage 'hf\hub'
Add-HfCache $hub $SmallRepo $SmallRev @((Join-Path $SmallGguf $SmallFile), (Join-Path $SmallGguf 'tokenizer.json'))
$xs = Join-Path $Dist "hf\hub\models--togatogah--jinen-v2-xsmall.gguf\snapshots\$XsmallRev"
Add-HfCache $hub $XsmallRepo $XsmallRev @((Join-Path $xs 'jinen-v2-xsmall-Q8_0.gguf'), (Join-Path $xs 'tokenizer.json'))

# 4. licenses (the main model is the built-in default, jinen-v2-small-karukan-q8, so no config is shipped:
#    config\config.toml stays the user's own)
$lic = Join-Path $stage 'licenses'
New-Item -ItemType Directory -Force (Join-Path $lic 'karukan'), (Join-Path $lic 'model') | Out-Null
Copy-Item (Join-Path $root 'LICENSE-MIT'), (Join-Path $root 'LICENSE-APACHE') (Join-Path $lic 'karukan')
Copy-Item (Join-Path $PSScriptRoot 'package\licenses\dict') $lic -Recurse
Copy-Item 'D:\Projects\train-data\hf_r9b\README.md' (Join-Path $lic 'model\jinen-v2-small-karukan.md')
Copy-Item (Join-Path $PSScriptRoot 'package\README.txt') $stage

# 5. the installer: karukan-setup.exe with this folder packed into it
$setupVersion = (Select-String -Path (Join-Path $root 'karukan-im\windows\setup\Cargo.toml') -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
$installer = Join-Path $Out "karukan-setup-$setupVersion.exe"
& $Setup pack $stage -o $installer | Out-Host
if ($LASTEXITCODE -ne 0) { throw "pack failed: $LASTEXITCODE" }
$i = Get-Item $installer
"{0}  {1:N0} MB  sha256 {2}" -f $i.FullName, ($i.Length / 1MB), (Get-FileHash $installer).Hash.ToLower()
if ($NoZip) { Remove-Item $stage -Recurse -Force; return }

# 6. the zip: the same folder plus setup.cmd / uninstall.cmd (for installing without the exe)
foreach ($f in 'setup.ps1', 'uninstall.ps1') {
    Copy-Item (Join-Path $PSScriptRoot "package\$f") $stage
}
foreach ($n in 'setup', 'uninstall') {
    $cmd = "@echo off`r`npowershell -NoProfile -ExecutionPolicy Bypass -File `"%~dp0$n.ps1`"`r`npause`r`n"
    [IO.File]::WriteAllText((Join-Path $stage "$n.cmd"), $cmd, [Text.Encoding]::ASCII)
}

#    (bsdtar from System32: fast, and no 2 GB limit)
$zip = Join-Path $Out ("karukan-windows-{0}.zip" -f (Get-Date -Format 'yyyyMMdd'))
if (Test-Path $zip) { Remove-Item $zip }
Push-Location $Out
try { & "$env:SystemRoot\System32\tar.exe" -a -c -f $zip karukan } finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw "tar failed: $LASTEXITCODE" }
Remove-Item $stage -Recurse -Force
$z = Get-Item $zip
"{0}  {1:N0} MB  sha256 {2}" -f $z.FullName, ($z.Length / 1MB), (Get-FileHash $zip).Hash.ToLower()
