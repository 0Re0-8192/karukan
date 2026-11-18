"""Builds ctxset.jsonl: one item = a sentence with one slot (pre + SLOT + post).
ok is the surface a good converter picks; ng lists plausible wrong ones for the same reading.
Scoring compares log P(pre+surface+post) — the model that prefers ok understands the context."""
import json, sys, pathlib

# (pre, reading, ok, [ng], post)
H = """
会議の前に資料を|かくにん|確認|核人|してください。
この病気は早期に|はっけん|発見|発券|することが大切だ。
新幹線の切符を|はっけん|発券|発見|してもらった。
来週の|かいぎ|会議|怪議|は中止になりました。
昨日は早く|ねた|寝た|ネタ|のでまだ眠い。
このブログのための|ねた|ネタ|寝た|を探している。
今朝は6時に|おきた|起きた|沖田|。
彼は新選組の|おきた|沖田|起きた|総司を演じた。
この薬は頭痛に|きいて|効いて|聞いて|いるみたいだ。
先生の話をちゃんと|きいて|聞いて|効いて|いなかった。
グラフの|てん|点|天|を結んで線にする。
|てん|天|点|気予報によると明日は晴れだ。
大学で教員|かてい|課程|家庭|を履修している。
温かい|かてい|家庭|課程|を築きたい。
新しいパソコンを|かう|買う|飼う|予定だ。
庭で犬を|かう|飼う|買う|のが夢だ。
ソースコードを|へんこう|変更|返校|してコミットした。
電車の中で本を|よむ|読む|詠む|のが習慣だ。
俳句を|よむ|詠む|読む|のは難しい。
ご飯を|たべる|食べる|食べ類|前に手を洗う。
アプリを|きどう|起動|軌道|したら画面が真っ暗になった。
人工衛星が|きどう|軌道|起動|に乗った。
彼女は|いし|意思|医師|をはっきり伝えた。
手術をした|いし|医師|意思|に感謝した。
この|せいかく|正確|性格|な数値を記録する。
兄は明るい|せいかく|性格|正確|だ。
駅まで|はしって|走って|橋って|行った。
川に|はし|橋|箸|が架かっている。
ご飯を|はし|箸|橋|で食べる。
空に|くも|雲|蜘蛛|が浮かんでいる。
壁に大きな|くも|蜘蛛|雲|がいた。
今日は|あつい|暑い|熱い|ので窓を開けた。
お茶が|あつい|熱い|暑い|から気をつけて。
本を|かりて|借りて|刈りて|読んだ。
このサービスは月額|むりょう|無料|無料|で使える。
彼の話は|しんじ|信じ|神事|られない。
神社で|しんじ|神事|信じ|が行われた。
|かんせい|完成|歓声|した作品を公開する。
スタジアムに|かんせい|歓声|完成|が響いた。
高い|ビル|ビル|ビール|が立ち並ぶ。
仕事帰りに|びーる|ビール|ビル|を飲んだ。
|こうえん|公園|講演|で子供が遊んでいる。
大学で|こうえん|講演|公園|を聞いた。
駅前で|きょうそう|競争|協奏|が激しい。
ピアノ|きょうそう|協奏|競争|曲を聴いた。
この文章を|こうせい|校正|構成|してから提出する。
チームの|こうせい|構成|校正|を見直す。
彼は|じしん|自信|地震|に満ちている。
昨夜大きな|じしん|地震|自信|があった。
この件は|しやく|使役|試薬|ではなく別の問題だ。
データを|ほぞん|保存|保損|してから閉じる。
彼女は|たいせつ|大切|大雪|な友達だ。
冬の北国は|おおゆき|大雪|大幸|で交通が乱れた。
窓を|しめる|閉める|締める|のを忘れた。
ネクタイを|しめる|締める|閉める|と気が引き締まる。
今日は|はれ|晴れ|腫れ|て気持ちがいい。
虫に刺されて指が|はれ|腫れ|晴れ|た。
先生に|ならう|習う|倣う|ことが多い。
前例に|ならう|倣う|習う|ことにした。
会社の|しゃいん|社員|車員|として働く。
モデルの|せいど|精度|制度|を上げたい。
新しい|せいど|制度|精度|が導入された。
このバグは|さいげん|再現|際限|できない。
要求が|さいげん|際限|再現|なく増える。
画面を|さいこう|再考|最高|してほしい。
今日の天気は|さいこう|最高|再考|だ。
ファイルを|かいとう|解凍|回答|して中身を見る。
質問に|かいとう|回答|解凍|する。
このツールは|べんり|便利|弁理|で手放せない。
|はいる|入る|這入る|前にノックしてください。
結果を|ひょうか|評価|氷菓|してレポートにまとめる。
設定を|さくじょ|削除|作除|して再起動する。
モデルを|がくしゅう|学習|楽習|させる。
変換の|せいど|精度|制度|を測るベンチマークだ。
サーバーに|せつぞく|接続|折続|できない。
キーボードの|はいれつ|配列|排列|を変更した。
|いがい|以外|意外|は全部消してください。
彼が来るとは|いがい|意外|以外|だった。
|かんしん|関心|感心|が高まっている。
彼の努力には|かんしん|感心|関心|した。
この道は|ほしょう|保証|補償|できない。
損害を|ほしょう|補償|保証|してもらった。
お店で商品を|さがす|探す|捜す|のに苦労した。
警察が行方不明者を|さがす|捜す|探す|。
窓から|けしき|景色|気色|を眺める。
""".strip().splitlines()

