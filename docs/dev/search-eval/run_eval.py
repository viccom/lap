"""Run the Lap annotation-search evaluation.

Compares the current FTS logic against BM25, character n-grams, dense
embeddings (gateway `zhipu/embedding-3`), RRF fusion, reranking
(`zhipu/rerank`) and LLM query expansion. Writes `results.json` and prints a
markdown table.

Usage:
    python run_eval.py [--limit N] [--skip-network]
"""
import argparse
import json
import math
import os
import re
import statistics
import time
import urllib.error
import urllib.request
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
CORPUS = os.path.join(HERE, "corpus.json")
QUERIES = os.path.join(HERE, "queries.json")
HARD_QUERIES = os.path.join(HERE, "queries_hard.json")
RESULTS = os.path.join(HERE, "results.json")
EMBED_CACHE = os.path.join(HERE, "embeddings.json")
EXPAND_CACHE = os.path.join(HERE, "expansions.json")
RERANK_CACHE = os.path.join(HERE, "reranks.json")

DEFAULT_DB = os.path.join(
    os.environ.get("LOCALAPPDATA", ""), "com.julyx10.lap.debug", "libraries", "default.db"
)
GATEWAY = os.environ.get("LAP_VL_GATEWAY_URL", "http://192.168.199.8:3425").rstrip("/")
EMBED_MODEL = "zhipu/embedding-3"
RERANK_MODEL = "zhipu/rerank"
LLM_MODEL = "auto/deepseek-v4.1-flash"
RRF_K = 60

CJK = re.compile(r"[\u1100-\u11FF\u3040-\u30FF\u3105-\u312F\u3130-\u318F\u31C0-\u31FF\u3400-\u4DBF\u4E00-\u9FFF\uAC00-\uD7A3\uF900-\uFAFF\uFF66-\uFF9F]")


def api_key():
    key = os.environ.get("LAP_VL_API_KEY", "").strip()
    if key:
        return key
    sidecar = os.path.join(os.path.dirname(os.path.dirname(DEFAULT_DB)), "vl-api-key")
    with open(sidecar, encoding="utf-8") as handle:
        return handle.read().strip()


KEY = None


