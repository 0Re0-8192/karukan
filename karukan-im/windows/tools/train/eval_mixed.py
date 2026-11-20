"""Per-group accuracy on the set from build_mixed_eval.py (greedy, jinen format).

  python eval_mixed.py CKPT_DIR items.json [--show 10] [--device cuda]"""
import argparse, json, time, unicodedata, collections
import torch
from tokenizers import Tokenizer
from transformers import AutoModelForCausalLM

ap = argparse.ArgumentParser()
ap.add_argument("ckpt"); ap.add_argument("bench")
ap.add_argument("--device", default="cuda"); ap.add_argument("--show", type=int, default=0)
a = ap.parse_args()
nfkc = lambda s: unicodedata.normalize("NFKC", s)
tok = Tokenizer.from_file(a.ckpt + "/tokenizer.json")
CTX, INP, OUT = (tok.token_to_id(c) for c in ("", "", ""))
model = AutoModelForCausalLM.from_pretrained(a.ckpt, dtype=torch.bfloat16 if a.device == "cuda" else torch.float32).to(a.device).eval()
enc = lambda s: tok.encode(s, add_special_tokens=False).ids
hit, tot, miss = collections.Counter(), collections.Counter(), []
for it in json.load(open(a.bench, encoding="utf-8")):
    p = [CTX] + enc(nfkc(it["context_text"])) + [INP] + enc(nfkc(it["input"])) + [OUT]
    x = torch.tensor([p], device=a.device)
    with torch.no_grad():
        y = model.generate(x, attention_mask=torch.ones_like(x), max_new_tokens=60, do_sample=False, eos_token_id=1, pad_token_id=0)
    out = tok.decode(y[0, len(p):].tolist(), skip_special_tokens=True).strip()
    ok = nfkc(out) in {nfkc(e) for e in it["expected_output"]}
    for k in (it["group"] + "-" + it["layout"], it["group"], "all"):
        tot[k] += 1; hit[k] += ok
    if not ok:
        miss.append((it["group"] + "-" + it["layout"], it["context_text"][-12:], it["input"], out, it["expected_output"][0]))
print(a.ckpt, "  ".join(f"{k} {hit[k]}/{tot[k]} ({hit[k] / tot[k]:.0%})" for k in sorted(tot)))
for m in miss[: a.show]:
    print("  MISS", *m, sep=" | ")
