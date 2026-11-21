"""Pulls rows that teach homophones out of the existing training files.

  python mine_homophones.py --lex dict_pairs.jsonl --in wikipedia.jsonl --in llm-jp.jsonl:10000000 \
      --avoid ime_2000_answers.tsv --holdout 30 --cap 120 -o homophones.jsonl [--procs 24]

A homophone group is a reading with two or more kanji surfaces in the dictionary. Pass 1 counts, over the
input files (optionally only the first N rows of a file: FILE:N), how often each (reading, surface) occurs with
the reading present in the row's input. Pass 2 keeps the groups where at least two surfaces are common
(--min-count) and writes up to --cap rows per (reading, surface): the rarer surface is taken in full, the common
one is capped, so the groups come out balanced and the contrast is learned from the context.

--avoid takes evaluation tables (ime_2000_answers.tsv: reading in column 3, expected sentence in column 5; benchmark_v3 all.tsv by header; or
any jsonl with "output"). Rows sharing a --ngram-char stretch with an expected sentence are dropped, and
--holdout N reading groups of the table are left out of the pool entirely so that the evaluation can also
measure words the training never saw (their readings are written to OUT.holdout.txt).
Rows are in the zenz-v2.5-dataset shape: {"input": katakana, "output": text, "left_context": text|null}."""
import argparse, csv, json, os, random, re, sys, unicodedata
from collections import Counter, defaultdict
from multiprocessing import Pool

ap = argparse.ArgumentParser()
ap.add_argument("--lex", required=True)
ap.add_argument("--in", dest="inputs", action="append", required=True)
ap.add_argument("--avoid", action="append", default=[])
ap.add_argument("--holdout", type=int, default=0)
ap.add_argument("--seed", type=int, default=7)
ap.add_argument("--cap", type=int, default=120)
ap.add_argument("--min-count", type=int, default=30)
ap.add_argument("--ngram-char", type=int, default=10)
ap.add_argument("--min-len", type=int, default=8)
ap.add_argument("--max-len", type=int, default=70)
ap.add_argument("--procs", type=int, default=16)
ap.add_argument("--counts", help="pickle of pass-1 counts: reused when it exists, written when not")
ap.add_argument("-o", "--out", required=True)
a = ap.parse_args()

KANJI = re.compile(r"[一-鿿々]")
nfkc = lambda s: unicodedata.normalize("NFKC", s)
MAXS = 6


def load_lex():
    by_reading = defaultdict(set)
    for l in open(a.lex, encoding="utf-8"):
        try:
            j = json.loads(l)
        except Exception:
            continue
        r, s = j["input"], j["output"]
        if 2 <= len(r) <= 10 and 1 <= len(s) <= MAXS and KANJI.search(s):
            by_reading[r].add(s)
    groups = {r: ss for r, ss in by_reading.items() if len(ss) >= 2}
    surf = defaultdict(list)
    for r, ss in groups.items():
        for s in ss:
            surf[s].append(r)
    return groups, surf


GROUPS = SURF = None


def init():
    global GROUPS, SURF
    GROUPS, SURF = load_lex()


def matches(line):
    try:
        j = json.loads(line)
    except Exception:
        return None, []
    o, i = j.get("output") or "", j.get("input") or ""
    if not (a.min_len <= len(o) <= a.max_len) or "\n" in o:
        return None, []
    found = set()
    n = len(o)
    for p in range(n):
        for q in range(p + 1, min(n, p + MAXS) + 1):
            rs = SURF.get(o[p:q])
            if rs:
                # a clean alignment: the surface stands alone between non-kanji, appears once, and so does the reading
                if (p > 0 and KANJI.match(o[p - 1])) or (q < n and KANJI.match(o[q])) or o.count(o[p:q]) != 1:
                    continue
                for r in rs:
                    if i.count(r) == 1:
                        found.add((r, o[p:q]))
    return j, found


def count_batch(lines):
    c = Counter()
    for l in lines:
        _, f = matches(l)
        c.update(f)
    return c


def pick_batch(args):
    lines, want = args
    out = []
    for l in lines:
        j, f = matches(l)
        hit = [k for k in f if k in want]
        if hit:
            out.append((l, hit))
    return out