def post(path, payload, timeout=180):
    # Native UTF-8, not \uXXXX escapes: the upstream WAF intermittently
    # answers escaped CJK payloads with 405 (reproduced 2026-11-12).
    body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
    last_error = None
    for attempt in range(4):
        request = urllib.request.Request(
            f"{GATEWAY}{path}",
            data=body,
            headers={"x-api-key": KEY, "content-type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            last_error = error
            if error.code not in (405, 429, 500, 502, 503, 504):
                raise
            time.sleep(2 * (attempt + 1))
    raise last_error


# ----------------------------------------------------------------------------
# Tokenizers (mirroring t_fts.rs)
# ----------------------------------------------------------------------------

def tokenize_bigram(text):
    tokens, cjk, word = [], [], []

    def flush_cjk():
        if len(cjk) == 1:
            tokens.append(cjk[0])
        else:
            tokens.extend("".join(cjk[i : i + 2]) for i in range(len(cjk) - 1))
        cjk.clear()

    def flush_word():
        if word:
            tokens.append("".join(word))
            word.clear()

    for ch in text:
        if CJK.match(ch):
            flush_word()
            cjk.append(ch)
        elif ch.isalnum():
            flush_cjk()
            word.append(ch.lower())
        else:
            flush_cjk()
            flush_word()
    flush_cjk()
    flush_word()
    return tokens


def tokenize_char(text):
    tokens = []
    for ch in text:
        if CJK.match(ch):
            tokens.append(ch)
        elif ch.isalnum():
            pass
    return tokens


def tokenize_words(text):
    return [w.lower() for w in re.findall(r"[A-Za-z0-9]+", text)]


def all_tokens(text):
    return tokenize_bigram(text) + tokenize_char(text) + tokenize_words(text)


# ----------------------------------------------------------------------------
# Methods
# ----------------------------------------------------------------------------

class BM25:
    def __init__(self, docs, tokenizer, k1=1.2, b=0.75):
        self.ids = [doc["id"] for doc in docs]
        self.docs = [tokenizer(doc["text"]) for doc in docs]
        self.k1, self.b = k1, b
        self.df = Counter()
        for tokens in self.docs:
            for token in set(tokens):
                self.df[token] += 1
        self.n = len(self.docs)
        self.avgdl = statistics.mean(len(tokens) for tokens in self.docs) or 1.0
        self.tfs = [Counter(tokens) for tokens in self.docs]

    def idf(self, token):
        df = self.df.get(token, 0)
        return math.log(1 + (self.n - df + 0.5) / (df + 0.5))

    def rank(self, tokens):
        tokens = list(dict.fromkeys(tokens))
        scores = []
        for index, tf in enumerate(self.tfs):
            score = 0.0
            dl = len(self.docs[index])
            for token in tokens:
                freq = tf.get(token, 0)
                if not freq:
                    continue
                norm = freq * (self.k1 + 1) / (freq + self.k1 * (1 - self.b + self.b * dl / self.avgdl))
                score += self.idf(token) * norm
            scores.append((score, index))
        scores.sort(key=lambda item: (-item[0], item[1]))
        return [self.ids[index] for score, index in scores if score > 0]


def dice_rank(docs, query):
    query_set = set(tokenize_bigram(query)) | set(tokenize_char(query))
    scored = []
    for index, doc in enumerate(docs):
        doc_set = set(tokenize_bigram(doc["text"])) | set(tokenize_char(doc["text"]))
        inter = len(query_set & doc_set)
        score = 2 * inter / (len(query_set) + len(doc_set)) if query_set and doc_set else 0
        scored.append((score, index))
    scored.sort(key=lambda item: (-item[0], item[1]))
    return [docs[index]["id"] for score, index in scored if score > 0]


def current_fts_rank(docs, query):
    """The product logic: all bigrams, else all characters."""
    bigrams = tokenize_bigram(query)
    chars = tokenize_char(query)
    doc_tokens = [set(tokenize_bigram(doc["text"])) for doc in docs]

    strict = [i for i, tokens in enumerate(doc_tokens) if bigrams and all(token in tokens for token in bigrams)]
    if strict:
        # The product ranks by matched columns; with one text field every
        # strict hit ties, so the fallback order is the document order.
        return [docs[i]["id"] for i in strict]

    relaxed = [
        i
        for i, doc in enumerate(docs)
        if chars and all(ch in set(tokenize_char(doc["text"])) for ch in chars)
    ]
    return [docs[i]["id"] for i in relaxed]


def rrf(*rankings, k=RRF_K, weights=None):
    scores = {}
    for weight_index, ranking in enumerate(rankings):
        weight = 1.0 if not weights else weights[weight_index]
        for rank, doc_id in enumerate(ranking, start=1):
            scores[doc_id] = scores.get(doc_id, 0.0) + weight / (k + rank)
    return [doc_id for doc_id, _ in sorted(scores.items(), key=lambda item: -item[1])]


# ----------------------------------------------------------------------------
# Network-backed signals
# ----------------------------------------------------------------------------

def embed_texts(texts):
    vectors = []
    for start in range(0, len(texts), 16):
        batch = texts[start : start + 16]
        try:
            data = post("/v1/embeddings", {"model": EMBED_MODEL, "input": batch})
            vectors.extend(item["embedding"] for item in data["data"])
            continue
        except Exception as error:  # noqa: BLE001 - fall back to single inputs
            print(f"embedding batch at {start} failed ({error}); retrying one by one")
        for text in batch:
            data = post("/v1/embeddings", {"model": EMBED_MODEL, "input": [text]})
            vectors.extend(item["embedding"] for item in data["data"])
            time.sleep(0.2)
    return vectors


def cosine(a, b):
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    return dot / (na * nb) if na and nb else 0.0


def dense_rank(docs, doc_vectors, query_vector):
    scored = [
        (cosine(query_vector, vector), index)
        for index, vector in enumerate(doc_vectors)
    ]
    scored.sort(key=lambda item: (-item[0], item[1]))
    return [docs[index]["id"] for _, index in scored]


def rerank(docs, ranking, query, top_k=10):
    by_id = {doc["id"]: doc for doc in docs}
    candidates = ranking[:top_k]
    if not candidates:
        return ranking
    documents = [by_id[doc_id]["text"] for doc_id in candidates]
    data = post("/v1/rerank", {"model": RERANK_MODEL, "query": query, "documents": documents})
    scored = []
    for item in data.get("results", []):
        index = int(item["index"])
        scored.append((float(item.get("relevance_score", 0)), index))
    scored.sort(key=lambda item: (-item[0], item[1]))
    reranked = [candidates[index] for _, index in scored]
    return reranked + ranking[top_k:]


def expand_queries(queries, batch_size=12):
    expansions = {}
    for start in range(0, len(queries), batch_size):
        batch = [item["query"] for item in queries[start : start + batch_size]]
        prompt = (
            "为下面每个搜索查询各写 4 个中文同义词或相关检索词（单词或短语），"
            "用于提高关键词检索召回。输出 JSON 对象 {查询原文: [词1,词2,词3,词4]}，不要解释。\n"
            + "\n".join(f"- {query}" for query in batch)
        )
        body = {
            "model": LLM_MODEL,
            "max_tokens": 1500,
            "system": "只输出 JSON，不要解释。",
            "messages": [{"role": "user", "content": [{"type": "text", "text": prompt}]}],
        }
        try:
            data = post("/v1/messages", body)
            text = "".join(
                block.get("text", "") for block in data.get("content", []) if block.get("type") == "text"
            ).strip()
            if text.startswith("```"):
                text = text.split("\n", 1)[-1].rsplit("```", 1)[0]
            parsed = json.loads(text)
            if isinstance(parsed, dict):
                for query, terms in parsed.items():
                    if isinstance(terms, list):
                        expansions[query] = [str(term) for term in terms if str(term).strip()]
        except Exception as error:  # noqa: BLE001 - the method is optional
            print(f"expansion batch failed: {error}")
    return expansions


# ----------------------------------------------------------------------------
# Evaluation
# ----------------------------------------------------------------------------

def evaluate(queries, rankings, docs):
    by_id = {doc["id"]: doc for doc in docs}
    negatives = [item for item in queries if item.get("target") is None]
    positives = [item for item in queries if item.get("target") is not None]
    per_query = []
    metrics = {}
    ranks = []
    for item in positives:
        ranking = rankings.get(item["query"], [])
        target = int(item["target"])
        rank = ranking.index(target) + 1 if target in ranking else None
        ranks.append(rank)
        per_query.append(
            {
                "query": item["query"],
                "type": item["type"],
                "target": target,
                "rank": rank,
                "top3": [by_id.get(doc_id, {}).get("name", str(doc_id)) for doc_id in ranking[:3]],
            }
        )
    total = len(positives)
    metrics["recall@1"] = sum(1 for rank in ranks if rank == 1) / total if total else 0
    metrics["recall@3"] = sum(1 for rank in ranks if rank and rank <= 3) / total if total else 0
    metrics["recall@5"] = sum(1 for rank in ranks if rank and rank <= 5) / total if total else 0
    metrics["mrr@10"] = sum(1 / rank for rank in ranks if rank and rank <= 10) / total if total else 0
    metrics["ndcg@10"] = (
        sum(1 / math.log2(rank + 1) for rank in ranks if rank and rank <= 10) / total if total else 0
    )
    metrics["misses"] = sum(1 for rank in ranks if rank is None)
    negative_hits = []
    for item in negatives:
        ranking = rankings.get(item["query"], [])
        negative_hits.append(
            {
                "query": item["query"],
                "top3": [by_id.get(doc_id, {}).get("name", str(doc_id)) for doc_id in ranking[:3]],
            }
        )
    return metrics, per_query, negative_hits


def main():
    global KEY
    parser = argparse.ArgumentParser()
    parser.add_argument("--limit", type=int, default=0)
    parser.add_argument("--skip-network", action="store_true")
    args = parser.parse_args()
    KEY = api_key()

    with open(CORPUS, encoding="utf-8") as handle:
        docs = json.load(handle)
    with open(QUERIES, encoding="utf-8") as handle:
        queries = json.load(handle)
    if os.path.exists(HARD_QUERIES):
        with open(HARD_QUERIES, encoding="utf-8") as handle:
            queries.extend(json.load(handle))
    if args.limit:
        queries = queries[: args.limit]

    methods = {}

    def timed(name, fn):
        start = time.perf_counter()
        rankings = {}
        for item in queries:
            rankings[item["query"]] = fn(item["query"])
        elapsed = (time.perf_counter() - start) / max(1, len(queries))
        methods[name] = {"rankings": rankings, "latency_ms": round(elapsed * 1000, 2)}

    timed("current_fts", lambda query: current_fts_rank(docs, query))

    bm25_bigram = BM25(docs, tokenize_bigram)
    bm25_char = BM25(docs, tokenize_char)
    bm25_mixed = BM25(docs, all_tokens)
    timed("bm25_bigram", lambda query: bm25_bigram.rank(tokenize_bigram(query)))
    timed("bm25_char", lambda query: bm25_char.rank(tokenize_char(query)))
    timed("bm25_mixed", lambda query: bm25_mixed.rank(all_tokens(query)))
    timed("dice", lambda query: dice_rank(docs, query))

    network = {}
    if not args.skip_network:
        # Document embeddings.
        cache = {}
        if os.path.exists(EMBED_CACHE):
            with open(EMBED_CACHE, encoding="utf-8") as handle:
                cache = json.load(handle)
        doc_vectors = cache.get("docs")
        if not doc_vectors or len(doc_vectors) != len(docs):
            doc_vectors = embed_texts([doc["text"] for doc in docs])
            cache["docs"] = doc_vectors
            with open(EMBED_CACHE, "w", encoding="utf-8", newline="\n") as handle:
                json.dump(cache, handle)
        query_cache = cache.setdefault("queries", {})
        missing = [item["query"] for item in queries if item["query"] not in query_cache]
        if missing:
            for query, vector in zip(missing, embed_texts(missing)):
                query_cache[query] = vector
            with open(EMBED_CACHE, "w", encoding="utf-8", newline="\n") as handle:
                json.dump(cache, handle)

        def dense(query):
            return dense_rank(docs, doc_vectors, query_cache[query])

        timed("dense", dense)
        timed("rrf_bm25_dense", lambda query: rrf(bm25_mixed.rank(all_tokens(query)), dense(query)))
        timed(
            "rrf_bigram_dense",
            lambda query: rrf(bm25_bigram.rank(tokenize_bigram(query)), dense(query)),
        )

        # Rerank the dense top-10 (cache per query).
        rerank_cache = {}
        if os.path.exists(RERANK_CACHE):
            with open(RERANK_CACHE, encoding="utf-8") as handle:
                rerank_cache = json.load(handle)
        start = time.perf_counter()
        reranked_dense = {}
        for item in queries:
            query = item["query"]
            base_dense = methods["dense"]["rankings"][query]
            key = f"{query}||dense"
            if key not in rerank_cache:
                rerank_cache[key] = rerank(docs, base_dense, query)
            reranked_dense[query] = rerank_cache[key]
        elapsed = (time.perf_counter() - start) / max(1, len(queries))
        methods["dense_rerank"] = {"rankings": reranked_dense, "latency_ms": round(elapsed * 1000, 2)}
        with open(RERANK_CACHE, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(rerank_cache, handle)

        # LLM query expansion + BM25.
        expansions = {}
        if os.path.exists(EXPAND_CACHE):
            with open(EXPAND_CACHE, encoding="utf-8") as handle:
                expansions = json.load(handle)
        missing = [item for item in queries if item["query"] not in expansions]
        if missing:
            expansions.update(expand_queries(missing))
            with open(EXPAND_CACHE, "w", encoding="utf-8", newline="\n") as handle:
                json.dump(expansions, handle, ensure_ascii=False, indent=2)
        timed(
            "expand_bm25",
            lambda query: bm25_mixed.rank(all_tokens(query) + [term for term in expansions.get(query, []) for term in tokenize_bigram(term) + tokenize_char(term)]),
        )
        network = {"embed_model": EMBED_MODEL, "rerank_model": RERANK_MODEL, "llm_model": LLM_MODEL}

    summary = {}
    details = {}
    for name, data in methods.items():
        metrics, per_query, negatives = evaluate(queries, data["rankings"], docs)
        metrics["latency_ms"] = data["latency_ms"]
        summary[name] = metrics
        details[name] = {"per_query": per_query, "negatives": negatives}

    order = ["current_fts", "bm25_bigram", "bm25_char", "bm25_mixed", "dice"]
    if not args.skip_network:
        order += ["dense", "rrf_bigram_dense", "rrf_bm25_dense", "dense_rerank", "expand_bm25"]
    print(f"corpus={len(docs)} queries={len(queries)} network={bool(network)}")
    header = "| method | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/query |"
    print(header)
    print("|---|---|---|---|---|---|---|---|")
    for name in order:
        if name not in summary:
            continue
        metrics = summary[name]
        print(
            f"| {name} | {metrics['recall@1']:.3f} | {metrics['recall@3']:.3f} | "
            f"{metrics['recall@5']:.3f} | {metrics['mrr@10']:.3f} | {metrics['ndcg@10']:.3f} | "
            f"{metrics['misses']} | {metrics['latency_ms']:.1f} |"
        )

    with open(RESULTS, "w", encoding="utf-8", newline="\n") as handle:
        json.dump(
            {
                "corpus_size": len(docs),
                "query_count": len(queries),
                "network": network,
                "summary": summary,
                "details": details,
            },
            handle,
            ensure_ascii=False,
            indent=2,
        )
        handle.write("\n")
    print(f"results -> {RESULTS}")


if __name__ == "__main__":
    main()
