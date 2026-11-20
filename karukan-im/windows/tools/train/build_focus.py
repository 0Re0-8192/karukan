"""Pulls the rows that teach what the model is weakest at out of an existing training file:
digits followed by a counter / unit (9時, 100円, 20歳, 1.2倍) and Japanese text with latin words
(GitHubにプッシュする). Used to weight those rows up in the next run.

  python build_focus.py IN.jsonl OUT_NUM.jsonl OUT_ENG.jsonl [--procs 4]"""
import argparse, json, re, sys
from multiprocessing import Pool

NUM = re.compile(r"[0-9][0-9.,]*(?:年|月|日|時|分|秒|円|歳|才|人|個|回|倍|本|台|匹|枚|冊|件|点|位|番|階|週間|時間|分間|年間|ヶ月|か月|ヵ月|%|パーセント|キロ|メートル|センチ|グラム|リットル|度|割|代|世紀|万|億|千|百)")
ENG = re.compile(r"[A-Za-z]{2,}")
JP = re.compile(r"[぀-ヿ一-鿿]")

def classify(line):
    try:
        j = json.loads(line)
    except Exception:
        return 0
    o = j.get("output") or ""
    if len(o) > 100:
        return 0
    if NUM.search(o):
        return 1
    if ENG.search(o) and JP.search(o) and ENG.search(j.get("input") or ""):   # the latin word is typed as such
        return 2
    return 0

def work(batch):
    return [(l, classify(l)) for l in batch]

def batches(f, n=5000):
    b = []
    for l in f:
        b.append(l)
        if len(b) >= n:
            yield b
            b = []
    if b:
        yield b

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("inp"); ap.add_argument("num"); ap.add_argument("eng")
    ap.add_argument("--procs", type=int, default=4)
    a = ap.parse_args()
    n = [0, 0, 0]
    with Pool(a.procs) as pool, open(a.inp, encoding="utf-8") as f, \
         open(a.num, "a", encoding="utf-8") as fn, open(a.eng, "a", encoding="utf-8") as fe:
        for res in pool.imap(work, batches(f)):
            for l, c in res:
                n[c] += 1
                if c == 1: fn.write(l if l.endswith("\n") else l + "\n")
                elif c == 2: fe.write(l if l.endswith("\n") else l + "\n")
    print(a.inp, "rows", sum(n), "num", n[1], "eng", n[2], flush=True)
