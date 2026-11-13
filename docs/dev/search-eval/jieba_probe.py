"""BM25 with jieba word-segmentation variants over the same eval set.

Meilisearch's one genuinely different ingredient for Chinese is charabia's
jieba word-level segmentation. This probe extracts exactly that ingredient
and measures it with the existing harness (BM25 + run_eval.evaluate), without
any new infrastructure, to answer: would borrowing the idea (not the service)
improve the product's search?

Usage: python jieba_probe.py
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import jieba  # noqa: E402
from run_eval import BM25, all_tokens, evaluate, tokenize_bigram, tokenize_char, tokenize_words  # noqa: E402


def tokenize_jieba(text):
    return [token.lower() for token in jieba.cut(text) if token.strip()]


def tokenize_jieba_char(text):
    # "jieba replaces bigrams": words + single chars + latin words
    return tokenize_jieba(text) + tokenize_char(text) + tokenize_words(text)


def tokenize_jieba_bigram(text):
    # superset of the current product tokens: words + bigrams + single chars + latin
    return tokenize_jieba(text) + tokenize_bigram(text) + tokenize_char(text) + tokenize_words(text)


def main():
    with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as handle:
        docs = json.load(handle)
    with open(os.path.join(HERE, "queries.json"), encoding="utf-8") as handle:
        queries = json.load(handle)
    with open(os.path.join(HERE, "queries_hard.json"), encoding="utf-8") as handle:
        queries.extend(json.load(handle))

    # Warm the dictionary once so build time is not charged to the first query.
    jieba.initialize()

    methods = {}
    for name, tokenizer in (
        ("bm25_jieba", tokenize_jieba),
        ("bm25_jieba_char", tokenize_jieba_char),
        ("bm25_jieba_bigram", tokenize_jieba_bigram),
        ("bm25_mixed(product)", all_tokens),
    ):
        index = BM25(docs, tokenizer)
        import time

        started = time.perf_counter()
        rankings = {}
        for item in queries:
            rankings[item["query"]] = index.rank(tokenizer(item["query"]))
        latency = (time.perf_counter() - started) / len(queries) * 1000
        metrics, _, negatives = evaluate(queries, rankings, docs)
        metrics["latency_ms"] = round(latency, 2)
        methods[name] = {"metrics": metrics, "negatives": negatives}

    print("| method | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/query |")
    print("|---|---|---|---|---|---|---|---|")
    for name, data in methods.items():
        metrics = data["metrics"]
        print(
            f"| {name} | {metrics['recall@1']:.3f} | {metrics['recall@3']:.3f} | "
            f"{metrics['recall@5']:.3f} | {metrics['mrr@10']:.3f} | "
            f"{metrics['ndcg@10']:.3f} | {metrics['misses']} | {metrics['latency_ms']} |"
        )
    for name in ("bm25_jieba",):
        print(f"negatives ({name}):")
        for item in methods[name]["negatives"]:
            print(f"  {item['query']} -> {', '.join(item['top3']) or '(none)'}")


if __name__ == "__main__":
    main()
