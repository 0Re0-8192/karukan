"""Fine-tunes a causal LM into a kana-kanji converter in the jinen format:

    <left_context><katakana input><output></s>

(NFKC, no BOS: karukan tokenizes without special tokens.) Loss only on <output></s>.
Kind to a shared machine: VRAM capped (per-process fraction), GPU duty cycle capped
(sleeps after each micro-step), pauses while other programs leave too little VRAM,
and stops cleanly when the file STOP appears in the output directory.

  python train_kk.py --model gemma-trim --out run1 \\
      --data wiki=sample_wikipedia.jsonl:0.35 cc=sample_llm-jp.jsonl:0.63 mine=mine_pairs.jsonl:0.02 \\
      --steps 20000 --vram-gb 6 --duty 0.5

Resumes from OUT/ckpt when it exists."""
import argparse, json, math, os, queue, random, subprocess, threading, time, unicodedata

import numpy as np
import torch
from tokenizers import Tokenizer
from transformers import AutoModelForCausalLM

ap = argparse.ArgumentParser()
ap.add_argument("--model", required=True)
ap.add_argument("--out", required=True)
ap.add_argument("--data", nargs="+", required=True, help="name=path:weight")
ap.add_argument("--steps", type=int, default=20000)
ap.add_argument("--micro-bs", type=int, default=32)
ap.add_argument("--accum", type=int, default=8)
ap.add_argument("--lr", type=float, default=1e-4)
ap.add_argument("--warmup", type=int, default=200)
ap.add_argument("--max-len", type=int, default=160)
ap.add_argument("--device", default="cuda")
ap.add_argument("--vram-gb", type=float, default=6.0)
ap.add_argument("--duty", type=float, default=0.5, help="max share of wall time the GPU computes")
ap.add_argument("--min-free-gb", type=float, default=2.0, help="pause while other programs leave less VRAM free")
ap.add_argument("--save-min", type=float, default=20)
ap.add_argument("--eval-every", type=int, default=500)
ap.add_argument("--holdout-mine", type=int, default=300, help="first N rows of the `mine` source are never trained on")
ap.add_argument("--seed", type=int, default=1)
ap.add_argument("--threads", type=int, default=8)
a = ap.parse_args()
os.makedirs(a.out, exist_ok=True)
torch.set_num_threads(a.threads)
LOG = open(os.path.join(a.out, "train.log"), "a", encoding="utf-8", buffering=1)


def log(*m):
    s = time.strftime("%H:%M:%S ") + " ".join(str(x) for x in m)
    print(s, flush=True)
    LOG.write(s + "\n")


nfkc = lambda s: unicodedata.normalize("NFKC", s)
tok = Tokenizer.from_file(os.path.join(a.model, "tokenizer.json"))
M_CTX, M_IN, M_OUT = tok.token_to_id(""), tok.token_to_id(""), tok.token_to_id("")
EOS, PAD = 1, 0
assert None not in (M_CTX, M_IN, M_OUT), "marker tokens missing from the tokenizer"
enc = lambda s: tok.encode(s, add_special_tokens=False).ids


class Source:
    def __init__(self, name, path, weight, skip=0):
        self.name, self.weight, self.skip = name, weight, skip
        self.f = open(path, "rb")
        import mmap
        self.mm = mmap.mmap(self.f.fileno(), 0, access=mmap.ACCESS_READ)
        nl = np.flatnonzero(np.frombuffer(self.mm, dtype=np.uint8) == 10)
        self.start = np.concatenate([[0], nl[:-1] + 1]) if len(nl) else np.zeros(0, dtype=np.int64)
        self.end = nl
        self.n = len(self.end) - skip
        self.perm = np.random.RandomState(a.seed + hash(name) % 1000).permutation(self.n) + skip
        self.counter = 0

    def row(self, i):
        return json.loads(self.mm[self.start[i]:self.end[i]].decode("utf-8"))

    def next(self):
        i = int(self.perm[self.counter % self.n])
        self.counter += 1
        return self.row(i)


sources = []
for spec in a.data:
    name, rest = spec.split("=", 1)
    path, w = rest.rsplit(":", 1)
    sources.append(Source(name, path, float(w), a.holdout_mine if name == "mine" else 0))
tot_w = sum(s.weight for s in sources)
log("sources", [(s.name, s.n, s.weight / tot_w) for s in sources])


