# Builds the portable zip of karukan for Windows: unzip anywhere, run setup.cmd once, done (no build tools,
# no network). The program files come from an installed, working dist; the models are laid out as a
# HuggingFace cache so the server finds them without touching the network.
#   .\package.ps1                         -> <repo>\release\karukan-windows-<date>.zip
#   .\package.ps1 -Dist D:\Karukan -Out E:\out
[CmdletBinding()]
param(
    [string]$Dist,
    [string]$Out,
    [string]$SmallGguf = 'D:\Projects\train-data\gguf9b_0.5'
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

# 1. program: the loader (the registered DLL), the newest implementation DLL, the server
Copy-Item (Join-Path $Dist 'karukan_tsf.dll') $stage
$impl = Get-ChildItem $Dist -Filter 'karukan_tsf_impl_*.dll' | Sort-Object Name | Select-Object -Last 1
if (-not $impl) { throw "no karukan_tsf_impl_*.dll in $Dist" }
Copy-Item $impl.FullName $stage
Copy-Item (Join-Path $Dist 'karukan-imserver.exe') $stage
New-Item -ItemType File (Join-Path $stage 'portable.flag') | Out-Null

# 2. data: the system dictionary only (no learning, no user dictionaries)
Copy-Checked (Join-Path $Dist 'data\dict.bin') (Join-Path $stage 'data\dict.bin')

# 3. models as a HuggingFace cache under hf\ (the server is started with HF_HOME there)
$hub = Join-Path $stage 'hf\hub'
Add-HfCache $hub $SmallRepo $SmallRev @((Join-Path $SmallGguf $SmallFile), (Join-Path $SmallGguf 'tokenizer.json'))
$xs = Join-Path $Dist "hf\hub\models--togatogah--jinen-v2-xsmall.gguf\snapshots\$XsmallRev"
Add-HfCache $hub $XsmallRepo $XsmallRev @((Join-Path $xs 'jinen-v2-xsmall-Q8_0.gguf'), (Join-Path $xs 'tokenizer.json'))

# 4. config: the published small model as the main model; everything else is the built-in default
$cfg = @"
# karukan settings: only what is written here overrides the built-in defaults (see docs/configuration.md)
[conversion]
model = "jinen-v2-small-karukan-r9b"

[models]
jinen-v2-small-karukan-r9b = { repo = "$SmallRepo", filename = "$SmallFile" }
"@
New-Item -ItemType Directory -Force (Join-Path $stage 'config') | Out-Null
[IO.File]::WriteAllText((Join-Path $stage 'config\config.toml'), $cfg.Replace("`r`n", "`n"), (New-Object Text.UTF8Encoding $false))

# 5. setup / uninstall / readme
foreach ($f in 'setup.ps1', 'uninstall.ps1', 'README.txt') {
    Copy-Item (Join-Path $PSScriptRoot "package\$f") $stage
}
$lic = Join-Path $stage 'licenses'
New-Item -ItemType Directory -Force (Join-Path $lic 'karukan'), (Join-Path $lic 'model') | Out-Null
Copy-Item (Join-Path $root 'LICENSE-MIT'), (Join-Path $root 'LICENSE-APACHE') (Join-Path $lic 'karukan')
Copy-Item (Join-Path $PSScriptRoot 'package\licenses\dict') $lic -Recurse
Copy-Item 'D:\Projects\train-data\hf_r9b\README.md' (Join-Path $lic 'model\jinen-v2-small-karukan.md')
foreach ($n in 'setup', 'uninstall') {
    $cmd = "@echo off`r`npowershell -NoProfile -ExecutionPolicy Bypass -File `"%~dp0$n.ps1`"`r`npause`r`n"
    [IO.File]::WriteAllText((Join-Path $stage "$n.cmd"), $cmd, [Text.Encoding]::ASCII)
}

# 6. zip (bsdtar from System32: fast, and no 2 GB limit)
$zip = Join-Path $Out ("karukan-windows-{0}.zip" -f (Get-Date -Format 'yyyyMMdd'))
if (Test-Path $zip) { Remove-Item $zip }
Push-Location $Out
try { & "$env:SystemRoot\System32\tar.exe" -a -c -f $zip karukan } finally { Pop-Location }
if ($LASTEXITCODE -ne 0) { throw "tar failed: $LASTEXITCODE" }
Remove-Item $stage -Recurse -Force
$z = Get-Item $zip
"{0}  {1:N0} MB  sha256 {2}" -f $z.FullName, ($z.Length / 1MB), (Get-FileHash $zip).Hash.ToLower()
