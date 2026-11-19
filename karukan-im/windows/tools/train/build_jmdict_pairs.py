"""Word pairs from JMdict (jukugo, yojijukugo, idioms): every kanji spelling x every reading of an entry.

  python build_jmdict_pairs.py JMdict_e.gz -o jmdict_pairs.jsonl [--seed 1]

Rows: {"input": katakana, "output": spelling, "left_context": null}. Spellings tagged as outdated (oK),
irregular (iK) or rarely used (rK), spellings of 1 char only and readings that are not kana are dropped."""
import argparse, gzip, json, random, re
import xml.etree.ElementTree as ET

ap = argparse.ArgumentParser()
ap.add_argument("src")
ap.add_argument("-o", "--out", required=True)
ap.add_argument("--seed", type=int, default=1)
a = ap.parse_args()

HIRA = re.compile(r"^[ぁ-ゖー]+$")
KATA = re.compile(r"^[ァ-ヺー]+$")


def to_kata(s):
    return "".join(chr(ord(c) + 0x60) if "ぁ" <= c <= "ゖ" else c for c in s)


pairs = set()
with gzip.open(a.src, "rb") as f:
    for _, e in ET.iterparse(f, events=("end",)):
        if e.tag != "entry":
            continue
        kebs = [k.findtext("keb") for k in e.findall("k_ele")
                if not ({i.text for i in k.findall("ke_inf")} & {"&oK;", "&iK;", "&rK;", "oK", "iK", "rK"})]
        rebs = [r.findtext("reb") for r in e.findall("r_ele")]
        for k in kebs:
            if not k or len(k) < 2:
                continue
            for r in rebs:
                if r and (HIRA.match(r) or KATA.match(r)):
                    rk = to_kata(r)
                    if rk != to_kata(k):
                        pairs.add((rk, k))
        e.clear()
rows = sorted(pairs)
random.Random(a.seed).shuffle(rows)
with open(a.out, "w", encoding="utf-8", newline="\n") as f:
    for r, k in rows:
        f.write(json.dumps({"input": r, "output": k, "left_context": None}, ensure_ascii=False) + "\n")
print(f"{len(rows)} pairs -> {a.out}")
