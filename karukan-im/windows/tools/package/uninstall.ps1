# karukan を外す(このフォルダは消さない。学習と設定は data / config に残る)
$ErrorActionPreference = 'Stop'
$dll = Join-Path $PSScriptRoot 'karukan_tsf.dll'
$Clsid = '{4B9F6E5A-1D3C-4C7E-9A52-8E21F0C7A311}'; $Profile = '{A1C2E3D4-5F60-4718-9B2A-3C4D5E6F7081}'
Add-Type -TypeDefinition @"
using System.Runtime.InteropServices;
public static class KarukanInputDll {
  [DllImport("input.dll", CharSet = CharSet.Unicode)]
  public static extern bool InstallLayoutOrTip(string psz, uint dwFlags);
}
"@
[void][KarukanInputDll]::InstallLayoutOrTip("0411:$Clsid$Profile", 1)  # 1 = 一覧から外す
$p = Start-Process regsvr32.exe -ArgumentList '/u', '/s', "`"$dll`"" -Verb RunAs -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "regsvr32 /u が失敗しました(終了コード $($p.ExitCode))" }
"karukan を外しました。このフォルダは消して構いません(開いているアプリを閉じてから)。"