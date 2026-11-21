karukan(Windows 版)

入れ方
1. このフォルダを置きたい場所に置く(動かすときは uninstall してから動かし、もう一度 setup)。
2. setup.cmd を実行する(管理者の確認が 1 回出る)。
3. Win+Space で karukan を選ぶ。ネットにつながっていなくても使える。

外し方: uninstall.cmd

学習と設定は、このフォルダの data と config に保存される。設定の項目は
https://github.com/togatoga/karukan の docs/configuration.md を参照。

中身とライセンス(licenses フォルダに全文)
- karukan 本体: MIT / Apache-2.0
- 変換モデル(hf フォルダ): CC BY-SA 4.0
  https://huggingface.co/OReOOreoaaaa/jinen-v2-small-karukan.gguf (元は togatogah/jinen-v2-small.gguf)
  https://huggingface.co/togatogah/jinen-v2-xsmall.gguf
  出典:国立国会図書館「全国書誌データ」および国立国会図書館サーチの書誌データを加工して作成
  (元モデルの作者による加工。国立国会図書館が作成・提供・保証するものではない)
- システム辞書(data\dict.bin): SudachiDict を元に構築、Apache-2.0(licenses\dict)