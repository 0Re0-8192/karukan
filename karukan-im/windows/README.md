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
cargo build --release -p karukan-im --bin karukan-imserver
cargo build --release -p karukan-tsf
```

DLL と `karukan-imserver.exe` を同じフォルダに置きます。`portable.flag` を置くと、設定・学習・モデルをそのフォルダ内(`data`・`config`・`hf`)に保存します。

## 登録

管理者権限で `regsvr32 karukan_tsf.dll`(解除は `/u`)。64 ビットのアプリでのみ使えます。

## 英単語混じり

英単語をローマ字のまま打っても（`windowsdeha`）、変換候補の先頭付近に `windowsでは` が出ます。単語は同梱の頻出語リスト（`karukan-im/core/data/english_words.txt`）から、3 文字以上で、打鍵の半分以上が英語になる場合だけ採用します。

### 訂正からの自動学習

F9/F10 で英語にした語・Shift で打った語は、次の打鍵から自動で英単語として扱います（`user_english.txt`）。別の候補を選んだ記録は `corrections.tsv`（ローカルのみ、30 日で消える）に残り、PC が空いているときに `tools\curate_run.ps1` がローカルモデル（llama-server、CPU のみ、Gemma 4 26B-A4B）で固有名詞だけ選び、`user_dicts\auto.txt` と `user_english.txt` に追記します。登録は `tools\install_curate_task.ps1`（4 時間ごと、10 分操作が無いときだけ、ログは `data\curate.log`）。外部には何も送りません。
