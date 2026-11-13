"""Measure how the product tokenizer (bigram + single char + latin) reacts to
typos, which the existing eval set does not cover.

Two typo families are generated deterministically from the 110 positive
queries:
  - homophone: replace one CJK character (not the first) with a different
    character sharing its pinyin (what a pinyin IME produces);
  - random: replace one CJK character with an unrelated one (worst case);
  - two_typos: two random substitutions.

The ranking mirror is the same BM25 used by run_eval.py (`bm25_mixed`), so the
baseline here (R@1 0.955) is the offline mirror, not the shipped FTS5 command
(0.936). Relative degradation is the signal.

Usage: python typo_probe.py
"""
import json
import os
import random
import sys
import zlib

from pypinyin import lazy_pinyin, Style

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from run_eval import BM25, all_tokens, evaluate  # noqa: E402

CJK_START, CJK_END = "\u4e00", "\u9fff"


def is_cjk(ch):
    return CJK_START <= ch <= CJK_END


def pinyin_of(ch):
    parts = lazy_pinyin(ch, style=Style.NORMAL, errors="ignore")
    return parts[0] if parts else ""


def build_pool(docs, queries):
    chars = set()
    for doc in docs:
        chars.update(ch for ch in doc["text"] if is_cjk(ch))
    for item in queries:
        chars.update(ch for ch in item["query"] if is_cjk(ch))
    return sorted(chars)


def build_homophones(pool):
    table = {}
    for ch in pool:
        py = pinyin_of(ch)
        if py:
            table.setdefault(py, []).append(ch)
    for py, chars in list(table.items()):
        if len(chars) < 2:
            del table[py]
    return table


def typo_homophone(query, pool, homophones, rng):
    positions = [i for i, ch in enumerate(query) if is_cjk(ch) and i > 0]
    rng.shuffle(positions)
    for index in positions:
        ch = query[index]
        candidates = [c for c in homophones.get(pinyin_of(ch), []) if c != ch]
        if candidates:
            replacement = rng.choice(candidates)
            return query[:index] + replacement + query[index + 1 :], f"{ch}->{replacement}"
    return None, None


def typo_random(query, pool, rng, positions_count=1):
    positions = [i for i, ch in enumerate(query) if is_cjk(ch) and i > 0]
    if len(positions) < positions_count:
        return None, None
    chosen = rng.sample(positions, positions_count)
    chars = list(query)
    detail = []
    for index in chosen:
        old = chars[index]
        new = old
        while new == old:
            new = rng.choice(pool)
        chars[index] = new
        detail.append(f"{old}->{new}")
    return "".join(chars), ",".join(detail)


def main():
    with open(os.path.join(HERE, "corpus.json"), encoding="utf-8") as handle:
        docs = json.load(handle)
    with open(os.path.join(HERE, "queries.json"), encoding="utf-8") as handle:
        queries = json.load(handle)
    with open(os.path.join(HERE, "queries_hard.json"), encoding="utf-8") as handle:
        queries.extend(json.load(handle))

    positives = [item for item in queries if item.get("target") is not None]
    pool = build_pool(docs, queries)
    homophones = build_homophones(pool)

    index = BM25(docs, all_tokens)

    def rank_all(items):
        rankings = {}
        for item in items:
            rankings[item["query"]] = index.rank(all_tokens(item["query"]))
        return rankings

    base_rankings = rank_all(positives)
    base_metrics, _, _ = evaluate(queries, base_rankings, docs)

    variants = {"baseline": (positives, base_rankings)}
    generated = {"homophone": [], "random": [], "two_typos": []}
    details = {"homophone": [], "random": [], "two_typos": []}
    for item in positives:
        rng = random.Random(zlib.crc32(item["query"].encode("utf-8")))
        mutated, why = typo_homophone(item["query"], pool, homophones, rng)
        if mutated:
            generated["homophone"].append({**item, "query": mutated})
            details["homophone"].append((item["query"], mutated, why))
        rng = random.Random(zlib.crc32(item["query"].encode("utf-8")) + 1)
        mutated, why = typo_random(item["query"], pool, rng)
        if mutated:
            generated["random"].append({**item, "query": mutated})
            details["random"].append((item["query"], mutated, why))
        rng = random.Random(zlib.crc32(item["query"].encode("utf-8")) + 2)
        mutated, why = typo_random(item["query"], pool, rng, positions_count=2)
        if mutated:
            generated["two_typos"].append({**item, "query": mutated})

    for name, items in generated.items():
        variants[name] = (items, rank_all(items))

    print("| variant | n | R@1 | R@3 | R@5 | MRR@10 | misses |")
    print("|---|---|---|---|---|---|---|")
    for name, (items, rankings) in variants.items():
        metrics, _, _ = evaluate(items, rankings, docs)
        print(
            f"| {name} | {len(items)} | {metrics['recall@1']:.3f} | "
            f"{metrics['recall@3']:.3f} | {metrics['recall@5']:.3f} | "
            f"{metrics['mrr@10']:.3f} | {metrics['misses']} |"
        )

    # Per-type view for the homophone family (the realistic one).
    for family in ("homophone", "random"):
        items, rankings = variants[family]
        print(f"\nper type ({family}):")
        for kind in sorted({item["type"] for item in items}):
            subset = [item for item in items if item["type"] == kind]
            metrics, _, _ = evaluate(subset, rankings, docs)
            print(
                f"  {kind}: n={len(subset)} R@1={metrics['recall@1']:.3f} "
                f"R@3={metrics['recall@3']:.3f} misses={metrics['misses']}"
            )

    print("\nhomophone samples:")
    for original, mutated, why in details["homophone"][:8]:
        print(f"  {original}\n    -> {mutated}   ({why})")
    print("\nrandom samples:")
    for original, mutated, why in details["random"][:6]:
        print(f"  {original}\n    -> {mutated}   ({why})")


if __name__ == "__main__":
    main()
