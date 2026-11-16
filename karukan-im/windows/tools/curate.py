"""Turn the IME's local corrections log into dictionary entries.

Reads `corrections.tsv` (written by karukan-imserver), asks a local
OpenAI-compatible model which of the corrections are worth remembering, and
writes:

  user_english.txt        English words (one per line)
  user_dicts/auto.txt     Mozc TSV: reading <TAB> surface <TAB> pos <TAB> note

Nothing leaves the machine: the model is a local server (default
http://127.0.0.1:8080). Run it when the PC is idle; it exits quickly when
there is nothing new.

    python curate.py --data D:\\path\\to\\data [--dry-run] [--stub reply.json]
"""

import argparse
import json
import os
import re
import sys
import urllib.request

KEEP = 300  # records per run
SENSITIVE = re.compile(r"@|://|\d{6,}|[A-Za-z0-9_\-]{24,}")
HIRAGANA = re.compile(r"^[ぁ-んー]{1,40}$")
WORD = re.compile(r"^[A-Za-z]{3,32}$")

PROMPT = """あなたは日本語入力システム(IME)の辞書係です。以下はユーザーが入力を訂正・確定した記録です。
各行は `種類 / 打ったキー / 読み / 最初の候補 / 確定した文字` です。
種類: fkey=F9/F10で英語にした, pick=別の候補を選んだ, live=ライブ変換の結果をそのまま確定。

辞書に登録する価値があるものだけを選んでください。
- english: 打ったキーが英単語・製品名・固有名詞のつづりで、確定した文字がそのつづりと一致するもの。小文字で。
- japanese: 読みと確定した文字の組で、人名・地名・固有名詞・専門用語など、次も同じ変換をしたいもの。
  読みはひらがなだけ。確定した文字の一部ではなく、語として切り出すこと。
次は選ばない: 一般的な語、文全体、数字や記号だけ、個人を特定できる情報(住所・電話・メール・パスワード)、迷うもの。
出力は次のJSONだけ(説明は不要):
{"english": ["word", ...], "japanese": [{"reading": "よみ", "surface": "表記"}, ...]}

記録:
"""


def read_new(path, since):
    rows = []
    if not os.path.exists(path):
        return rows, since
    newest = since
    with open(path, encoding="utf-8") as f:
        for line in f:
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 6 or not parts[0].isdigit():
                continue
            ts = int(parts[0])
            if ts <= since:
                continue
            newest = max(newest, ts)
            kind, raw, reading, first, committed = parts[1:]
            if SENSITIVE.search(raw) or SENSITIVE.search(committed):
                continue
            rows.append((kind, raw, reading, first, committed))
    return rows[-KEEP:], newest


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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True, help="the IME data directory")
    ap.add_argument("--url", default="http://127.0.0.1:8080")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--stub", help="read the model reply from this file instead")
    a = ap.parse_args()

    log = os.path.join(a.data, "corrections.tsv")
    state = os.path.join(a.data, "curate.state")
    since = 0
    if os.path.exists(state):
        try:
            since = int(open(state).read().strip())
        except ValueError:
            pass
    rows, newest = read_new(log, since)
    if not rows:
        print("nothing new")
        return 0

    prompt = PROMPT + "\n".join("\t".join(r) for r in rows)
    if a.dry_run and not a.stub:
        print(prompt)
        return 0
    reply = open(a.stub, encoding="utf-8").read() if a.stub else ask(a.url, prompt)
    out = parse(reply)

    committed = "\n".join(r[4] for r in rows)
    raws = {r[1].lower() for r in rows}
    english = sorted(
        {
            w.lower()
            for w in out.get("english", [])
            if isinstance(w, str) and WORD.match(w) and w.lower() in raws | {x.lower() for x in re.findall(r"[A-Za-z]{3,32}", committed)}
        }
    )
    japanese = []
    for e in out.get("japanese", []):
        if not isinstance(e, dict):
            continue
        reading, surface = str(e.get("reading", "")), str(e.get("surface", ""))
        # the model must not invent: the surface has to be in what was committed
        if HIRAGANA.match(reading) and surface and surface != reading and len(surface) <= 30 and surface in committed:
            japanese.append((reading, surface))

    print(f"english: {english}")
    print(f"japanese: {japanese}")
    if a.dry_run:
        return 0

    eng_path = os.path.join(a.data, "user_english.txt")
    have = set()
    if os.path.exists(eng_path):
        have = {l.strip().lower() for l in open(eng_path, encoding="utf-8")}
    with open(eng_path, "a", encoding="utf-8") as f:
        for w in english:
            if w not in have:
                f.write(w + "\n")

    dict_dir = os.path.join(a.data, "user_dicts")
    os.makedirs(dict_dir, exist_ok=True)
    dict_path = os.path.join(dict_dir, "auto.txt")
    known = set()
    if os.path.exists(dict_path):
        known = {tuple(l.split("\t")[:2]) for l in open(dict_path, encoding="utf-8") if not l.startswith("#")}
    with open(dict_path, "a", encoding="utf-8") as f:
        for reading, surface in japanese:
            if (reading, surface) not in known:
                f.write(f"{reading}\t{surface}\t固有名詞\tauto\n")

    with open(state, "w") as f:
        f.write(str(newest))
    return 0


if __name__ == "__main__":
    sys.exit(main())
