"""Word-level training pairs from the Mozc OSS dictionary and SudachiDict (core + notcore).

  python build_dict_pairs.py DICT_DIR -o dict_pairs.jsonl [--seed 1]

Rows in the zenz-v2.5-dataset shape: {"input": katakana, "output": surface, "left_context": null}.
Kept: not a rare entry (Mozc cost, Sudachi proper nouns by cost and length), the reading is kana only, the surface is 1-24 chars, is not just the reading in kana, and
is not a symbol / whitespace entry. Duplicates by (reading, surface) collapse; the file is shuffled."""
import argparse, csv, glob, json, os, random, re, sys, unicodedata

ap = argparse.ArgumentParser()
ap.add_argument("dict_dir")
ap.add_argument("-o", "--out", required=True)
ap.add_argument("--seed", type=int, default=1)
ap.add_argument("--mozc-cost", type=int, default=9500, help="Mozc entries costlier than this (rarer) are dropped")
ap.add_argument("--proper-cost", type=int, default=20000, help="same for Sudachi proper nouns, which are mostly rare names/places")
a = ap.parse_args()
csv.field_size_limit(10**7)

HIRA = re.compile(r"^[ぁ-ゖー]+$")
KATA = re.compile(r"^[ァ-ヺー]+$")
ESC = re.compile(r"\\u([0-9a-fA-F]{4})")


def to_kata(s):
    return "".join(chr(ord(c) + 0x60) if "ぁ" <= c <= "ゖ" else c for c in s)


def unescape(s):
    return ESC.sub(lambda m: chr(int(m.group(1), 16)), s)


def ok_surface(s):
    if not (1 <= len(s) <= 24) or s.strip() != s or "　" in s:
        return False
    return not all(unicodedata.category(c)[0] in "PSZC" for c in s)


pairs = {}          # (reading, surface) -> source tag
n_in = 0
for p in sorted(glob.glob(os.path.join(a.dict_dir, "mozc_dictionary*.txt"))):
    for l in open(p, encoding="utf-8"):
        f = l.rstrip("\n").split("\t")
        if len(f) < 5:
            continue
        n_in += 1
        r, s = f[0], f[4]
        if int(f[3]) > a.mozc_cost:
            continue
        if HIRA.match(r) and ok_surface(s) and to_kata(r) != to_kata(s):
            pairs.setdefault((to_kata(r), s), "mozc")
for p in sorted(glob.glob(os.path.join(a.dict_dir, "*_lex.csv"))):
    for f in csv.reader(open(p, encoding="utf-8", newline="")):
        if len(f) < 13:
            continue
        n_in += 1
        s, pos, r = unescape(f[4]), f[5], f[11]
        if pos in ("記号", "補助記号", "空白") or not KATA.match(r):
            continue
        if f[6] == "固有名詞" and (int(f[3]) > a.proper_cost or len(s) > 8):
            continue
        if ok_surface(s) and r != to_kata(s):
            pairs.setdefault((r, s), "sudachi")
rows = list(pairs.items())
random.Random(a.seed).shuffle(rows)
with open(a.out, "w", encoding="utf-8", newline="\n") as f:
    for (r, s), src in rows:
        f.write(json.dumps({"input": r, "output": s, "left_context": None}, ensure_ascii=False) + "\n")
latin = sum(1 for (r, s), _ in rows if re.search(r"[A-Za-z]", s))
print(f"{n_in} dictionary lines -> {len(rows)} pairs ({sum(1 for _, v in rows if v == 'mozc')} mozc, "
      f"{sum(1 for _, v in rows if v == 'sudachi')} sudachi), {latin} with latin letters -> {a.out}")
