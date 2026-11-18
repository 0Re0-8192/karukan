"""Turns the user's own messages (jsonl with {"text": ...}) into clean segments
for kana-kanji training. A segment is what one composition might hold: a line or
a sentence of 3-120 chars, Japanese-dominant, no code/paths/URLs/log noise.

  python clean_mine.py IN.jsonl [IN2.jsonl ...] -o OUT.jsonl [--show N]

OUT rows: {"text", "left": previous segment of the same message or null, "src"}."""
import argparse, json, random, re, sys

ap = argparse.ArgumentParser()
ap.add_argument("inputs", nargs="+")
ap.add_argument("-o", "--out", required=True)
ap.add_argument("--show", type=int, default=0)
ap.add_argument("--max-seg", type=int, default=120)
a = ap.parse_args()

JA = re.compile(r"[぀-ヿ一-鿿]")
URL = re.compile(r"https?://\S+|www\.\S+")
PATH = re.compile(r"[A-Za-z]:\\\S*|(?:/[\w.\-]+){2,}|\S+\.(?:py|rs|ps1|json|toml|exe|dll|gguf|md|txt|log)\b")
MENTION = re.compile(r"@\w+")
HEX = re.compile(r"\b[0-9a-fA-F]{8,}\b")
PLACEHOLDER = re.compile(r"\[パス\]|\[[^\]]{1,12}\]")
SPLIT = re.compile(r"(?<=[。！？!?])\s*|\n+")
SYM = set("{}[]()<>=;:/\\|\"'`#$%&*_^~")


def segments(text):
    text = URL.sub(" ", text)
    text = PATH.sub(" ", text)
    text = MENTION.sub(" ", text)
    text = HEX.sub(" ", text)
    text = PLACEHOLDER.sub(" ", text)
    out = []
    for s in SPLIT.split(text):
        s = re.sub(r"\s+", " ", s).strip()
        if not s or len(s) < 3 or len(s) > a.max_seg:
            continue
        ja = sum(1 for c in s if JA.match(c))
        if ja / len(s) < 0.5:
            continue
        if sum(1 for c in s if c in SYM) / len(s) > 0.10:
            continue
        out.append(s)
    return out


seen, rows, drop = set(), [], 0
for p in a.inputs:
    src = p.replace("\\", "/").rsplit("/", 1)[-1]
    for i, l in enumerate(open(p, encoding="utf-8")):
        try:
            t = json.loads(l)["text"]
        except Exception:
            drop += 1
            continue
        prev = None
        for s in segments(t):
            if s in seen:
                prev = s
                continue
            seen.add(s)
            rows.append({"text": s, "left": prev, "src": src})
            prev = s
with open(a.out, "w", encoding="utf-8", newline="\n") as f:
    for r in rows:
        f.write(json.dumps(r, ensure_ascii=False) + "\n")
print(f"{len(rows)} segments, {sum(len(r['text']) for r in rows)} chars, {drop} unreadable lines -> {a.out}")
if a.show:
    random.Random(1).shuffle(rows)
    for r in rows[: a.show]:
        print("  ", r["text"])
