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
ap.add_argument("--gguf", default=None, help="GGUF file name inside CKPT_DIR (dequantised on load), for the shipped jinen models")
a = ap.parse_args()
if a.device == "cuda":
    torch.cuda.set_per_process_memory_fraction(a.vram_gb / (torch.cuda.get_device_properties(0).total_memory / 2**30))
nfkc = lambda s: unicodedata.normalize("NFKC", s)
tok = Tokenizer.from_file(a.ckpt + "/tokenizer.json")
CTX, INP, OUT = (tok.token_to_id(c) for c in ("\uee02", "\uee00", "\uee01"))
extra = {}
if a.gguf:
    # transformers derives head_dim from hidden/heads; the GGUF states it (Qwen3 differs)
    import gguf as _gguf
    from transformers import AutoConfig
    r = _gguf.GGUFReader(a.ckpt + "/" + a.gguf)
    f = r.fields["qwen3.attention.key_length"]
    cfg = AutoConfig.from_pretrained(a.ckpt, gguf_file=a.gguf)
    cfg.head_dim = int(f.parts[f.data[0]][0])
    extra = {"gguf_file": a.gguf, "config": cfg}
model = AutoModelForCausalLM.from_pretrained(a.ckpt, **extra, dtype=torch.bfloat16 if a.device == "cuda" else torch.float32).to(a.device).eval()
# end-of-text ids: our checkpoints use 1; the shipped jinen GGUFs name their own (not stopping there ran on into junk)
EOS_IDS = sorted({1} | {i for i in (tok.token_to_id(t) for t in ("</s>", "<|endoftext|>", "<|im_end|>", "<eos>")) if i is not None} | ({model.generation_config.eos_token_id} if isinstance(model.generation_config.eos_token_id, int) else set(model.generation_config.eos_token_id or [])))
enc = lambda s: tok.encode(s, add_special_tokens=False).ids
items = [i for i in json.load(open(a.bench, encoding="utf-8")) if i.get("input")]
ok, ok_raw, miss, t0 = 0, 0, [], time.time()
for it in items:
    ctx = "" if a.no_context else (it.get("context_text") or "")
    p = [CTX] + enc(nfkc(ctx)) + [INP] + enc(nfkc(it["input"])) + [OUT]
    x = torch.tensor([p], device=a.device)
    with torch.no_grad():
        y = model.generate(x, attention_mask=torch.ones_like(x), max_new_tokens=100, do_sample=False, eos_token_id=EOS_IDS, pad_token_id=0)
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
