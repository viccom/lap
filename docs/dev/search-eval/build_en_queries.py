"""Translate the 40 annotated captions into short English search queries.

Batched (10 per call, one query per line) because a single 40-item JSON call
timed out on the gateway.

Usage: python build_en_queries.py
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import run_eval  # noqa: E402

BATCH = 10


def translate_batch(docs, offset):
    listing = "\n".join(f"{offset + index + 1}. {doc['text']}" for index, doc in enumerate(docs))
    prompt = (
        "把下面每条中文照片描述改写成一句自然的英文搜索查询，像用户在搜索框里输入的话，"
        "6 到 14 个单词，保留人物、动作、地点和关键物体。\n"
        f"只输出英文查询本身，每行一条，共 {len(docs)} 行，顺序与输入一致，不要编号、不要解释。\n"
        f"输入：\n{listing}"
    )
    data = run_eval.post(
        "/v1/messages",
        {
            "model": run_eval.LLM_MODEL,
            "max_tokens": 1200,
            "system": "只输出查询文本，每行一条。",
            "messages": [{"role": "user", "content": [{"type": "text", "text": prompt}]}],
        },
    )
    text = "".join(
        block.get("text", "") for block in data.get("content", []) if block.get("type") == "text"
    )
    lines = []
    for line in text.splitlines():
        line = re.sub(r"^\s*(?:\d+[.、)]\s*|-\s*)", "", line).strip().strip('"').strip()
        if not line or line.startswith("```"):
            continue
        lines.append(line)
    if len(lines) != len(docs):
        raise SystemExit(f"batch at {offset}: expected {len(docs)} lines, got {len(lines)}:\n{text[:400]}")
    return lines


def main():
    run_eval.KEY = run_eval.api_key()
    with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as handle:
        docs = json.load(handle)

    rows = []
    for start in range(0, len(docs), BATCH):
        batch = docs[start : start + BATCH]
        lines = translate_batch(batch, start)
        for doc, line in zip(batch, lines):
            rows.append({"target": doc["id"], "type": "en", "query": line})
        print(f"{start + len(batch)}/{len(docs)}")

    with open(os.path.join(HERE, "queries_en.json"), "w", encoding="utf-8", newline="\n") as handle:
        json.dump(rows, handle, ensure_ascii=False, indent=1)
        handle.write("\n")
    for row in rows:
        print(f"{row['target']}: {row['query']}")


if __name__ == "__main__":
    main()
