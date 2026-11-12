"""Build the search evaluation set for the Lap annotation corpus.

Reads the annotated photos from a Lap library, writes `corpus.json`, and (with
`--generate`) asks the configured gateway to write keyword / paraphrase /
sentence queries for every caption. The hand-written regression queries at the
bottom come from the user's real reports.

Usage:
    python build_eval_set.py --generate
Environment:
    LAP_LIBRARY_DB   path to default.db (default: debug library)
    LAP_VL_GATEWAY_URL / LAP_VL_API_KEY   optional overrides; the key falls
    back to the vl-api-key sidecar next to the library.
"""
import argparse
import json
import os
import sqlite3
import time
import urllib.request

DEFAULT_DB = os.path.join(
    os.environ.get("LOCALAPPDATA", ""),
    "com.julyx10.lap.debug",
    "libraries",
    "default.db",
)
HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "corpus.json")
QUERIES = os.path.join(HERE, "queries.json")
MODEL = "auto/deepseek-v4.1-flash"

# The user's reports: two synonym cases and two negatives.
HAND_WRITTEN = [
    {"target": 101, "type": "user-synonym", "query": "一个小男孩躺在草地上"},
    {"target": 1, "type": "user-synonym", "query": "摘葡萄的小男孩"},
    {"target": 101, "type": "user-keyword", "query": "草地上玩滑板车的孩子"},
    {"target": 1, "type": "user-keyword", "query": "葡萄园采摘"},
    {"target": None, "type": "negative", "query": "摘葡萄的小女孩"},
    {"target": None, "type": "negative", "query": "飞机在跑道上起飞"},
    {"target": None, "type": "negative", "query": "海边冲浪的女孩"},
]


def gateway_config():
    base = os.environ.get("LAP_VL_GATEWAY_URL", "http://192.168.199.8:3425").rstrip("/")
    key = os.environ.get("LAP_VL_API_KEY", "").strip()
    if not key:
        sidecar = os.path.join(os.path.dirname(os.path.dirname(DEFAULT_DB)), "vl-api-key")
        with open(sidecar, encoding="utf-8") as handle:
            key = handle.read().strip()
    return base, key


def load_corpus(db_path):
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True)
    rows = conn.execute(
        """
        SELECT v.file_id, a.name, v.result_json
        FROM vl_annotations v JOIN afiles a ON a.id = v.file_id
        WHERE v.template_id = 1 AND v.template_version = 1
        ORDER BY v.file_id
        """
    ).fetchall()
    conn.close()
    docs = []
    for file_id, name, result_json in rows:
        try:
            data = json.loads(result_json)
        except json.JSONDecodeError:
            continue
        parts = []
        for value in data.values():
            if isinstance(value, str):
                parts.append(value)
            elif isinstance(value, list):
                parts.extend(str(item) for item in value if isinstance(item, str))
        text = " ".join(part.strip() for part in parts if part.strip())
        if text:
            docs.append({"id": int(file_id), "name": name, "text": text})
    return docs


def call_messages(base, key, prompt, max_tokens=2000):
    body = json.dumps(
        {
            "model": MODEL,
            "max_tokens": max_tokens,
            "system": "只输出 JSON，不要解释。",
            "messages": [{"role": "user", "content": [{"type": "text", "text": prompt}]}],
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        f"{base}/v1/messages",
        data=body,
        headers={
            "x-api-key": key,
            "anthropic-version": "2023-06-01",
            "content-type": "application/json",
        },
    )
    with urllib.request.urlopen(request, timeout=180) as response:
        payload = json.load(response)
    texts = [block.get("text", "") for block in payload.get("content", []) if block.get("type") == "text"]
    return "".join(texts).strip()


def strip_fence(text):
    text = text.strip()
    if text.startswith("```"):
        first, _, rest = text.partition("\n")
        if rest:
            text = rest
        if text.rstrip().endswith("```"):
            text = text.rstrip()[:-3]
    return text.strip()


def generate_queries(docs, base, key, batch_size=4):
    queries = []
    for start in range(0, len(docs), batch_size):
        batch = docs[start : start + batch_size]
        listing = "\n".join(f"[{doc['id']}] {doc['text']}" for doc in batch)
        prompt = (
            "你是搜索评测集的作者。为下面每张照片的标注描述生成 3 条中文搜索查询：\n"
            "A 关键词（2-4 个词，取描述里的独特实体/动作）；\n"
            "B 同义改写（换一种说法，使用同义词或不同词序，不要照抄原文）；\n"
            "C 自然句子（像用户口述想找的照片，包含 1-2 个细节）。\n"
            "要求：查询必须能唯一指向该照片；不要出现文件名；不要提及其他照片。\n"
            "输出 JSON 数组，每项 {\"target\": id, \"type\": \"keyword|paraphrase|sentence\", \"query\": \"...\"}，不要解释。\n\n"
            f"{listing}"
        )
        parsed = None
        for attempt in range(2):
            try:
                parsed = json.loads(strip_fence(call_messages(base, key, prompt)))
                break
            except Exception as error:  # noqa: BLE001 - retried once, then reported
                print(f"batch {start}: attempt {attempt + 1} failed: {error}")
                time.sleep(2)
        if not isinstance(parsed, list):
            print(f"batch {start}: skipped, model did not return a JSON array")
            continue
        for item in parsed:
            target = item.get("target")
            query = str(item.get("query", "")).strip()
            kind = item.get("type", "")
            if target in [doc["id"] for doc in batch] and query and kind in ("keyword", "paraphrase", "sentence"):
                queries.append({"target": int(target), "type": kind, "query": query})
        print(f"batch {start}: {len(queries)} queries so far")
    queries.extend(HAND_WRITTEN)
    # Deduplicate identical query strings, keeping the first label.
    seen = set()
    unique = []
    for item in queries:
        if item["query"] in seen:
            continue
        seen.add(item["query"])
        unique.append(item)
    return unique


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--db", default=os.environ.get("LAP_LIBRARY_DB", DEFAULT_DB))
    parser.add_argument("--generate", action="store_true", help="generate queries with the gateway")
    args = parser.parse_args()

    docs = load_corpus(args.db)
    with open(CORPUS, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(docs, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    print(f"corpus: {len(docs)} documents -> {CORPUS}")

    if args.generate:
        base, key = gateway_config()
        queries = generate_queries(docs, base, key)
        with open(QUERIES, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(queries, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        print(f"queries: {len(queries)} -> {QUERIES}")


if __name__ == "__main__":
    main()
