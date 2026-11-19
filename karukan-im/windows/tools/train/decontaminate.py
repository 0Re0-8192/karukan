"""Drops training rows that contain a stretch of an evaluation sentence.

  python decontaminate.py ITEMS.json IN.jsonl OUT.jsonl [--n 10]

A row goes when its output (NFKC) shares any N-char stretch with an expected output of the
benchmark (AJIMEE-Bench). Wikipedia-derived sets like JWTD, the source of AJIMEE's sentences,
are in the Wikipedia training data, so the score would otherwise partly measure memory."""
import argparse, json, unicodedata
from multiprocessing import Pool

ap = argparse.ArgumentParser()
ap.add_argument("items")
ap.add_argument("inp")
ap.add_argument("out")
ap.add_argument("--n", type=int, default=10)
ap.add_argument("--procs", type=int, default=8)
a = ap.parse_args()
nfkc = lambda s: unicodedata.normalize("NFKC", s)

GRAMS = set()
for it in json.load(open(a.items, encoding="utf-8")):
    for e in it["expected_output"] + [it.get("original_text") or ""]:
        e = nfkc(e)
        GRAMS.update(e[i:i + a.n] for i in range(len(e) - a.n + 1))


def bad(line):
    try:
        o = nfkc(json.loads(line)["output"])
    except Exception:
        return True
    return any(o[i:i + a.n] in GRAMS for i in range(len(o) - a.n + 1))


def work(lines):
    return [(l, bad(l)) for l in lines]


def batches(f, size=2000):
    b = []
    for l in f:
        b.append(l)
        if len(b) >= size:
            yield b
            b = []
    if b:
        yield b


if __name__ == "__main__":
    kept = dropped = 0
    with open(a.inp, encoding="utf-8") as f, open(a.out, "w", encoding="utf-8", newline="\n") as o, Pool(a.procs) as p:
        for res in p.imap(work, batches(f), chunksize=1):
            for l, b in res:
                if b:
                    dropped += 1
                else:
                    o.write(l)
                    kept += 1
    print(f"kept {kept}, dropped {dropped} -> {a.out}")