def encode_row(r):
    ctx = nfkc(r.get("left_context") or "")
    ids_p = [M_CTX] + enc(ctx) + [M_IN] + enc(nfkc(r["input"])) + [M_OUT]
    ids_o = enc(nfkc(r["output"])) + [EOS]
    if len(ids_p) + len(ids_o) > a.max_len:
        if len(ids_p) + len(ids_o) - len(enc(ctx)) > a.max_len or not ctx:
            return None
        cut = len(ids_p) + len(ids_o) - a.max_len        # shave the oldest context
        ids_p = [M_CTX] + ids_p[1 + cut:]
    return ids_p, ids_o


rng = random.Random(a.seed)


POOL_STEPS = 8          # steps' worth of rows sorted by length together


def step_groups():
    """One optimizer step's micro-batches at a time. Rows are drawn POOL_STEPS steps at a time and
    sorted by length before being cut into micro-batches, so a micro-batch holds rows of about
    one length (dictionary words are ~15 tokens, sentences up to 160: mixed, most of the
    compute went to padding); the micro-batches are then shuffled across the pool's steps."""
    per = a.accum * a.micro_bs
    while True:
        items = []
        while len(items) < per * POOL_STEPS:
            s = rng.choices(sources, weights=[x.weight for x in sources])[0]
            e = encode_row(s.next())
            if e:
                items.append(e)
        items.sort(key=lambda x: len(x[0]) + len(x[1]))
        mbs = [collate(items[i:i + a.micro_bs]) for i in range(0, len(items), a.micro_bs)]
        rng.shuffle(mbs)
        for k in range(POOL_STEPS):
            yield mbs[k * a.accum:(k + 1) * a.accum]


def prefetch(gen, depth=6):
    """Builds the next steps' batches on a thread while the GPU works on this one."""
    q = queue.Queue(maxsize=depth)

    def work():
        for g in gen:
            q.put(g)

    threading.Thread(target=work, daemon=True).start()
    while True:
        yield q.get()


def collate(items):
    L = max(len(p) + len(o) for p, o in items)
    ids = torch.full((len(items), L), PAD, dtype=torch.long)
    lab = torch.full((len(items), L), -100, dtype=torch.long)
    att = torch.zeros((len(items), L), dtype=torch.long)
    for i, (p, o) in enumerate(items):
        n = len(p) + len(o)
        ids[i, :n] = torch.tensor(p + o)
        lab[i, len(p):n] = torch.tensor(o)
        att[i, :n] = 1
    return ids, att, lab


dev = a.device
if dev == "cuda":
    total_gb = torch.cuda.get_device_properties(0).total_memory / 2**30
    torch.cuda.set_per_process_memory_fraction(min(0.95, a.vram_gb / total_gb))


def free_gb():
    try:
        out = subprocess.run(["nvidia-smi", "--query-gpu=memory.free", "--format=csv,noheader,nounits"],
                             capture_output=True, text=True, timeout=10).stdout.strip().splitlines()[0]
        return float(out) / 1024
    except Exception:
        return 99.0


def wait_for_room(need):
    shown = False
    while dev == "cuda" and free_gb() < need:
        if not shown:
            log(f"VRAM free {free_gb():.1f} GB < {need:.1f}: waiting for the other programs")
            shown = True
        if os.path.exists(os.path.join(a.out, "STOP")):
            return
        time.sleep(30)


wait_for_room(a.vram_gb + 1.0)
ck = os.path.join(a.out, "ckpt")
resume = os.path.exists(os.path.join(ck, "state.json"))
model = AutoModelForCausalLM.from_pretrained(ck if resume else a.model, dtype=torch.float32).to(dev)
model.config.use_cache = False
opt = torch.optim.AdamW(model.parameters(), lr=a.lr, betas=(0.9, 0.95), weight_decay=0.01, fused=(dev == "cuda"))
step = 0
if resume:
    st = json.load(open(os.path.join(ck, "state.json")))
    step = st["step"]
    for s in sources:
        s.counter = st["counters"].get(s.name, 0)
    opt.load_state_dict(torch.load(os.path.join(ck, "opt.pt"), map_location=dev))
    rng.setstate(tuple(tuple(x) if isinstance(x, list) else x for x in st["rng"]))
    log("resumed at step", step)
log("params %.1fM" % (sum(p.numel() for p in model.parameters()) / 1e6))


