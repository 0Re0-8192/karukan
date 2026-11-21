"""Homophone evaluation set: candidate lists for authoring, validation/tagging/freezing, blind-review sheets.

  python homo_eval.py cands --counts counts.pkl READING...            # surfaces of each reading by corpus count
  python homo_eval.py sheet RAW.jsonl... -o review.jsonl --seed 1     # blind sheet: context, reading, shuffled candidates
  python homo_eval.py check RAW.jsonl... -o items.json --dict dict_all.jsonl --counts counts.pkl \
      [--review picks.jsonl ...] [--holdout run7=FILE ...] [--avoid TRAIN.jsonl ...]

Raw rows (one JSON per line, written by the authors):
  {"id": "a-0001", "reading": "いし", "context_text": "...", "input": "イシ", "expected_output": ["意志"], "rivals": ["石", "医師", "意思"]}
`expected_output` lists every acceptable spelling (kana forms, 見る/観る); `rivals` the other surfaces of the same
reading that the context rules out. `check` drops a row when the input is not katakana, a key or rival is unknown to
the dictionary and the corpus counts under that reading, a key is also a rival, the (context, input) pair repeats, the
blind reviewer picked a rival or none of the keys, or context+answer shares a 10-char stretch with a training row.
Every kept item is tagged: `sense` major/minor (is the key the most frequent kanji surface among key+rivals in the
corpus counts), `held_<run>` (the reading was held out of that run's homophone rows). The output is written with
its sha256 so the set can be frozen before any model is scored."""
import argparse, hashlib, json, pickle, random, re, sys, unicodedata
from collections import Counter, defaultdict

nfkc = lambda s: unicodedata.normalize("NFKC", s)
KATA = re.compile(r"^[ァ-ヴー]+$")
KANJI = re.compile(r"[一-鿿々]")


def kata(s):
    return "".join(chr(ord(c) + 0x60) if "ぁ" <= c <= "ゖ" else c for c in s)


def load_counts(p):
    by = defaultdict(dict)
    for (r, s), n in pickle.load(open(p, "rb")).items():
        by[r][s] = n
    return by


def rows(paths):
    for p in paths:
        for n, l in enumerate(open(p, encoding="utf-8"), 1):
            l = l.strip()
            if not l:
                continue
            try:
                yield p, json.loads(l)
            except Exception as e:
                print(f"skip {p}:{n}: {e}", file=sys.stderr)


def cmd_cands(a):
    by = load_counts(a.counts)
    for r in a.readings:
        k = kata(r)
        c = sorted(by.get(k, {}).items(), key=lambda x: -x[1])
        tot = sum(n for _, n in c) or 1
        keep = [f"{s}:{n}" for s, n in c if n >= a.min and n / tot >= a.share][: a.top]
        print(r, " ".join(keep))


def cmd_sheet(a):
    rnd = random.Random(a.seed)
    with open(a.out, "w", encoding="utf-8") as f:
        for _, j in rows(a.raw):
            c = list(dict.fromkeys(j["expected_output"] + j["rivals"]))
            rnd.shuffle(c)
            f.write(json.dumps({"id": j["id"], "context_text": j["context_text"], "input": j["input"], "candidates": c}, ensure_ascii=False) + "\n")


def grams(s, n):
    return {s[i:i + n] for i in range(len(s) - n + 1)}


