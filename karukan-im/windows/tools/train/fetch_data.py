"""Samples zenz-v2.5-dataset evenly across both files (not the head, which is one
stretch of one topic) with HTTP range requests of 1 MB at random offsets, and
fetches AJIMEE-Bench. Partial first/last lines of each chunk are dropped.

  python fetch_data.py OUT_DIR [--wiki-mb 600] [--cc-mb 900] [--seed 1]"""
import argparse, json, os, random, sys, time, urllib.request
from concurrent.futures import ThreadPoolExecutor

ap = argparse.ArgumentParser()
ap.add_argument("out")
ap.add_argument("--wiki-mb", type=int, default=300)
ap.add_argument("--cc-mb", type=int, default=450)
ap.add_argument("--seed", type=int, default=1)
a = ap.parse_args()

BASE = "https://huggingface.co/datasets/Miwa-Keita/zenz-v2.5-dataset/resolve/main/"
SIZES = {"train_wikipedia.jsonl": 4131_400_000, "train_llm-jp-corpus-v3.jsonl": 32384_400_000}
CHUNK = 1 << 20
os.makedirs(a.out, exist_ok=True)


def fetch(name, off):
    for attempt in range(4):
        try:
            req = urllib.request.Request(BASE + name, headers={"Range": f"bytes={off}-{off + CHUNK - 1}"})
            b = urllib.request.urlopen(req, timeout=60).read()
            lines = b.decode("utf-8", "ignore").split("\n")[1:-1]   # drop the cut ends
            ok = []
            for l in lines:
                try:
                    j = json.loads(l)
                except Exception:
                    continue
                if j.get("input") and j.get("output"):
                    ok.append(l)
            return ok
        except Exception as e:
            time.sleep(2 * (attempt + 1))
    return []


def sample(name, mb, seed):
    rnd = random.Random(seed)
    size = SIZES[name]
    n = mb
    # stratified: one chunk from each of n equal slices of the file
    offs = [int((i + rnd.random()) * (size - CHUNK) / n) for i in range(n)]
    t0, got = time.time(), 0
    path = os.path.join(a.out, name.replace("train_", "sample_"))
    with open(path, "w", encoding="utf-8", newline="\n") as f, ThreadPoolExecutor(8) as ex:
        for lines in ex.map(lambda o: fetch(name, o), offs):
            for l in lines:
                f.write(l + "\n")
            got += len(lines)
    print(f"{name}: {got} lines from {n} slices in {time.time()-t0:.0f}s -> {path}", flush=True)


sample("train_wikipedia.jsonl", a.wiki_mb, a.seed)
sample("train_llm-jp-corpus-v3.jsonl", a.cc_mb, a.seed + 1)

# AJIMEE-Bench
os.makedirs(os.path.join(a.out, "ajimee"), exist_ok=True)
for u in ["https://raw.githubusercontent.com/azooKey/AJIMEE-Bench/main/JWTD_v2/v1/evaluation_items.json"]:
    try:
        d = urllib.request.urlopen(u, timeout=60).read()
        open(os.path.join(a.out, "ajimee", "evaluation_items.json"), "wb").write(d)
        print("ajimee:", len(d), "bytes")
    except Exception as e:
        print("ajimee failed:", e)
