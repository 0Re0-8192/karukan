"""Writes a small hand-made evaluation set for digits, latin words and context-dependent readings,
in the AJIMEE-Bench item format (index / context_text / input / expected_output).

  python build_mixed_eval.py OUT.json

Items of the `num` and `eng` groups are written once as a whole reading ("1.2バイ" -> "1.2倍") and
produced in two layouts: `ride` (the digits / latin letters are part of the model input, as with
chunk_digits / chunk_alphabets > 0) and `ctx` (they are already typed text to the left, and the
model converts only the Japanese part, as with the default chunking). `ctx` groups are homophones
that only the text on the left can tell apart."""
import json, sys, re

NUM = [  # reading, outputs
    ("1.2バイ", ["1.2倍"]), ("3ガツ15ニチ", ["3月15日"]), ("2024ネン", ["2024年"]), ("10コ", ["10個"]),
    ("5ニン", ["5人"]), ("3カイメ", ["3回目"]), ("100エン", ["100円"]), ("1,000エン", ["1,000円"]),
    ("50パーセント", ["50%", "50パーセント"]), ("3.5キロ", ["3.5キロ", "3.5kg"]), ("12ジ30プン", ["12時30分"]),
    ("9ジ", ["9時"]), ("2ヒキ", ["2匹"]), ("3ビョウ", ["3秒"]), ("20サイ", ["20歳"]), ("1バンメ", ["1番目"]),
    ("5ヘイホウメートル", ["5平方メートル", "5㎡"]), ("3ダイ", ["3台"]), ("0.5バイ", ["0.5倍"]),
    ("8ジカン", ["8時間"]), ("30プンゴ", ["30分後"]), ("3ネンカン", ["3年間"]), ("1シュウカン", ["1週間"]),
    ("2ケタ", ["2桁"]), ("3ボン", ["3本"]), ("4ホン", ["4本"]), ("100メートル", ["100メートル", "100m"]),
    ("ダイ3ショウ", ["第3章"]), ("ワタシガ25サイダ", ["私が25歳だ"]), ("ケッカガ1.2バイニナッタ", ["結果が1.2倍になった"]),
    ("2ガツ1ニチ", ["2月1日"]), ("7ニンノコドモ", ["7人の子供", "7人の子ども"]), ("16ビット", ["16ビット"]),
    ("3.14ト", ["3.14と"]), ("ダイ2カイ", ["第2回"]), ("25ネンマエ", ["25年前"]), ("1000ネンゴ", ["1000年後"]),
]
ENG = [
    ("Rustデカイタ", ["Rustで書いた"]), ("iPhoneヲカウ", ["iPhoneを買う"]), ("Windowsノコウシン", ["Windowsの更新"]),
    ("GitHubニプッシュスル", ["GitHubにプッシュする"]), ("AIノケンキュウ", ["AIの研究"]), ("PCガコワレタ", ["PCが壊れた"]),
    ("USBメモリヲサス", ["USBメモリを挿す", "USBメモリをさす", "USBメモリを刺す"]),
    ("WiFiニツナガラナイ", ["WiFiに繋がらない", "WiFiにつながらない", "Wi-Fiに繋がらない", "Wi-Fiにつながらない"]),
    ("Excelデシュウケイスル", ["Excelで集計する"]), ("YouTubeデミタ", ["YouTubeで見た", "YouTubeで観た"]),
    ("ChatGPTニキイタ", ["ChatGPTに聞いた", "ChatGPTに訊いた"]), ("Tシャツヲキル", ["Tシャツを着る"]),
    ("SNSデシェアスル", ["SNSでシェアする"]), ("CDヲキク", ["CDを聞く", "CDを聴く"]), ("DVDガミエナイ", ["DVDが見えない"]),
    ("NHKノニュース", ["NHKのニュース"]), ("JRデイク", ["JRで行く"]), ("ABテストヲオコナウ", ["ABテストを行う", "ABテストを行なう"]),
    ("GPUノメモリ", ["GPUのメモリ"]), ("LINEデレンラクスル", ["LINEで連絡する"]),
]
CTX = [  # left context, reading, outputs
    ("ごはんを食べるときに使う", "ハシ", ["箸"]), ("川にかかる", "ハシ", ["橋"]), ("道の", "ハシ", ["端"]),
    ("手紙を書く", "カミ", ["紙"]), ("神社で祈る", "カミ", ["神"]), ("美容院で", "カミヲキル", ["髪を切る"]),
    ("工場の", "キカイ", ["機械"]), ("チャンスの", "キカイ", ["機会"]),
    ("夕方に散歩した", "コウエン", ["公園"]), ("大学で先生の", "コウエンヲキク", ["講演を聞く", "講演を聴く"]),
    ("努力の", "セイカ", ["成果"]), ("オリンピックの", "セイカ", ["聖火"]), ("八百屋で売る", "セイカ", ["青果"]),
    ("河原の", "イシ", ["石"]), ("病院の", "イシ", ["医師"]), ("固い", "イシ", ["意志"]),
    ("庭に咲く", "ハナ", ["花"]), ("風邪をひいて", "ハナガツマル", ["鼻が詰まる"]),
    ("夏は", "アツイ", ["暑い"]), ("お茶が", "アツイ", ["熱い"]), ("この本は", "アツイ", ["厚い"]),
    ("朝が", "ハヤイ", ["早い"]), ("足が", "ハヤイ", ["速い"]),
    ("映画を", "ミル", ["観る", "見る"]), ("医者が患者を", "ミル", ["診る", "見る"]),
    ("美術館で絵を", "カンショウスル", ["鑑賞する"]), ("他人の生活に", "カンショウスル", ["干渉する"]),
    ("品質を", "ホショウスル", ["保証する"]), ("社会", "ホショウ", ["保障"]), ("損害の", "ホショウ", ["補償"]),
    ("調査の", "タイショウ", ["対象"]), ("左右", "タイショウ", ["対称"]),
    ("新しい法律が", "シコウサレル", ["施行される"]), ("論理的な", "シコウ", ["思考"]),
    ("鉛筆を2", "ホンカウ", ["本買う"]), ("アジアの", "ニホン", ["日本"]),
    ("明日の天気は", "ハレ", ["晴れ"]), ("お祝いの", "ハレ", ["晴れ"]),
    ("駅まで", "アルク", ["歩く"]), ("壁に絵を", "カケル", ["掛ける"]), ("宿題を", "ダス", ["出す"]),
    ("会議の資料を", "サクセイスル", ["作成する"]), ("風が", "フク", ["吹く"]), ("服を", "フク", ["拭く", "吹く"]),
]
items = []
def add(group, ctx, reading, outs, lay):
    items.append({"index": f"{group}-{lay}-{len(items)}", "group": group, "layout": lay,
                  "context_text": ctx, "input": reading, "expected_output": outs})
for group, rows in (("num", NUM), ("eng", ENG)):
    for reading, outs in rows:
        add(group, "", reading, outs, "ride")
        m = re.match(r"^[0-9A-Za-z.,]+", reading)   # leading digits / latin letters
        if m and all(o.startswith(m.group()) for o in outs):
            add(group, m.group(), reading[m.end():], [o[m.end():] for o in outs], "ctx")
for ctx, reading, outs in CTX:
    add("ctx", ctx, reading, outs, "ctx")
    add("ctx", "", reading, outs, "noctx")
json.dump(items, open(sys.argv[1], "w", encoding="utf-8"), ensure_ascii=False, indent=1)
print(len(items), "items")
