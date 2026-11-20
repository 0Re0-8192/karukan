"""Keeps the windows whose reading -> writing pattern is the usual one.

The anchor (digits / latin word) is replaced by `#`, giving a pattern like `#ジ` -> `#時`. A window
stays when its pattern was seen at least --min times and its output is at least --share of all the
outputs seen for the same input pattern, which drops the source's reading slips (`1バン` -> `1万`).

  python filter_templates.py IN.jsonl OUT.jsonl [--min 5] [--share 0.5]"""
import argparse, json, re, collections
ANCHOR = re.compile(r"[A-Za-z0-9][A-Za-z0-9.,'’\-%]*")
ap = argparse.ArgumentParser()
ap.add_argument("inp"); ap.add_argument("out")
ap.add_argument("--min", type=int, default=5); ap.add_argument("--share", type=float, default=0.5)
a = ap.parse_args()
pat = lambda s: ANCHOR.sub("#", s)
both, inn = collections.Counter(), collections.Counter()
for l in open(a.inp, encoding="utf-8"):
    r = json.loads(l); pi = pat(r["input"]); both[(pi, pat(r["output"]))] += 1; inn[pi] += 1
kept = total = 0
with open(a.out, "w", encoding="utf-8") as fo:
    for l in open(a.inp, encoding="utf-8"):
        r = json.loads(l); total += 1
        pi, po = pat(r["input"]), pat(r["output"])
        c = both[(pi, po)]
        if c >= a.min and c / inn[pi] >= a.share:
            fo.write(l); kept += 1
print(a.inp, "kept", kept, "of", total)
