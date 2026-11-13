# Offline RRF fusion probe over already-collected rank files.
#
# Joins lap's BM25 channel (product_ranks.json, produced by product_replay.mjs
# via search_fts) with the local caption-embedding channel
# (caption_search_local_ranks.json, bge-small-zh, threshold-cut at 0.45),
# then scores Reciprocal Rank Fusion:  rrf(d) = sum_c w_c / (k + rank_c(d))
# against the labeled targets, broken down by query type.
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def load(name):
    return json.loads((HERE / name).read_text(encoding="utf-8"))


def index_by_query(rows):
    out = {}
    for row in rows:
        out[row["query"]] = row
    return out


def rank_of(target, ids):
    try:
        return ids.index(target) + 1
    except ValueError:
        return None


def fuse(channels, k, top_m):
    """channels: list of (ids, weight). Returns fused ranked id list."""
    scores = {}
    for ids, weight in channels:
        for i, fid in enumerate(ids[:top_m]):
            scores[fid] = scores.get(fid, 0.0) + weight / (k + i + 1)
    return [fid for fid, _ in sorted(scores.items(), key=lambda kv: (-kv[1], kv[0]))]


def metrics(ranks):
    """ranks: list of 1-based rank or None. Returns compact metric line."""
    n = len(ranks)
    r1 = sum(1 for r in ranks if r == 1)
    r3 = sum(1 for r in ranks if r is not None and r <= 3)
    r10 = sum(1 for r in ranks if r is not None and r <= 10)
    known = [r for r in ranks if r is not None]
    avg = sum(known) / len(known) if known else float("nan")
    # noise proxy: non-target items ranked above target (capped at 10)
    above = [min(r - 1, 10) if r is not None else 10 for r in ranks]
    return (
        f"R@1={r1}/{n} ({r1 / n:.3f})  R@3={r3}/{n} ({r3 / n:.3f})  "
        f"R@10={r10}/{n} ({r10 / n:.3f})  miss={n - r10}  "
        f"avg_rank={avg:.2f}  noise_above={sum(above) / n:.2f}"
    )


bm25_rows = index_by_query(load("product_ranks.json"))
sem_rows = index_by_query(load("caption_search_local_ranks.json"))

joined = [q for q in sem_rows if q in bm25_rows]
print(f"bm25 rows={len(bm25_rows)}  semantic rows={len(sem_rows)}  joined={len(joined)}")

items = [(q, sem_rows[q]["target"], sem_rows[q]["type"]) for q in joined]

configs = [("bm25 only", [(0, 1)]), ("sem only", [(1, 1)])]
configs += [
    (f"RRF k={k} M={m} 1:1", [(0, 1), (1, 1)])
    for k in (1, 10, 60)
    for m in (10, 20)
]
configs += [
    ("RRF k=60 M=20 sem2x", [(0, 1), (1, 2)]),
    ("RRF k=60 M=20 bm25_2x", [(0, 2), (1, 1)]),
]

by_type = {}
for label, channels in configs:
    ranks = []
    for q, target, qtype in items:
        bm25_ids = bm25_rows[q]["ids"]
        sem_ids = sem_rows[q]["ids"]
        if channels == [(0, 1)]:
            fused = bm25_ids
        elif channels == [(1, 1)]:
            fused = sem_ids
        else:
            weight_by_channel = {0: 1.0, 1: 1.0}
            chans = []
            for idx, w in channels:
                ids = bm25_ids if idx == 0 else sem_ids
                chans.append((ids, w))
            k = int(label.split("k=")[1].split()[0])
            m = int(label.split("M=")[1].split()[0])
            fused = fuse(chans, k, m)
        ranks.append(rank_of(target, fused))
        by_type.setdefault((label, qtype), []).append(rank_of(target, fused))
    print(f"{label:26s} {metrics(ranks)}")

type_counts = {}
for _, _, qtype in items:
    type_counts[qtype] = type_counts.get(qtype, 0) + 1
print("type counts:", "  ".join(f"{t}={c}" for t, c in sorted(type_counts.items())))

print()
print("=== breakdown by query type (all types, selected configs) ===")
all_types = sorted(type_counts)
for label in ("bm25 only", "sem only", "RRF k=60 M=20 1:1", "RRF k=60 M=20 sem2x"):
    for qtype in all_types:
        key = (label, qtype)
        if key in by_type:
            print(f"{label:26s} {qtype:10s} {metrics(by_type[key])}")

print()
print("=== per-query misses / rank>3 (sem2x fusion vs channels) ===")
for q, target, qtype in items:
    bm25_ids = bm25_rows[q]["ids"]
    sem_ids = sem_rows[q]["ids"]
    fused = fuse([(bm25_ids, 1), (sem_ids, 2)], 60, 20)
    r_b = rank_of(target, bm25_ids)
    r_s = rank_of(target, sem_ids)
    r_f = rank_of(target, fused)
    if r_f is None or r_f > 3:
        print(
            f"[{qtype}] target={target} bm25_rank={r_b} sem_rank={r_s} "
            f"fused_rank={r_f}  q={q}"
        )

print()
print("=== positive-only metrics (denominator excludes negative, aligns with 0.936/0.945) ===")
positives = [(q, t, ty) for q, t, ty in items if t is not None]
for label in ("bm25 only", "sem only", "RRF k=60 M=20 1:1", "RRF k=60 M=20 sem2x"):
    ranks = []
    for q, target, _ in positives:
        bm25_ids = bm25_rows[q]["ids"]
        sem_ids = sem_rows[q]["ids"]
        if label == "bm25 only":
            fused = bm25_ids
        elif label == "sem only":
            fused = sem_ids
        else:
            fused = fuse([(bm25_ids, 1), (sem_ids, 2 if "sem2x" in label else 1)], 60, 20)
        ranks.append(rank_of(target, fused))
    print(f"{label:26s} {metrics(ranks)}")

print()
print("=== negative queries: returned-list length per channel (0 = correct reject) ===")
for q, target, qtype in items:
    if target is not None:
        continue
    bm25_ids = bm25_rows[q]["ids"]
    sem_ids = sem_rows[q]["ids"]
    fused1 = fuse([(bm25_ids, 1), (sem_ids, 1)], 60, 20)
    fused2 = fuse([(bm25_ids, 1), (sem_ids, 2)], 60, 20)
    print(
        f"[{qtype}] bm25={len(bm25_ids)}  sem={len(sem_ids)}  "
        f"rrf1:1={len(fused1)}  rrf sem2x={len(fused2)}  q={q}"
    )
