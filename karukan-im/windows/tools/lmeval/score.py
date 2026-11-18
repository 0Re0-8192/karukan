"""Context-understanding test for any causal LM (no kana-kanji training needed).
For each item compares log P(pre + surface + post) of the right surface against the
wrong ones. Accuracy = share of items where the right one scores highest.

  python -I score.py MODEL_DIR_OR_HF_ID [--set ctxset.jsonl] [--device cuda|cpu] [--out res.json]

Needs torch + transformers (install into a venv on D:, never on C:)."""
import argparse, json, pathlib, sys, time

ap = argparse.ArgumentParser()
ap.add_argument("model")
ap.add_argument("--set", default=str(pathlib.Path(__file__).with_name("ctxset.jsonl")))
ap.add_argument("--device", default="cuda")
ap.add_argument("--out")
ap.add_argument("--bos", default="auto", help="auto|yes|no: prepend BOS so the first token is scored")
a = ap.parse_args()

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

tok = AutoTokenizer.from_pretrained(a.model)
lm = AutoModelForCausalLM.from_pretrained(a.model, torch_dtype=torch.bfloat16 if a.device == "cuda" else torch.float32).to(a.device).eval()
bos = tok.bos_token_id if tok.bos_token_id is not None else tok.eos_token_id
use_bos = a.bos != "no" and bos is not None


@torch.no_grad()
def logp(text):
    ids = tok(text, add_special_tokens=False)["input_ids"]
    if use_bos:
        ids = [bos] + ids
    x = torch.tensor([ids], device=a.device)
    lg = lm(x).logits[0, :-1].float().log_softmax(-1)
    return lg.gather(1, x[0, 1:, None]).sum().item()


items = [json.loads(l) for l in open(a.set, encoding="utf-8") if l.strip()]
rows, t0 = [], time.time()
for it in items:
    full = lambda s: it["pre"] + s + it["post"]
    s_ok = logp(full(it["ok"]))
    s_ng = [(n, logp(full(n))) for n in it["ng"]]
    best = max(s_ng, key=lambda t: t[1])
    rows.append(dict(item=it, ok=s_ok, ng=s_ng, hit=s_ok > best[1], margin=s_ok - best[1]))

def acc(rs): return sum(r["hit"] for r in rs) / max(1, len(rs))
ctx = [r for r in rows if r["item"].get("src") != "corrections"]
real = [r for r in rows if r["item"].get("src") == "corrections"]
print(f"{a.model}: all {acc(rows):.1%} ({sum(r['hit'] for r in rows)}/{len(rows)})  context-set {acc(ctx):.1%} ({len(ctx)})  real-mistakes {acc(real):.1%} ({len(real)})  {time.time()-t0:.0f}s")
for r in rows:
    if not r["hit"]:
        i = r["item"]; print("  MISS", i["pre"] + "[" + i["ok"] + "|" + "/".join(i["ng"]) + "]" + i["post"], f"{r['margin']:.1f}")
if a.out:
    json.dump(rows, open(a.out, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
