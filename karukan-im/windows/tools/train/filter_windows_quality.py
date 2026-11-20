"""Second filter for anchor windows: keeps only windows worth training on.

 num: the anchor must be followed by a counter reading that is the right one for its writing
      (#エン→#円, #ジ→#時 ...). The source has reading slips (円 read マル, 万 read バン/ヨロズ),
      which a whitelist drops instead of learning.
 eng: the text after the latin word must contain katakana (a reading to convert), so bare
      `Word ` -> `Word ` rows go.

  python filter_windows_quality.py num|eng IN.jsonl OUT.jsonl"""
import json, re, sys
kind, inp, outp = sys.argv[1:4]
ANCHOR = re.compile(r"[A-Za-z0-9][A-Za-z0-9.,'’\-%]*")
KANA = re.compile(r"[ァ-ヴー]")
COUNTERS = {  # reading -> accepted writings (what the output must start with, after the anchor)
    "エン": ["円"], "マンエン": ["万円"], "マン": ["万"], "センエン": ["千円"], "オク": ["億"], "ジ": ["時"], "ジカン": ["時間"],
    "フン": ["分"], "プン": ["分"], "ビョウ": ["秒"], "ネン": ["年"], "ネンカン": ["年間"], "ネンマエ": ["年前"], "ネンゴ": ["年後"],
    "ガツ": ["月"], "ニチ": ["日"], "ニチカン": ["日間"], "サイ": ["歳", "才"], "ニン": ["人"], "ニンメ": ["人目"], "カイ": ["回", "階"],
    "カイメ": ["回目"], "バイ": ["倍"], "コ": ["個"], "ホン": ["本"], "ボン": ["本"], "ポン": ["本"], "ダイ": ["台", "代"], "ケン": ["件", "軒"],
    "サツ": ["冊"], "マイ": ["枚"], "テン": ["点"], "ヒキ": ["匹"], "ピキ": ["匹"], "ビキ": ["匹"], "トウ": ["頭"], "ワ": ["羽"],
    "バンメ": ["番目"], "バン": ["番"], "バンチ": ["番地"], "ゴウ": ["号"], "カイメ": ["回目"], "カゲツ": ["ヶ月", "か月", "カ月", "ヵ月"],
    "シュウカン": ["週間"], "パーセント": ["%", "パーセント"], "キロ": ["キロ"], "メートル": ["メートル"], "センチ": ["センチ"],
    "グラム": ["グラム"], "リットル": ["リットル"], "ド": ["度"], "ワリ": ["割"], "セイキ": ["世紀"], "ダイメ": ["代目"], "ミリ": ["ミリ"],
    "ヘイホウメートル": ["平方メートル"], "ヘイホウキロメートル": ["平方キロメートル"], "キログラム": ["キログラム"], "ケタ": ["桁"],
    "ブン": ["分"], "ジュウ": ["十"], "ゼン": ["全"],
}
KEYS = sorted(COUNTERS, key=len, reverse=True)
def ok_num(inp_, out_):
    m = ANCHOR.match(inp_); mo = ANCHOR.match(out_)
    if not m or not mo:
        return False
    ri, ro = inp_[m.end():], out_[mo.end():]
    for k in KEYS:
        if ri.startswith(k):
            return any(ro.startswith(w) for w in COUNTERS[k])
    return False
n = kept = 0
with open(outp, "w", encoding="utf-8") as fo:
    for l in open(inp, encoding="utf-8"):
        r = json.loads(l); n += 1
        if kind == "num":
            good = ok_num(r["input"], r["output"])
        else:
            m = ANCHOR.match(r["input"])
            good = bool(m) and bool(KANA.search(r["input"][m.end():]))
        if good:
            fo.write(l); kept += 1
print(kind, "kept", kept, "of", n)