def cmd_check(a):
    dict_pairs = defaultdict(set)
    for l in open(a.dict, encoding="utf-8"):
        try:
            j = json.loads(l)
        except Exception:
            continue
        dict_pairs[j["input"]].add(j["output"])
    by = load_counts(a.counts)
    picks = defaultdict(list)  # id -> one pick set per reviewer; any disagreement drops the item
    for p in a.review:
        for _, j in rows([p]):
            picks[j["id"]].append(set(j["picks"]))
    held = {}
    for spec in a.holdout:
        run, path = spec.split("=", 1)
        held[run] = {kata(l.strip()) for l in open(path, encoding="utf-8-sig") if l.strip()}

    raw = list(rows(a.raw))
    # contamination grams: context + first key of every candidate item
    want = defaultdict(list)
    for i, (_, j) in enumerate(raw):
        for g in grams(nfkc(j["context_text"] + j["expected_output"][0]), a.n):
            want[g].append(i)
    hit = set()
    for p in a.avoid:
        for l in open(p, encoding="utf-8"):
            try:
                t = json.loads(l)
            except Exception:
                continue
            o = nfkc((t.get("left_context") or "") + (t.get("output") or ""))
            for g in grams(o, a.n):
                if g in want:
                    hit.update(want[g])

    def known(reading, s):
        if not KANJI.search(s):
            return True  # kana spellings need no lookup
        return s in dict_pairs.get(reading, ()) or s in by.get(reading, {})

    out, why, seen = [], Counter(), set()
    for i, (src, j) in enumerate(raw):
        inp, ctx, keys, riv = j["input"], j["context_text"], j["expected_output"], j["rivals"]
        bad = None
        extended = bool(j.get("reading")) and inp != kata(j["reading"])
        if not KATA.match(inp):
            bad = "input not katakana"
        elif not ctx or len(ctx) > a.max_ctx:
            bad = "context empty/too long"
        elif not keys or not riv or set(keys) & set(riv):
            bad = "keys/rivals"
        elif not extended and not all(known(inp, s) for s in keys + riv):  # phrases are not in the dictionary
            bad = "unknown surface"
        elif (ctx, inp) in seen:
            bad = "duplicate"
        elif i in hit:
            bad = "overlaps training"
        elif any(not (p & set(keys)) or p & set(riv) for p in picks.get(j["id"], [])):
            bad = "review disagrees"
        elif a.min_reviews and len(picks.get(j["id"], [])) < a.min_reviews:
            bad = "not reviewed"
        if bad:
            why[bad] += 1
            if a.verbose:
                print(f"DROP {bad}: {j['id']} {ctx}|{inp} -> {keys} / {riv}", file=sys.stderr)
            continue
        seen.add((ctx, inp))
        cnt = by.get(inp, {})
        kanji_all = [s for s in keys + riv if KANJI.search(s)]
        top = max(kanji_all, key=lambda s: cnt.get(s, 0)) if kanji_all else None
        it = dict(j)
        it.update({"index": j["id"], "group": "homo", "layout": "ctx",
                   "sense": "major" if top in keys else "minor",
                   # the input runs past the reading (キセイセン for きせい): the rivals often stop being words, so the
                   # input alone settles it
                   "extended": extended,
                   "cue_far": bool(j.get("cue_far")),
                   "key_count": max(cnt.get(s, 0) for s in keys), "top_rival_count": max(cnt.get(s, 0) for s in riv)})
        for run, rs in held.items():
            it["held_" + run] = inp in rs or kata(j.get("reading", "")) in rs
        out.append(it)
    data = json.dumps(out, ensure_ascii=False, indent=1)
    open(a.out, "w", encoding="utf-8").write(data)
    h = hashlib.sha256(data.encode("utf-8")).hexdigest()
    open(a.out + ".sha256", "w").write(h + "\n")
    tags = Counter(it["sense"] for it in out)
    hk = {run: sum(it["held_" + run] for it in out) for run in held}
    print(f"kept {len(out)} / {len(raw)}  dropped {dict(why)}  sense {dict(tags)}  held {hk}  readings {len({it['input'] for it in out})}  sha256 {h[:16]}")


ap = argparse.ArgumentParser()
sp = ap.add_subparsers(dest="cmd", required=True)
p = sp.add_parser("cands"); p.add_argument("readings", nargs="+"); p.add_argument("--counts", required=True)
p.add_argument("--min", type=int, default=100); p.add_argument("--share", type=float, default=0.005); p.add_argument("--top", type=int, default=8)
p = sp.add_parser("sheet"); p.add_argument("raw", nargs="+"); p.add_argument("-o", "--out", required=True); p.add_argument("--seed", type=int, default=1)
p = sp.add_parser("check"); p.add_argument("raw", nargs="+"); p.add_argument("-o", "--out", required=True)
p.add_argument("--dict", required=True); p.add_argument("--counts", required=True)
p.add_argument("--review", action="append", default=[]); p.add_argument("--min-reviews", type=int, default=0)
p.add_argument("--holdout", action="append", default=[])
p.add_argument("--avoid", action="append", default=[]); p.add_argument("--n", type=int, default=10)
p.add_argument("--max-ctx", type=int, default=30); p.add_argument("--verbose", action="store_true")
a = ap.parse_args()
{"cands": cmd_cands, "sheet": cmd_sheet, "check": cmd_check}[a.cmd](a)
