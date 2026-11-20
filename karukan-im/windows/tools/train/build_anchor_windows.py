"""Cuts short, correctly aligned training windows out of ordinary rows.

A digit run or latin word is typed and shown the same way (`25` / `GitHub`), so it anchors the
input to the output: the text between two anchors has an exact reading. A window is one anchor and
the text after it ("9ジ" -> "9時", "GitHubニ" -> "GitHubに"), the shape of what the converter sees
while typing, with the output before it as left context; `before` windows add the short text in
front. These are the short digit/counter and latin+kana spans the long rows teach too rarely.

  python build_anchor_windows.py IN.jsonl OUT.jsonl [--procs 4] [--max-in 24]"""
import argparse, json, re, sys, hashlib
from multiprocessing import Pool

ANCHOR = re.compile(r"[A-Za-z0-9][A-Za-z0-9.,'’\-%]*")
MAXIN = 24

def split(s):
    parts, anchors, pos = [], [], 0
    for m in ANCHOR.finditer(s):
        parts.append(s[pos:m.start()]); anchors.append(m.group()); pos = m.end()
    parts.append(s[pos:])
    return parts, anchors

def windows(line):
    try:
        j = json.loads(line)
        inp, out, ctx = j["input"], j["output"], j.get("left_context") or ""
    except Exception:
        return []
    pi, ai = split(inp); po, ao = split(out)
    if not ai or ai != ao or len(pi) != len(po):
        return []
    res = []
    for k, a in enumerate(ai):
        tin, tout = pi[k + 1], po[k + 1]
        if not tin or len(a) + len(tin) > MAXIN:
            continue
        left_out = ctx + "".join(po[i] + ao[i] for i in range(k)) + po[0 if k == 0 else k]
        if k == 0:
            left_out = ctx + po[0]
        else:
            left_out = ctx + "".join(po[i] + ao[i] for i in range(k)) + po[k]
        # ctx layout: the text before the anchor is part of the left context; the window starts at the anchor
        res.append({"input": a + tin, "output": a + tout, "left_context": left_out[-60:] or None})
        # with the short text in front, when there is one
        bin_, bout = pi[k], po[k]
        if bin_ and len(bin_) <= 8 and len(bin_) + len(a) + len(tin) <= MAXIN:
            lc = (ctx + "".join(po[i] + ao[i] for i in range(k)))[-60:] or None if k else (ctx[-60:] or None)
            res.append({"input": bin_ + a + tin, "output": bout + a + tout, "left_context": lc})
    return [json.dumps(r, ensure_ascii=False) for r in res]

def work(batch):
    out = []
    for l in batch:
        out.extend(windows(l))
    return out

def batches(f, n=4000):
    b = []
    for l in f:
        b.append(l)
        if len(b) >= n:
            yield b; b = []
    if b: yield b

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("inp"); ap.add_argument("out")
    ap.add_argument("--procs", type=int, default=4); ap.add_argument("--max-in", type=int, default=24)
    a = ap.parse_args(); MAXIN = a.max_in
    seen, n_in, n_out = set(), 0, 0
    with Pool(a.procs) as pool, open(a.inp, encoding="utf-8") as f, open(a.out, "w", encoding="utf-8") as fo:
        for res in pool.imap(work, batches(f)):
            n_in += 1
            for l in res:
                r = json.loads(l)
                h = hash((r["input"], r["output"], (r["left_context"] or "")[-6:]))
                if h in seen: continue
                seen.add(h); fo.write(l + "\n"); n_out += 1
    print(a.inp, "batches", n_in, "windows", n_out, flush=True)
