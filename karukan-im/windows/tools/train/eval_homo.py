"""Scores a frozen homophone set (homo_eval.py check) with a jinen-format checkpoint, greedy.

  python eval_homo.py score CKPT items.json -o result.json [--gguf FILE] [--ctx-chars 10]
  python eval_homo.py compare result_a.json result_b.json ...

Each item is run twice: with the whole context_text, and with only its last --ctx-chars characters, which is what
the engine hands the model (`[conversion] context_chars`, default 10). An answer is `ok` (one of the keys), `rival`
(a surface the context rules out: the homophone error) or `other` (anything else: orthography, segmentation).
`compare` prints accuracy by tag and, for each pair of results, the discordant counts with an exact McNemar p."""
import argparse, json, math, sys, unicodedata
from collections import Counter

nfkc = lambda s: unicodedata.normalize("NFKC", s)


def score(a):
    import torch
    from tokenizers import Tokenizer
    from transformers import AutoModelForCausalLM
    tok = Tokenizer.from_file(a.ckpt + "/tokenizer.json")
    CTX, INP, OUT = (tok.token_to_id(c) for c in ("\uee02", "\uee00", "\uee01"))
    extra = {}
    if a.gguf:
        import gguf as _gguf
        from transformers import AutoConfig
        r = _gguf.GGUFReader(a.ckpt + "/" + a.gguf)
        f = r.fields["qwen3.attention.key_length"]
        cfg = AutoConfig.from_pretrained(a.ckpt, gguf_file=a.gguf)
        cfg.head_dim = int(f.parts[f.data[0]][0])
        extra = {"gguf_file": a.gguf, "config": cfg}
    model = AutoModelForCausalLM.from_pretrained(a.ckpt, **extra, dtype=torch.bfloat16).to("cuda").eval()
    eos = model.generation_config.eos_token_id
    EOS_IDS = sorted({1} | {i for i in (tok.token_to_id(t) for t in ("</s>", "<|endoftext|>", "<|im_end|>", "<eos>")) if i is not None}
                     | ({eos} if isinstance(eos, int) else set(eos or [])))
    enc = lambda s: tok.encode(s, add_special_tokens=False).ids

    def run(ctx, inp):
        p = [CTX] + enc(nfkc(ctx)) + [INP] + enc(nfkc(inp)) + [OUT]
        x = torch.tensor([p], device="cuda")
        with torch.no_grad():
            y = model.generate(x, attention_mask=torch.ones_like(x), max_new_tokens=32, do_sample=False, eos_token_id=EOS_IDS, pad_token_id=0)
        return tok.decode(y[0, len(p):].tolist(), skip_special_tokens=True).strip()

    items = json.load(open(a.items, encoding="utf-8"))
    res = []
    for it in items:
        keys, riv = {nfkc(s) for s in it["expected_output"]}, {nfkc(s) for s in it["rivals"]}
        r = {"index": it["index"]}
        for mode, ctx in (("full", it["context_text"]), ("c%d" % a.ctx_chars, it["context_text"][-a.ctx_chars:])):
            o = nfkc(run(ctx, it["input"]))
            r[mode] = o
            r[mode + "_v"] = "ok" if o in keys else "rival" if o in riv else "other"
        res.append(r)
    json.dump({"ckpt": a.ckpt, "items": a.items, "results": res}, open(a.out, "w", encoding="utf-8"), ensure_ascii=False, indent=0)
    summary(a.items, [a.out])


def mcnemar(b, c):
    n = b + c
    if n == 0:
        return 1.0
    k = min(b, c)
    p = sum(math.comb(n, i) for i in range(k + 1)) / 2 ** n
    return min(1.0, 2 * p)


def summary(items_path, paths):
    items = {it["index"]: it for it in json.load(open(items_path, encoding="utf-8"))}
    tags = sorted({k for it in items.values() for k in it if k.startswith("held_")})
    loaded = [(p, json.load(open(p, encoding="utf-8"))) for p in paths]
    modes = [m for m in loaded[0][1]["results"][0] if m.endswith("_v")]
    for p, d in loaded:
        name = d["ckpt"].rstrip("\\/").split("\\")[-1].split("/")[-1]
        for m in modes:
            parts = []
            groups = {"all": lambda it: True, "major": lambda it: it["sense"] == "major", "minor": lambda it: it["sense"] == "minor",
                      "near": lambda it: not it.get("cue_far"), "far": lambda it: it.get("cue_far"),
                      "plain": lambda it: not it.get("extended")}
            for t in tags:
                groups[t] = (lambda t: lambda it: it[t])(t)
            for g, f in groups.items():
                v = Counter(r[m] for r in d["results"] if f(items[r["index"]]))
                n = sum(v.values()) or 1
                parts.append(f"{g} {v['ok'] / n:.1%} (n={n}, rival {v['rival']}, other {v['other']})")
            print(f"{name:24s} {m[:-2]:5s} " + "  ".join(parts))
    if len(loaded) > 1:
        for m in modes:
            print(f"-- pairwise {m[:-2]} (only-A-right / only-B-right, McNemar p)")
            for i in range(len(loaded)):
                for j in range(i + 1, len(loaded)):
                    A = {r["index"]: r[m] == "ok" for r in loaded[i][1]["results"]}
                    B = {r["index"]: r[m] == "ok" for r in loaded[j][1]["results"]}
                    b = sum(A[k] and not B[k] for k in A if k in B)
                    c = sum(B[k] and not A[k] for k in A if k in B)
                    na = loaded[i][1]["ckpt"].rstrip("\\/").replace("/", "\\").split("\\")[-1]
                    nb = loaded[j][1]["ckpt"].rstrip("\\/").replace("/", "\\").split("\\")[-1]
                    print(f"   {na} vs {nb}: {b} / {c}  p={mcnemar(b, c):.3f}")


ap = argparse.ArgumentParser()
sp = ap.add_subparsers(dest="cmd", required=True)
p = sp.add_parser("score"); p.add_argument("ckpt"); p.add_argument("items"); p.add_argument("-o", "--out", required=True)
p.add_argument("--gguf"); p.add_argument("--ctx-chars", type=int, default=10)
p = sp.add_parser("compare"); p.add_argument("items"); p.add_argument("results", nargs="+")
a = ap.parse_args()
if a.cmd == "score":
    score(a)
else:
    summary(a.items, a.results)