def batches(path, limit, n=4000):
    b, seen = [], 0
    with open(path, encoding="utf-8") as f:
        for l in f:
            b.append(l)
            seen += 1
            if len(b) >= n:
                yield b
                b = []
            if limit and seen >= limit:
                break
    if b:
        yield b


def inputs():
    for spec in a.inputs:
        m = re.match(r"^(.*\.jsonl)(?::(\d+))?$", spec)
        yield m.group(1), int(m.group(2)) if m.group(2) else 0


def eval_tables():
    sents, groups = [], defaultdict(set)
    for p in a.avoid:
        if p.endswith(".tsv"):
            rd = list(csv.reader(open(p, encoding="utf-8-sig"), delimiter="\t"))
            if "input_hiragana" in rd[0]:  # benchmark_v3 all.tsv: columns by header
                h = {k: i for i, k in enumerate(rd[0])}
                for r in rd[1:]:
                    sents.append(r[h["expected"]])
                    groups[r[h["reading"]]].add(r[h["target"]])
            else:  # ime_2000_answers.tsv
                for r in rd[1:]:
                    sents.append(r[4])
                    groups[r[2]].add(r[5])
        else:
            for l in open(p, encoding="utf-8"):
                sents.append(json.loads(l)["output"])
    return sents, groups


if __name__ == "__main__":
    sents, evgroups = eval_tables()
    grams = set()
    for s in sents:
        s = nfkc(s)
        grams.update(s[i:i + a.ngram_char] for i in range(len(s) - a.ngram_char + 1))
    kata = lambda s: "".join(chr(ord(c) + 0x60) if "ぁ" <= c <= "ゖ" else c for c in s)
    rng = random.Random(a.seed)
    hold = set(rng.sample(sorted(evgroups), a.holdout)) if a.holdout else set()
    hold_k = {kata(h) for h in hold}
    open(a.out + ".holdout.txt", "w", encoding="utf-8").write("\n".join(sorted(hold)) + "\n")
    print("avoid sentences", len(sents), "grams", len(grams), "holdout groups", len(hold), flush=True)

    import pickle
    total = Counter()
    with Pool(a.procs, initializer=init) as pool:
        if a.counts and os.path.exists(a.counts):
            total = pickle.load(open(a.counts, "rb"))
            print("counts loaded", len(total), flush=True)
        else:
            for p, lim in inputs():
                for c in pool.imap_unordered(count_batch, batches(p, lim)):
                    total.update(c)
                print("counted", p, lim, "pairs", len(total), flush=True)
            if a.counts:
                pickle.dump(total, open(a.counts, "wb"))
        by_r = defaultdict(list)
        for (r, s), n in total.items():
            if n >= a.min_count and r not in hold_k:
                by_r[r].append((s, n))
        want = {(r, s) for r, ss in by_r.items() if len(ss) >= 2 for s, _ in ss}
        print("contested readings", sum(1 for ss in by_r.values() if len(ss) >= 2), "pairs", len(want), flush=True)

        taken = Counter()
        rows, dropped = [], 0
        for p, lim in inputs():
            for res in pool.imap_unordered(pick_batch, ((b, want) for b in batches(p, lim))):
                for l, hit in res:
                    hit = [k for k in hit if taken[k] < a.cap]
                    if not hit:
                        continue
                    o = nfkc(json.loads(l)["output"])
                    if any(o[i:i + a.ngram_char] in grams for i in range(len(o) - a.ngram_char + 1)):
                        dropped += 1
                        continue
                    for k in hit:
                        taken[k] += 1
                    rows.append(l if l.endswith("\n") else l + "\n")
            print("picked", p, "rows", len(rows), "dropped for overlap", dropped, flush=True)
    rng.shuffle(rows)
    with open(a.out + ".report.tsv", "w", encoding="utf-8") as rf:
        per = defaultdict(list)
        for (r, sf), n in taken.items():
            per[r].append((sf, n))
        for r, ss in sorted(per.items()):
            body = " ".join(f"{sf}:{n}" for sf, n in sorted(ss, key=lambda x: -x[1]))
            rf.write(r + chr(9) + body + chr(10))
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.writelines(rows)
    print("wrote", len(rows), "rows,", len(taken), "(reading, surface) pairs ->", a.out)
