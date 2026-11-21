# karukan を使えるようにする。このフォルダの場所で登録するので、フォルダを動かしたら uninstall してから、もう一度これを実行する
$ErrorActionPreference = 'Stop'
$dir = $PSScriptRoot
$dll = Join-Path $dir 'karukan_tsf.dll'
$Clsid = '{4B9F6E5A-1D3C-4C7E-9A52-8E21F0C7A311}'; $Profile = '{A1C2E3D4-5F60-4718-9B2A-3C4D5E6F7081}'
# ネットから落とした zip の中身は読み込みを止められることがあるので、印を外す
Get-ChildItem $dir -Recurse -File | Unblock-File
# 1. COM と入力方式の登録(管理者の確認が 1 回出る)
$p = Start-Process regsvr32.exe -ArgumentList '/s', "`"$dll`"" -Verb RunAs -Wait -PassThru
if ($p.ExitCode -ne 0) { throw "regsvr32 が失敗しました(終了コード $($p.ExitCode))" }
# 2. このユーザーの入力方式の一覧に入れる
Add-Type -TypeDefinition @"
using System.Runtime.InteropServices;
public static class KarukanInputDll {
  [DllImport("input.dll", CharSet = CharSet.Unicode)]
  public static extern bool InstallLayoutOrTip(string psz, uint dwFlags);
}
"@
if (-not [KarukanInputDll]::InstallLayoutOrTip("0411:$Clsid$Profile", 0)) { throw '入力方式の一覧に入れられませんでした' }
"karukan を入れました。Win+Space で選べます。"