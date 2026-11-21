"""Variants of a frozen homophone set for the multi-line context question.

  python homo_multiline.py homo1k.json OUTDIR [--seed 1]

A user who breaks lines where others write 、/。 leaves the engine (which keeps only the caret's line) with
almost no context. Each variant rewrites every item's context_text as such a user would type it, then as the
engine could pass it to the model:
  lastline   only the text after the last line break (what the engine does today)
  nl         the lines joined by a raw "\n"
  maru / ten / space   the line breaks replaced by 。 / 、 / a space
  unrel_*    an unrelated line (another item's context) put above, joined the same way: what a far-off
             previous line costs
Only items whose context has a 、/。 to break at are written (the rest would not change)."""
import argparse, json, os, random, re

ap = argparse.ArgumentParser()
ap.add_argument("items"); ap.add_argument("outdir"); ap.add_argument("--seed", type=int, default=1)
a = ap.parse_args()
items = json.load(open(a.items, encoding="utf-8"))
rnd = random.Random(a.seed)
os.makedirs(a.outdir, exist_ok=True)

split = [it for it in items if re.search(r"[、。]", it["context_text"].rstrip("、。"))]
lines_of = {it["index"]: [s for s in re.split(r"[、。]", it["context_text"]) if s] for it in split}
JOIN = {"nl": "\n", "maru": "。", "ten": "、", "space": " "}
variants = {"lastline": lambda ls, o: ls[-1]}
for k, sep in JOIN.items():
    variants[k] = (lambda sep: lambda ls, o: sep.join(ls))(sep)
    variants["unrel_" + k] = (lambda sep: lambda ls, o: sep.join(o + ls))(sep)
variants["unrel_lastline"] = lambda ls, o: ls[-1]  # same as lastline: the unrelated line is cut off
for name, f in variants.items():
    out = []
    for it in split:
        other = rnd.choice(items)
        while other["index"] == it["index"]:
            other = rnd.choice(items)
        o = [s for s in re.split(r"[、。]", other["context_text"]) if s]
        j = dict(it)
        j["context_text"] = f(lines_of[it["index"]], o)
        out.append(j)
    json.dump(out, open(os.path.join(a.outdir, name + ".json"), "w", encoding="utf-8"), ensure_ascii=False, indent=0)
print(f"{len(split)} items with a break point; variants: {', '.join(variants)}")
