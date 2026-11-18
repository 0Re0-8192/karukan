"""Adds katakana readings to clean_mine.py output, keeping only segments on which
two independent analysers (pykakasi, MeCab/unidic-lite via fugashi) agree.
Rows in the zenz-v2.5-dataset shape: {"input": katakana, "output": text, "left_context": str|null}.

  python build_mine_pairs.py IN.jsonl -o OUT.jsonl [--rejects REJ.jsonl]"""
import argparse, json, re, unicodedata

import fugashi
import jaconv
import pykakasi

ap = argparse.ArgumentParser()
ap.add_argument("inp")
ap.add_argument("-o", "--out", required=True)
ap.add_argument("--rejects")
ap.add_argument("--left-chars", type=int, default=40)
a = ap.parse_args()

tagger = fugashi.Tagger()
kks = pykakasi.kakasi()
JA = re.compile(r"[぀-ヿ一-鿿]")


def read_mecab(text):
    out = []
    for w in tagger(text):
        s = w.surface
        if JA.search(s):
            k = getattr(w.feature, "kana", None)
            if not k or k == "*":
                if all("぀" <= c <= "ヿ" for c in s):
                    k = jaconv.hira2kata(s)
                else:
                    return None
            out.append(k)
        else:
            out.append(s)
    return "".join(out)


def read_kakasi(text):
    out = []
    for it in kks.convert(text):
        o = it["orig"]
        out.append(it["kana"] if JA.search(o) else o)
    return "".join(out)


def norm(s):
    s = unicodedata.normalize("NFKC", s)
    return s.replace(" ", "").replace("ー", "ー")


kept = rejected = 0
rows = [json.loads(l) for l in open(a.inp, encoding="utf-8")]
with open(a.out, "w", encoding="utf-8", newline="\n") as f, open(a.rejects or "/dev/null" if False else (a.rejects or "NUL"), "w", encoding="utf-8") as rj:
    for r in rows:
        t = r["text"]
        m = read_mecab(t)
        k = read_kakasi(t)
        if m is None or norm(m) != norm(k):
            rejected += 1
            if a.rejects:
                rj.write(json.dumps({"text": t, "mecab": m, "kakasi": k}, ensure_ascii=False) + "\n")
            continue
        left = r.get("left")
        f.write(json.dumps({"input": norm(m), "output": t, "left_context": (left[-a.left_chars:] if left else None)}, ensure_ascii=False) + "\n")
        kept += 1
print(f"kept {kept}, rejected {rejected} ({kept / max(1, kept + rejected):.0%} agree) -> {a.out}")
