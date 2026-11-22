<div align="center">
  <img src="icon.png" width="128" alt="karukan" />
  <h1>karukan for Windows</h1>
  <p>ニューラルかな漢字変換の日本語入力(TSF)</p>

  [![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)
</div>

[karukan](https://github.com/togatoga/karukan)(Linux・macOS 向け)を Windows に移植したものです。モデルと辞書を同梱しているので、オフラインで使えます。

## 入れ方

1. [Releases](https://github.com/0Re0-8192/karukan-windows/releases) から `karukan-setup-<版>.exe` を取得する。
2. 実行して「入れる」を選ぶ(管理者の確認が 1 回出ます)。
3. Win+Space で karukan を選ぶ。

- 更新: 新しい版の setup を実行して「更新」。学習・設定・ユーザー辞書は残ります。
- 外す: 設定 > アプリ から karukan をアンインストール。
- 64 ビットのアプリでのみ使えます。exe は未署名なので、SmartScreen の警告が出たら「詳細情報」→「実行」。

## Windows 版で足したもの

- MS-IME 準拠のキー(半角/全角・変換・無変換・F6〜F10)。一覧は [karukan-im/windows/README.md](karukan-im/windows/README.md)
- 英単語をローマ字のまま打っても英字の候補が出る(`windowsdeha` → `windowsでは`)
- 改行をまたいだ文脈(前の行を文として左文脈に入れる、最大 25 文字)
- 既定モデル [jinen-v2-small-karukan](https://huggingface.co/OReOOreoaaaa/jinen-v2-small-karukan.gguf): jinen-v2-small を、短い読みを左文脈つきで変換する使い方に合わせて追加学習したもの
- インストーラー(上書き更新、アプリの再起動なしで反映)

設定は [docs/configuration.md](docs/configuration.md)、キーは [docs/key-bindings.md](docs/key-bindings.md)。

## ビルド

```powershell
.\karukan-im\windows\tools\build.ps1
.\karukan-im\windows\tools\package.ps1
```

詳しくは [karukan-im/windows/README.md](karukan-im/windows/README.md)。

## 元のプロジェクト

エンジン・辞書・Linux/macOS 版は [togatoga/karukan](https://github.com/togatoga/karukan) によるものです。元の README は [README.karukan.md](README.karukan.md)。

## ライセンス

- コード: MIT または Apache-2.0
- 変換モデル: CC BY-SA 4.0([jinen-v2-small-karukan](https://huggingface.co/OReOOreoaaaa/jinen-v2-small-karukan.gguf)、[jinen-v2-xsmall](https://huggingface.co/togatogah/jinen-v2-xsmall.gguf))
- システム辞書: SudachiDict をもとに構築、Apache-2.0