def save():
    model.save_pretrained(ck)
    torch.save(opt.state_dict(), os.path.join(ck, "opt.pt"))
    json.dump({"step": step, "counters": {s.name: s.counter for s in sources}, "rng": rng.getstate()},
              open(os.path.join(ck, "state.json"), "w"))
    for f in ("tokenizer.json", "tokenizer_config.json"):
        open(os.path.join(ck, f), "wb").write(open(os.path.join(a.model, f), "rb").read())
    log("checkpoint saved at step", step)


mine = next((s for s in sources if s.name == "mine"), None)
held = []
if mine is not None and a.holdout_mine:
    mp = a.data[[s.name for s in sources].index("mine")].split("=", 1)[1].rsplit(":", 1)[0]
    held = [json.loads(l) for _, l in zip(range(a.holdout_mine), open(mp, encoding="utf-8"))]


@torch.no_grad()
def evaluate():
    model.eval()
    ok, n = 0, min(100, len(held))
    for r in held[:n]:
        p = [M_CTX] + enc(nfkc(r.get("left_context") or "")) + [M_IN] + enc(nfkc(r["input"])) + [M_OUT]
        x = torch.tensor([p], device=dev)
        with torch.autocast(dev, dtype=torch.bfloat16, enabled=(dev == "cuda")):
            y = model.generate(x, attention_mask=torch.ones_like(x), max_new_tokens=80, do_sample=False,
                               eos_token_id=EOS, pad_token_id=PAD)
        out = tok.decode(y[0, len(p):].tolist(), skip_special_tokens=True)
        ok += out.strip() == nfkc(r["output"]).strip()
    model.train()
    return ok / max(1, n)


CAP = getattr(model.config, "final_logit_softcapping", None)


def out_loss(ids, att, lab):
    """Cross-entropy on the output positions only: the output layer (60k wide) is applied
    to just those rows instead of every position, which is most of the memory otherwise."""
    h = model.model(input_ids=ids, attention_mask=att).last_hidden_state[:, :-1]
    tgt = lab[:, 1:]
    m = tgt != -100
    lg = model.lm_head(h[m]).float()
    if CAP:
        lg = torch.tanh(lg / CAP) * CAP
    return torch.nn.functional.cross_entropy(lg, tgt[m], reduction="sum")   # the caller divides by the step's token count


model.train()
groups = prefetch(step_groups())
t_save, t_start, tok_seen = time.time(), time.time(), 0
total = a.steps
while step < total:
    if os.path.exists(os.path.join(a.out, "STOP")):
        log("STOP file found")
        break
    if step % 50 == 0:
        wait_for_room(a.min_free_gb)
    lr = a.lr * min(1.0, (step + 1) / a.warmup) * (0.1 + 0.9 * 0.5 * (1 + math.cos(math.pi * step / total)))
    for g in opt.param_groups:
        g["lr"] = lr
    loss_sum = 0.0
    group = next(groups)
    n_out = sum(int((lab[:, 1:] != -100).sum()) for _, _, lab in group)   # every output token weighs the same, whichever micro-batch it is in
    for ids, att, lab in group:
        t0 = time.perf_counter()
        ids, att, lab = ids.to(dev, non_blocking=True), att.to(dev, non_blocking=True), lab.to(dev, non_blocking=True)
        with torch.autocast(dev, dtype=torch.bfloat16, enabled=(dev == "cuda")):
            loss = out_loss(ids, att, lab) / n_out
        loss.backward()
        loss_sum += loss.item()
        tok_seen += int(att.sum())
        if dev == "cuda":
            torch.cuda.synchronize()
        busy = time.perf_counter() - t0
        if a.duty < 1.0:
            time.sleep(busy * (1 / a.duty - 1))
    torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
    opt.step()
    opt.zero_grad(set_to_none=True)
    step += 1
    if step % 20 == 0:
        el = time.time() - t_start
        log(f"step {step}/{total} loss {loss_sum:.4f} lr {lr:.2e} {tok_seen / el:.0f} tok/s  eta {(total - step) * el / max(1, step - 0) / 3600:.2f} h"
            + (f"  vram {torch.cuda.max_memory_allocated() / 2**30:.1f} GB" if dev == "cuda" else ""))
    if step % a.eval_every == 0 and held:
        log(f"step {step}: greedy exact match on held-out own text {evaluate():.1%}")
    if time.time() - t_save > a.save_min * 60:
        save()
        t_save = time.time()
save()
model.save_pretrained(os.path.join(a.out, "final"))
for f in ("tokenizer.json", "tokenizer_config.json"):
    open(os.path.join(a.out, "final", f), "wb").write(open(os.path.join(a.model, f), "rb").read())
log("done at step", step)
