"""Per-group accuracy on the set from build_mixed_eval.py (greedy, jinen format).

  python eval_mixed.py CKPT_DIR items.json [--show 10] [--device cuda]"""
import argparse, json, time, unicodedata, collections
import torch
from tokenizers import Tokenizer
from transformers import AutoModelForCausalLM

ap = argparse.ArgumentParser()
ap.add_argument("ckpt"); ap.add_argument("bench")
ap.add_argument("--device", default="cuda"); ap.add_argument("--show", type=int, default=0)
ap.add_argument("--gguf", default=None, help="GGUF file name inside CKPT_DIR (dequantised on load), for the shipped jinen models")
a = ap.parse_args()
nfkc = lambda s: unicodedata.normalize("NFKC", s)
tok = Tokenizer.from_file(a.ckpt + "/tokenizer.json")
CTX, INP, OUT = (tok.token_to_id(c) for c in ("", "", ""))
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
hit, tot, miss = collections.Counter(), collections.Counter(), []
for it in json.load(open(a.bench, encoding="utf-8")):
    p = [CTX] + enc(nfkc(it["context_text"])) + [INP] + enc(nfkc(it["input"])) + [OUT]
    x = torch.tensor([p], device=a.device)
    with torch.no_grad():
        y = model.generate(x, attention_mask=torch.ones_like(x), max_new_tokens=60, do_sample=False, eos_token_id=EOS_IDS, pad_token_id=0)
    out = tok.decode(y[0, len(p):].tolist(), skip_special_tokens=True).strip()
    ok = nfkc(out) in {nfkc(e) for e in it["expected_output"]}
    for k in (it["group"] + "-" + it["layout"], it["group"], "all"):
        tot[k] += 1; hit[k] += ok
    if not ok:
        miss.append((it["group"] + "-" + it["layout"], it["context_text"][-12:], it["input"], out, it["expected_output"][0]))
print(a.ckpt, "  ".join(f"{k} {hit[k]}/{tot[k]} ({hit[k] / tot[k]:.0%})" for k in sorted(tot)))
for m in miss[: a.show]:
    print("  MISS", *m, sep=" | ")
