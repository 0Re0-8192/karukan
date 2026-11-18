"""Cuts a model's vocabulary down to the tokens the training data uses, and adds
the three jinen markers (U+EE00-EE02) as single tokens.

  python trim_vocab.py SRC_MODEL_DIR OUT_DIR --data a.jsonl b.jsonl ... [--lines 200000]

Each --data file holds {"input","output","left_context"} rows (or {"text"}).
A token is kept when the data uses it, or it is a special / byte-fallback token,
or it is a merge parent of a kept token (so BPE reaches every kept token the same
way). Embeddings (tied with the output layer) are sliced to match. CPU only."""
import argparse, collections, itertools, json, os, sys

import torch
from tokenizers import Tokenizer
from transformers import AutoModelForCausalLM

ap = argparse.ArgumentParser()
ap.add_argument("src")
ap.add_argument("out")
ap.add_argument("--data", nargs="+", required=True)
ap.add_argument("--lines", type=int, default=200000, help="rows read from each data file")
ap.add_argument("--extra-text", nargs="*", default=[], help="plain text files, every line is used")
a = ap.parse_args()

MARK = ["", "", ""]
tok = Tokenizer.from_file(os.path.join(a.src, "tokenizer.json"))
tj = json.load(open(os.path.join(a.src, "tokenizer.json"), encoding="utf-8"))
vocab = tj["model"]["vocab"]
merges = [tuple(m) if isinstance(m, list) else tuple(m.split(" ")) for m in tj["model"]["merges"]]
inv = {i: t for t, i in vocab.items()}


def texts():
    for p in a.data:
        for l in itertools.islice(open(p, encoding="utf-8"), a.lines):
            j = json.loads(l)
            if "input" in j:
                yield (j.get("left_context") or "") + j["input"] + j["output"]
                yield j["output"]
                yield j["input"]
            else:
                yield j["text"]
    for p in a.extra_text:
        for l in open(p, encoding="utf-8"):
            yield l.rstrip("\n")


used = collections.Counter()
batch = []
for t in texts():
    batch.append(t)
    if len(batch) >= 2000:
        for e in tok.encode_batch(batch, add_special_tokens=False):
            used.update(e.ids)
        batch = []
if batch:
    for e in tok.encode_batch(batch, add_special_tokens=False):
        used.update(e.ids)
print("distinct tokens used:", len(used))

keep = set(used)
keep.update(range(0, 4))                                   # pad eos bos unk
keep.update(i for t, i in vocab.items() if len(t) == 6 and t.startswith("<0x") and t.endswith(">"))  # byte fallback
made_by = collections.defaultdict(list)
for x, y in merges:
    made_by[x + y].append((x, y))
stack = [inv[i] for i in keep]
while stack:
    t = stack.pop()
    for x, y in made_by.get(t, ()):
        for p in (x, y):
            if p in vocab and vocab[p] not in keep:
                keep.add(vocab[p])
                stack.append(p)
print("kept after merge-parent closure:", len(keep))

old_ids = sorted(keep)
new_of = {o: n for n, o in enumerate(old_ids)}
new_vocab = {inv[o]: n for o, n in new_of.items()}
for m in MARK:                                              # the three markers get ids of their own
    new_vocab[m] = len(new_vocab)
new_merges = [[x, y] for x, y in merges if x in new_vocab and y in new_vocab and (x + y) in new_vocab]
tj["model"]["vocab"] = new_vocab
tj["model"]["merges"] = new_merges
added = [dict(t, id=new_of[t["id"]]) for t in tj["added_tokens"] if t["id"] in new_of and t["id"] < 4]
for m in MARK:
    added.append({"id": new_vocab[m], "content": m, "single_word": False, "lstrip": False, "rstrip": False,
                  "normalized": False, "special": True})
tj["added_tokens"] = added
pp = tj.get("post_processor") or {}
for k, v in (pp.get("special_tokens") or {}).items():
    v["ids"] = [new_of[i] for i in v["ids"] if i in new_of]
os.makedirs(a.out, exist_ok=True)
json.dump(tj, open(os.path.join(a.out, "tokenizer.json"), "w", encoding="utf-8"), ensure_ascii=False)
json.dump({"tokenizer_class": "PreTrainedTokenizerFast", "bos_token": "<bos>", "eos_token": "<eos>",
           "pad_token": "<pad>", "unk_token": "<unk>", "padding_side": "right"},
          open(os.path.join(a.out, "tokenizer_config.json"), "w", encoding="utf-8"))
print("merges kept:", len(new_merges), "of", len(merges), " new vocab:", len(new_vocab))

model = AutoModelForCausalLM.from_pretrained(a.src, dtype=torch.float32)
emb = model.get_input_embeddings().weight.data
rows = emb[torch.tensor(old_ids)]
bytes_of = lambda ch: [vocab["<0x%02X>" % b] for b in ch.encode("utf-8")]
new_rows = torch.stack([emb[torch.tensor(bytes_of(m))].mean(0) for m in MARK])   # start from the markers' own bytes
w = torch.cat([rows, new_rows], 0)
model.resize_token_embeddings(w.shape[0])
model.get_input_embeddings().weight.data.copy_(w)
model.tie_weights()
model.config.vocab_size = w.shape[0]
model.save_pretrained(a.out)
n = sum(p.numel() for p in model.parameters())
print("saved", a.out, "params %.1fM" % (n / 1e6))
