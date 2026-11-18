"""AJIMEE-Bench (evaluation_items.json) with a jinen-format HF checkpoint, greedy.
Accuracy@1 is NFKC-exact against any of the expected outputs, like ajimee-bench.

  python eval_ajimee.py CKPT_DIR evaluation_items.json [--no-context] [--device cuda] [--show 15]"""
import argparse, json, time, unicodedata

import torch
from tokenizers import Tokenizer
from transformers import AutoModelForCausalLM

ap = argparse.ArgumentParser()
ap.add_argument("ckpt")
ap.add_argument("bench")
ap.add_argument("--no-context", action="store_true")
ap.add_argument("--device", default="cuda")
ap.add_argument("--show", type=int, default=15)
ap.add_argument("--vram-gb", type=float, default=4.0)
a = ap.parse_args()
if a.device == "cuda":
    torch.cuda.set_per_process_memory_fraction(a.vram_gb / (torch.cuda.get_device_properties(0).total_memory / 2**30))
nfkc = lambda s: unicodedata.normalize("NFKC", s)
tok = Tokenizer.from_file(a.ckpt + "/tokenizer.json")
CTX, INP, OUT = (tok.token_to_id(c) for c in ("\uee02", "\uee00", "\uee01"))
model = AutoModelForCausalLM.from_pretrained(a.ckpt, dtype=torch.bfloat16 if a.device == "cuda" else torch.float32).to(a.device).eval()
enc = lambda s: tok.encode(s, add_special_tokens=False).ids
items = [i for i in json.load(open(a.bench, encoding="utf-8")) if i.get("input")]
ok, ok_raw, miss, t0 = 0, 0, [], time.time()
for it in items:
    ctx = "" if a.no_context else (it.get("context_text") or "")
    p = [CTX] + enc(nfkc(ctx)) + [INP] + enc(nfkc(it["input"])) + [OUT]
    x = torch.tensor([p], device=a.device)
    with torch.no_grad():
        y = model.generate(x, attention_mask=torch.ones_like(x), max_new_tokens=100, do_sample=False, eos_token_id=1, pad_token_id=0)
    out = tok.decode(y[0, len(p):].tolist(), skip_special_tokens=True).strip()
    exp = it["expected_output"]
    hit = nfkc(out) in {nfkc(e) for e in exp}
    ok += hit
    ok_raw += out in exp
    if not hit:
        miss.append((it["input"], ctx[-20:], out, exp[0]))
n = len(items)
print(f"{a.ckpt}: Accuracy@1 (NFKC) {ok / n:.1%} ({ok}/{n}); exact {ok_raw / n:.1%}; context={'off' if a.no_context else 'on'}; {time.time() - t0:.0f}s")
for m in miss[: a.show]:
    print("  MISS", m[0], "| ctx:", m[1], "| got:", m[2], "| want:", m[3])
