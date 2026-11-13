"""Rebuild embeddings.json (docs + queries) with the gateway embedding model.

Only the embedding cache, so the network cost is one call per batch and the
rerank/expansion caches stay untouched.

Usage: python embed_only.py
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import run_eval  # noqa: E402


def main():
    run_eval.KEY = run_eval.api_key()
    with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as handle:
        docs = json.load(handle)
    with open(os.path.join(HERE, "queries.json"), encoding="utf-8") as handle:
        queries = json.load(handle)
    with open(os.path.join(HERE, "queries_hard.json"), encoding="utf-8") as handle:
        queries.extend(json.load(handle))

    doc_vectors = run_eval.embed_texts([doc["text"] for doc in docs])
    query_vectors = {}
    texts = list(dict.fromkeys(item["query"] for item in queries))
    for start in range(0, len(texts), 16):
        batch = texts[start : start + 16]
        for text, vector in zip(batch, run_eval.embed_texts(batch)):
            query_vectors[text] = vector
        print(f"queries {min(start + 16, len(texts))}/{len(texts)}")

    with open(os.path.join(HERE, "embeddings.json"), "w", encoding="utf-8", newline="\n") as handle:
        json.dump({"docs": doc_vectors, "queries": query_vectors}, handle)
    print(f"docs={len(doc_vectors)} dim={len(doc_vectors[0])} queries={len(query_vectors)}")


if __name__ == "__main__":
    main()
