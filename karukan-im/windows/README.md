# karukan-windows (TSF)

Windows 向けフロントエンド。TSF のテキストサービス DLL(`karukan_tsf.dll`)が、子プロセスの `karukan-imserver.exe` とやり取りします(macOS 版と同じ構成)。

## キーバインド

MS-IME に合わせています。

| キー | 動作 |
|------|------|
| 半角/全角 | IME オン/オフ |
| 変換 | 変換中は次候補、未入力ならオン |
| 無変換 | 変換中はカタカナ → 半角カタカナ → ひらがなの巡回、未入力ならオフ |
| ひらがな/カタカナ | ひらがなモードへ |
| F6 / Ctrl+U | ひらがな |
| F7 / Ctrl+I | 全角カタカナ |
| F8 / Ctrl+O | 半角カタカナ |
| F9 / Ctrl+P | 全角英数(連打で小文字 → 大文字 → 先頭大文字) |
| F10 / Ctrl+T | 半角英数(同上) |

その他は [共通キーバインド](../../docs/key-bindings.md) のとおりです。
文節の伸縮(Shift+←→)は未対応です(エンジンに文節の概念がありません)。

## ビルド

```powershell
.\karukan-im\windows\tools\build.ps1
```

`karukan-imserver.exe` と `karukan_tsf.dll` を `D:\cargo-target\karukan\release` に作ります(`-Clean` で llama.cpp も作り直し)。出力先をリポジトリの外にしているのは、`<repo>\target` の下だと、ビルドスクリプトが一時ファイルを作れず `lib.exe` / MSBuild が失敗するためです。llama.cpp は Ninja で作り、`rust-version` が足りなければ `cargo +1.96.0` を使います。

## インストール・更新

```powershell
.\karukan-im\windows\tools\install.ps1
```

ビルド出力の DLL とサーバーを `dist` へ置き(動作中のものは名前を変えて退避)、`portable.flag` を作り、システム辞書 `data\dict.bin` が無ければ取得し、DLL を登録(管理者権限)して、現在のユーザーで有効にします。解除は `-Uninstall`。置き場は `-Dest`、辞書を取らないなら `-SkipDict`。
`portable.flag` があると、設定・学習・モデルをそのフォルダ内(`data`・`config`・`hf`)に保存します。更新した DLL は、アプリを開き直すと読み込まれます。64 ビットのアプリでのみ使えます。

テスト: `KARUKAN_TSF_DEBUG=1 KARUKAN_IMSERVER=<karukan-imserver.exe> cargo test --release -p karukan-tsf --test tsf_e2e -- --nocapture`

## 英単語混じり

英単語をローマ字のまま打っても（`windowsdeha`）、変換候補の先頭付近に `windowsでは` が出ます。単語は同梱の頻出語リスト（`karukan-im/core/data/english_words.txt`）から、3 文字以上で、打鍵の半分以上が英語になる場合だけ採用します。

### 訂正からの自動学習

F9/F10 で英語にした語・Shift で打った語は、次の打鍵から自動で英単語として扱います（`user_english.txt`）。確定した入力はすべて `corrections.tsv`（打ったキー・読み・最初の候補・確定した文字、ローカルのみ、消さずに残す）に記録し、PC が空いているときに `tools\curate_run.ps1` がローカルモデル（llama-server、CPU のみ、検閲なしの Gemma 4 26B-A4B）で固有名詞・よく使う語だけ選び、`user_dicts\auto.txt` と `user_english.txt` に追記します。登録は `tools\install_curate_task.ps1`（4 時間ごと、10 分操作が無いときだけ、ログは `data\curate.log`）。英単語の候補だけ、語そのもの 1 語を Wikipedia / Wiktionary に問い合わせて実在を確かめます（文脈は送らない。`-NoWeb` で無効）。ネットに無い語でも、自分で選んだ語・2 回以上打った語は登録します（1 回だけでネットにも無い語はタイプミスとして見送り）。それ以外は外部に送りません。
