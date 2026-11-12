"""Generate hard paraphrase queries that deliberately avoid the caption words.

The first evaluation set was written by an LLM that could reuse caption nouns,
which flatters lexical retrieval. These queries forbid reusing any two-character
run from the caption, so only synonyms / hypernyms / periphrases remain.
"""
import io
import json
import os
import time
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "corpus.json")
OUT = os.path.join(HERE, "queries_hard.json")
GATEWAY = os.environ.get("LAP_VL_GATEWAY_URL", "http://192.168.199.8:3425").rstrip("/")
MODEL = "auto/deepseek-v4.1-flash"
DEFAULT_DB = os.path.join(
    os.environ.get("LOCALAPPDATA", ""), "com.julyx10.lap.debug", "libraries", "default.db"
)


def key():
    value = os.environ.get("LAP_VL_API_KEY", "").strip()
    if value:
        return value
    with open(os.path.join(os.path.dirname(os.path.dirname(DEFAULT_DB)), "vl-api-key"), encoding="utf-8") as handle:
        return handle.read().strip()


def call(prompt):
    body = json.dumps(
        {
            "model": MODEL,
            "max_tokens": 1500,
            "system": "只输出 JSON，不要解释。",
            "messages": [{"role": "user", "content": [{"type": "text", "text": prompt}]}],
        },
        ensure_ascii=False,
    ).encode("utf-8")
    request = urllib.request.Request(
        f"{GATEWAY}/v1/messages",
        data=body,
        headers={"x-api-key": key(), "anthropic-version": "2023-06-01", "content-type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        data = json.load(response)
    return "".join(block.get("text", "") for block in data.get("content", []) if block.get("type") == "text").strip()


def strip_fence(text):
    text = text.strip()
    if text.startswith("```"):
        text = text.split("\n", 1)[-1]
    if text.rstrip().endswith("```"):
        text = text.rstrip()[:-3]
    return text.strip()


def main():
    with io.open(CORPUS, encoding="utf-8") as handle:
        docs = json.load(handle)
    queries = []
    for start in range(0, len(docs), 3):
        batch = docs[start : start + 3]
        listing = "\n".join(f"[{doc['id']}] {doc['text']}" for doc in batch)
        prompt = (
            "为下面每张照片各写 1 条中文搜索查询，要求：\n"
            "1) 不得复用描述中出现的任何连续两个字（人名/品牌除外），只能用同义词、上位词、俗称或换一种生活化说法；\n"
            "2) 查询要能唯一指向该照片，包含 1-2 个可辨识细节；\n"
            "3) 像普通用户口述，不要罗列关键词。\n"
            "输出 JSON 数组 [{\"target\": id, \"query\": \"...\"}]，不要解释。\n\n" + listing
        )
        parsed = None
        for attempt in range(2):
            try:
                parsed = json.loads(strip_fence(call(prompt)))
                break
            except Exception as error:  # noqa: BLE001
                print("batch", start, "attempt", attempt + 1, "failed:", error)
                time.sleep(3)
        if not isinstance(parsed, list):
            continue
        ids = {doc["id"] for doc in batch}
        for item in parsed:
            if item.get("target") in ids and str(item.get("query", "")).strip():
                queries.append({"target": int(item["target"]), "type": "hard", "query": str(item["query"]).strip()})
    with io.open(OUT, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(queries, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    print(f"hard queries: {len(queries)} -> {OUT}")


if __name__ == "__main__":
    main()
