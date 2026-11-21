"""Training rows in the shape the engine converts: the reading of one chunk, with what precedes it as left context.

  python build_split_rows.py punct IN.jsonl[:N] -o OUT.jsonl [--ctx 25] [--seed 1]
  python build_split_rows.py homo  IN.jsonl --groups report.tsv -o OUT.jsonl [--ctx 25]

The source rows (zenz-v2.5 shape: input katakana, output text, left_context) mostly carry the whole sentence as
the input, so whatever settles a homophone sits inside the input. The engine instead converts chunks (and a
user who breaks lines often leaves only the caret's line): the cue sits in the left context and the input is
short. Both modes cut a row in two and move the converted head into left_context (kept to its last --ctx chars,
after any left_context the row had):
  punct  at a 、/。 that the input and the output have the same number of before it (so the cut is aligned),
         chosen at random among those; the tail runs to the next aligned 、/。 or the end
  homo   right before a homophone: a (reading, surface) pair of the groups (mine_homophones.py's report.tsv)
         whose reading occurs once in the input and surface once in the output, standing apart from other
         kanji; the tail runs to the next aligned 、/。 or the end"""
import argparse, json, random, re, sys
from collections import defaultdict

KANJI = re.compile(r"[一-鿿々]")
PUNCT = "、。"


def rows(spec):
    m = re.match(r"^(.*\.jsonl)(?::(\d+))?$", spec)
    path, limit = m.group(1), int(m.group(2) or 0)
    with open(path, encoding="utf-8") as f:
        for n, l in enumerate(f):
            if limit and n >= limit:
                break
            try:
                j = json.loads(l)
            except Exception:
                continue
            if j.get("input") and j.get("output"):
                yield j


def punct_positions(s):
    return [i for i, c in enumerate(s) if c in PUNCT]


def tail_end(i, o, ip, op):
    """End of the tail starting at (ip, op): the next aligned punctuation (inclusive) or the end."""
    pi = [k for k in punct_positions(i) if k >= ip]
    po = [k for k in punct_positions(o) if k >= op]
    if pi and po and i[pi[0]] == o[po[0]]:
        return pi[0] + 1, po[0] + 1
    return len(i), len(o)


def emit(f, j, ip, op, a):
    i, o = j["input"], j["output"]
    # a cut that lands between the same number of 、/。 on both sides; anything else is misaligned
    if [c for c in i[:ip] if c in PUNCT] != [c for c in o[:op] if c in PUNCT]:
        return False
    ie, oe = tail_end(i, o, ip, op)
    inp, out = i[ip:ie], o[op:oe]
    if len(inp) < 2 or not out.strip():
        return False
    ctx = ((j.get("left_context") or "") + o[:op])[-a.ctx:]
    if not ctx.strip():
        return False
    f.write(json.dumps({"input": inp, "output": out, "left_context": ctx}, ensure_ascii=False) + "\n")
    return True


def cmd_punct(a, f):
    rnd = random.Random(a.seed)
    n = 0
    for j in rows(a.inp):
        i, o = j["input"], j["output"]
        pi, po = punct_positions(i), punct_positions(o)
        if not pi or len(pi) != len(po) or any(i[x] != o[y] for x, y in zip(pi, po)):
            continue
        cuts = [(x + 1, y + 1) for x, y in zip(pi, po) if x + 1 < len(i)]
        if not cuts:
            continue
        ip, op = rnd.choice(cuts)
        n += emit(f, j, ip, op, a)
    return n


def cmd_homo(a, f):
    surf = defaultdict(list)  # surface -> readings
    for l in open(a.groups, encoding="utf-8"):
        p = l.rstrip("\n").split("\t")
        if len(p) < 2:
            continue
        for s in p[1].split():
            surf[s.rsplit(":", 1)[0]].append(p[0])
    rnd = random.Random(a.seed)
    n = 0
    for j in rows(a.inp):
        i, o = j["input"], j["output"]
        hits = []
        for p in range(len(o)):
            for q in range(p + 1, min(len(o), p + 6) + 1):
                s = o[p:q]
                if s not in surf or o.count(s) != 1:
                    continue
                if (p > 0 and KANJI.match(o[p - 1])) or (q < len(o) and KANJI.match(o[q])):
                    continue
                for r in surf[s]:
                    if i.count(r) == 1:
                        hits.append((i.index(r), p))
        hits = [h for h in hits if h[1] > 0]
        if not hits:
            continue
        ip, op = rnd.choice(hits)
        n += emit(f, j, ip, op, a)
    return n


ap = argparse.ArgumentParser()
ap.add_argument("mode", choices=["punct", "homo"])
ap.add_argument("inp")
ap.add_argument("-o", "--out", required=True)
ap.add_argument("--groups")
ap.add_argument("--ctx", type=int, default=25)
ap.add_argument("--seed", type=int, default=1)
a = ap.parse_args()
with open(a.out, "w", encoding="utf-8") as f:
    n = cmd_punct(a, f) if a.mode == "punct" else cmd_homo(a, f)
print(f"{a.mode}: wrote {n} rows to {a.out}", file=sys.stderr)
