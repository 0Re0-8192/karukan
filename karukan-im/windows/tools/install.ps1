# Install / update karukan (Windows TSF) in one go.
#   .\install.ps1                 build output -> <repo>\dist, register, enable for this user
#   .\install.ps1 -Dest D:\Karukan
#   .\install.ps1 -Uninstall
# A running DLL/server is replaced by renaming it aside, so open apps keep working;
# new apps pick up the new files.
[CmdletBinding()]
param(
    [string]$Source,
    [string]$Dest,
    [switch]$SkipDict,
    [switch]$Uninstall
)
$ErrorActionPreference = 'Stop'
$root = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) '..\..\..'
if (-not $Source) { $Source = Join-Path $root 'target\release' }
if (-not $Dest)   { $Dest   = Join-Path $root 'dist' }

$Clsid   = '{4B9F6E5A-1D3C-4C7E-9A52-8E21F0C7A311}'
$Profile = '{A1C2E3D4-5F60-4718-9B2A-3C4D5E6F7081}'
$DictUrl = 'https://github.com/togatoga/karukan/releases/latest/download/dict.tgz'

New-Item -ItemType Directory -Force $Dest | Out-Null
$Dest = (Resolve-Path $Dest).Path
$dll  = Join-Path $Dest 'karukan_tsf.dll'

function Invoke-Regsvr([string[]]$a) {
    $isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    $args2 = @{ FilePath = 'regsvr32.exe'; ArgumentList = $a; Wait = $true; PassThru = $true }
    if (-not $isAdmin) { $args2.Verb = 'RunAs' }
    $p = Start-Process @args2
    if ($p.ExitCode -ne 0) { throw "regsvr32 $($a -join ' ') failed: exit $($p.ExitCode)" }
}

if ($Uninstall) {
    if (Test-Path $dll) { Invoke-Regsvr @('/s', '/u', $dll) }
    "unregistered $dll"
    return
}

# 1. files: move a differing (possibly locked) file aside, then copy the new one
$stamp = Get-Date -Format 'yyMMddHHmmss'
foreach ($name in 'karukan_tsf.dll', 'karukan-imserver.exe') {
    $src = Join-Path $Source $name
    if (-not (Test-Path $src)) { throw "missing build output: $src" }
    $dst = Join-Path $Dest $name
    if ((Test-Path $dst) -and ((Get-FileHash $src).Hash -eq (Get-FileHash $dst).Hash)) { "same: $name"; continue }
    if (Test-Path $dst) { Move-Item $dst "$dst.old$stamp" }
    Copy-Item $src $dst
    "copied: $name"
}
# old copies that nothing holds any more
Get-ChildItem $Dest -Filter 'karukan*.old*' | ForEach-Object { try { Remove-Item $_.FullName -ErrorAction Stop } catch {} }

# 2. portable layout: settings, learning and models stay under $Dest
$flag = Join-Path $Dest 'portable.flag'
if (-not (Test-Path $flag)) { New-Item -ItemType File $flag | Out-Null }
New-Item -ItemType Directory -Force (Join-Path $Dest 'data') | Out-Null

# 3. system dictionary (without it only the model and the user's own entries convert)
$dict = Join-Path $Dest 'data\dict.bin'
if (-not $SkipDict -and -not (Test-Path $dict)) {
    $tmp = Join-Path $Dest 'data\tmp'
    New-Item -ItemType Directory -Force $tmp | Out-Null
    $tgz = Join-Path $tmp 'dict.tgz'
    Invoke-WebRequest $DictUrl -OutFile $tgz
    tar -xzf $tgz -C $tmp
    $found = Get-ChildItem $tmp -Recurse -Filter 'dict.bin' | Select-Object -First 1
    if (-not $found) { throw 'dict.bin not found in dict.tgz' }
    Move-Item $found.FullName $dict
    Remove-Item $tmp -Recurse -Force
    "dictionary: $dict"
}

# 4. COM + TSF profile (admin), then switch it on for this user
Invoke-Regsvr @('/s', $dll)
Add-Type -TypeDefinition @'
using System.Runtime.InteropServices;
public static class KarukanInputDll {
  [DllImport("input.dll", CharSet = CharSet.Unicode)]
  public static extern bool InstallLayoutOrTip(string psz, uint dwFlags);
}
'@
$ok = [KarukanInputDll]::InstallLayoutOrTip("0411:$Clsid$Profile", 0)
"registered, enabled for user: $ok  (reopen apps to load the new DLL)"