# host names / words where the dot is not a sentence stop
M = """
詳細は|example.jp|example.jp|エァmpェ。jp|を参照してください。
公式サイトは|www.google.com|www.google.com|ｗｗｗ。google。com|です。
ファイルの|2.16.7|2.16.7|2。16。7|というバージョンを使う。
|0.7|0.7|0。7|という値に設定した。
クラスを|LLM|LLM|エルエルエム|として扱う。
このアプリは|Windows|Windows|ウィンドウズ|で動く。
""".strip().splitlines()

H += open(pathlib.Path(__file__).with_name("more_items.txt"),encoding="utf-8").read().strip().splitlines()
items = []
for line in H + M:
    pre, reading, ok, ng, post = line.split("|", 4) if line.count("|") == 4 else (None,)*5
    if pre is None:
        parts = line.split("|")
        pre, reading, ok, ng, post = parts[0], parts[1], parts[2], parts[3], parts[4] if len(parts) > 4 else ""
    ngs = [x for x in ng.split("/") if x and x != ok]
    if ngs:
        items.append(dict(pre=pre, reading=reading, ok=ok, ng=ngs, post=post))

# real mistakes from corrections.tsv (no context available: whole phrase only)
real = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else None
if real and real.exists():
    skip = {"。jp", "0。", "0。7", "2。16。7"}
    for r in real.read_text(encoding="utf-8").splitlines():
        f = r.split("\t")
        if len(f) >= 6 and f[4] and f[4] != f[5] and f[3] and f[5] not in skip and len(f[5]) > 1:
            items.append(dict(pre="", reading=f[3], ok=f[5], ng=[f[4]], post="", src="corrections"))

out = pathlib.Path(__file__).with_name("ctxset.jsonl")
out.write_text("\n".join(json.dumps(i, ensure_ascii=False) for i in items) + "\n", encoding="utf-8")
print(len(items), "items ->", out)

# hard set: written independently (hard_items.txt), kept apart from ctxset.jsonl
hard = []
for line in pathlib.Path(__file__).with_name("hard_items.txt").read_text(encoding="utf-8").splitlines():
    f = line.split("|")
    if len(f) != 5:
        continue
    pre, reading, ok, ng, post = f
    ngs = [x for x in ng.split("/") if x and x != ok]
    if ngs:
        hard.append(dict(pre=pre, reading=reading, ok=ok, ng=ngs, post=post, src="hard"))
hp = pathlib.Path(__file__).with_name("hardset.jsonl")
hp.write_text("\n".join(json.dumps(i, ensure_ascii=False) for i in hard) + "\n", encoding="utf-8")
print(len(hard), "hard items ->", hp)
