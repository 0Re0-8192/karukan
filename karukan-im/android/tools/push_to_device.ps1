# Copy the models and the system dictionary to a device for the Gboard patch (karukan Japanese input).
#   .\push_to_device.ps1 -Serial <adb serial> [-Package com.google.android.inputmethod.latin]
# Files land in /sdcard/Android/data/<package>/files/karukan (see KarukanHook.java in Re0-Patch).
# -Serial is required on purpose: never let adb pick a device by itself.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Serial,
    [string]$Package = 'com.google.android.inputmethod.latin',
    [string]$Hub = 'D:\Projects\karukan\dist\hf\hub',
    [string]$Dict = 'D:\Projects\karukan\dist\data\dict.bin'
)
$ErrorActionPreference = 'Stop'
$env:TEMP = 'E:\Temp'; $env:TMP = 'E:\Temp'
$adb = 'adb'
$dest = "/sdcard/Android/data/$Package/files/karukan"

$devices = & $adb devices | Select-String "^$([regex]::Escape($Serial))\s+device"
if (-not $devices) { throw "device $Serial is not connected" }

function Push($local, $remote) {
    if (-not (Test-Path $local)) { throw "missing: $local" }
    $remoteDir = $remote -replace '/[^/]*$', ''
    & $adb -s $Serial shell "mkdir -p '$remoteDir'" | Out-Null
    & $adb -s $Serial push $local $remote
    if ($LASTEXITCODE -ne 0) { throw "push failed: $local" }
}

foreach ($m in @(@('small', 'jinen-v2-small.gguf', 'jinen-v2-small-Q8_0.gguf'),
                 @('xsmall', 'jinen-v2-xsmall.gguf', 'jinen-v2-xsmall-Q8_0.gguf'))) {
    $snap = Get-ChildItem (Join-Path $Hub "models--togatogah--$($m[1])\snapshots") -Directory | Select-Object -First 1
    Push (Join-Path $snap.FullName $m[2]) "$dest/models/$($m[0])/$($m[2])"
    Push (Join-Path $snap.FullName 'tokenizer.json') "$dest/models/$($m[0])/tokenizer.json"
}
Push $Dict "$dest/data/dict.bin"
"done: $dest"
