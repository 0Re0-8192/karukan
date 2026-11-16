"""Turn the IME's local typing log into dictionary entries.

Reads `corrections.tsv` (written by karukan-imserver: every committed input),
asks a local OpenAI-compatible model which entries are worth remembering, and
writes:

  user_english.txt        English words (one per line)
  user_dicts/auto.txt     Mozc TSV: reading <TAB> surface <TAB> pos <TAB> note

Nothing leaves the machine: the model is a local server (default
http://127.0.0.1:8080). Run it when the PC is idle; it exits quickly when
there is nothing new, and works through a long log in batches of KEEP records
(at most --batches per run), remembering how far it got in `curate.state`.

    python curate.py --data D:\\path\\to\\data [--dry-run] [--stub reply.json]
"""

import argparse
import json
import os
import re
import sys
import urllib.request

KEEP = 300  # records per batch
SENSITIVE = re.compile(r"@|://|\d{6,}|[A-Za-z0-9_\-]{24,}")
HIRAGANA = re.compile(r"^[ぁ-んー]{1,40}$")
WORD = re.compile(r"^[A-Za-z]{3,32}$")
LATIN = re.compile(r"[A-Za-z]")

PROMPT = """あなたは日本語入力システム(IME)の辞書係です。以下はユーザーが入力を確定した記録です。
各行は `種類 / 打ったキー / 読み / 最初の候補 / 確定した文字` です。
種類: conv=最初の候補のまま確定, pick=別の候補を選んだ, fkey=F9/F10で英語にした, live=ライブ変換の結果をそのまま確定。

辞書に登録する価値があるものだけを選んでください。
- english: 打ったキーが英単語・製品名・固有名詞のつづりで、確定した文字がそのつづりと一致するもの。小文字で。
- japanese: 読みと確定した文字の組で、人名・地名・固有名詞・専門用語・よく使う言い回しなど、次も同じ変換をしたいもの。
  読みはひらがなだけ。確定した文字の一部ではなく、語として切り出すこと。
  何度も出てくる語、または pick/fkey で選び直した語を優先する。
次は選ばない: 一般的な語、文全体、数字や記号だけ、パスワード・認証情報、迷うもの。
内容の良し悪しは判断しない(辞書の整理だけが仕事)。
出力は次のJSONだけ(説明は不要):
{"english": ["word", ...], "japanese": [{"reading": "よみ", "surface": "表記"}, ...]}

記録:
"""


def read_new(path, since):
    """Oldest-first batch of at most KEEP records newer than `since`.

    Returns (rows, newest): `newest` is the timestamp the next run resumes
    after (records the filter drops still count as read).
    """
    batch = []
    if not os.path.exists(path):
        return [], since
    with open(path, encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 6 or not parts[0].isdigit():
                continue
            ts = int(parts[0])
            if ts > since:
                batch.append((ts, tuple(parts[1:])))
    batch = batch[:KEEP]
    rows, seen = [], set()
    for _, r in batch:
        kind, raw, reading, first, committed = r
        if SENSITIVE.search(raw) or SENSITIVE.search(committed):
            continue
        # an ordinary conversion taken as offered has nothing to teach the dictionary
        if kind in ("conv", "live") and not LATIN.search(committed):
            continue
        if r in seen:
            continue
        seen.add(r)
        rows.append(r)
    return rows, (batch[-1][0] if batch else since)


def ask(url, prompt):
    body = {
        "model": "local",
        "messages": [{"role": "user", "content": prompt}],
        "temperature": 0,
        "max_tokens": 1500,
        "chat_template_kwargs": {"enable_thinking": False},
    }
    req = urllib.request.Request(
        url.rstrip("/") + "/v1/chat/completions",
        data=json.dumps(body).encode(),
        headers={"Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=600) as r:
        return json.load(r)["choices"][0]["message"]["content"]


def parse(reply):
    m = re.search(r"\{.*\}", reply, re.S)
    if not m:
        return {}
    try:
        return json.loads(m.group(0))
    except json.JSONDecodeError:
        return {}


def pick(rows, out):
    """What of the model's reply is allowed through: it must not invent."""
    committed = "\n".join(r[4] for r in rows)
    known_words = {r[1].lower() for r in rows} | {x.lower() for x in re.findall(r"[A-Za-z]{3,32}", committed)}
    english = sorted(
        {
            w.lower()
            for w in out.get("english", [])
            if isinstance(w, str) and WORD.match(w) and w.lower() in known_words
        }
    )
    japanese = []
    for e in out.get("japanese", []):
        if not isinstance(e, dict):
            continue
        reading, surface = str(e.get("reading", "")), str(e.get("surface", ""))
        if HIRAGANA.match(reading) and surface and surface != reading and len(surface) <= 30 and surface in committed:
            japanese.append((reading, surface))
    return english, japanese


def save(data, english, japanese):
    eng_path = os.path.join(data, "user_english.txt")
    have = set()
    if os.path.exists(eng_path):
        have = {l.strip().lower() for l in open(eng_path, encoding="utf-8")}
    with open(eng_path, "a", encoding="utf-8") as f:
        for w in english:
            if w not in have:
                have.add(w)
                f.write(w + "\n")

    dict_dir = os.path.join(data, "user_dicts")
    os.makedirs(dict_dir, exist_ok=True)
    dict_path = os.path.join(dict_dir, "auto.txt")
    known = set()
    if os.path.exists(dict_path):
        known = {tuple(l.split("\t")[:2]) for l in open(dict_path, encoding="utf-8") if not l.startswith("#")}
    with open(dict_path, "a", encoding="utf-8") as f:
        for reading, surface in japanese:
            if (reading, surface) not in known:
                known.add((reading, surface))
                f.write(f"{reading}\t{surface}\t固有名詞\tauto\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True, help="the IME data directory")
    ap.add_argument("--url", default="http://127.0.0.1:8080")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--stub", help="read the model reply from this file instead")
    ap.add_argument("--batches", type=int, default=6, help="batches of KEEP records per run")
    a = ap.parse_args()

    log = os.path.join(a.data, "corrections.tsv")
    state = os.path.join(a.data, "curate.state")
    since = 0
    if os.path.exists(state):
        try:
            since = int(open(state).read().strip())
        except ValueError:
            pass

    worked = False
    for _ in range(max(1, a.batches)):
        rows, newest = read_new(log, since)
        if newest == since:
            break
        if not rows:  # everything in the batch was filtered out: still counts as read
            if a.dry_run:
                break
            since = newest
            with open(state, "w") as f:
                f.write(str(since))
            continue

        prompt = PROMPT + "\n".join("\t".join(r) for r in rows)
        if a.dry_run and not a.stub:
            print(prompt)
            return 0
        reply = open(a.stub, encoding="utf-8").read() if a.stub else ask(a.url, prompt)
        english, japanese = pick(rows, parse(reply))
        print(f"english: {english}")
        print(f"japanese: {japanese}")
        worked = True
        if a.dry_run:
            return 0
        save(a.data, english, japanese)
        since = newest
        with open(state, "w") as f:
            f.write(str(since))

    if not worked:
        print("nothing new")
    return 0


if __name__ == "__main__":
    sys.exit(main())
