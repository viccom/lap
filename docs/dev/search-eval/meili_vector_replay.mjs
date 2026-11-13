// Replay the Lap eval set through Meilisearch's VECTOR and HYBRID channels
// (the part the keyword-only replay did not cover).
//
// Embeddings come from embeddings.json (zhipu/embedding-3, 2048 dims), fed to
// Meilisearch as a `userProvided` embedder — the same shape as the official
// Flickr demo, where an external multimodal model produces the vectors and
// Meilisearch only stores/searches them.
//
// Passes:
//   keyword   q only                       (reference; equals the earlier replay)
//   vector    vector only, semanticRatio 1 (pure semantic)
//   hybrid    q + vector, semanticRatio .5 (Meilisearch fusion)
//
// Usage: node meili_vector_replay.mjs [--url http://127.0.0.1:7700]
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.argv.includes('--url')
  ? process.argv[process.argv.indexOf('--url') + 1]
  : 'http://127.0.0.1:7700';
const INDEX = 'lap-vec';
const EMBEDDER = 'lap';

const load = file => JSON.parse(fs.readFileSync(path.join(HERE, file), 'utf8'));
const corpus = load('corpus.json');
const queries = [...load('queries.json'), ...load('queries_hard.json')];
const embeddings = load('embeddings.json');
const byId = Object.fromEntries(corpus.map(doc => [doc.id, doc.name]));
const dims = embeddings.docs[0].length;

const api = async (method, urlPath, body) => {
  const res = await fetch(`${BASE}${urlPath}`, {
    method,
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(`${method} ${urlPath} -> ${res.status} ${JSON.stringify(data).slice(0, 300)}`);
  }
  return data;
};

const waitTask = async taskUid => {
  for (;;) {
    const task = await api('GET', `/tasks/${taskUid}`);
    if (task.status === 'succeeded') return task;
    if (task.status === 'failed') {
      throw new Error(`task ${taskUid} failed: ${JSON.stringify(task.error).slice(0, 500)}`);
    }
    await new Promise(r => setTimeout(r, 60));
  }
};

let up = false;
for (let i = 0; i < 120 && !up; i++) {
  try {
    await api('GET', '/health');
    up = true;
  } catch {
    await new Promise(r => setTimeout(r, 500));
  }
}
if (!up) throw new Error(`meilisearch not reachable at ${BASE}`);

try {
  const t = await api('DELETE', `/indexes/${INDEX}`);
  if (t?.taskUid != null) await waitTask(t.taskUid);
} catch {
  // fresh index
}
await waitTask((await api('POST', '/indexes', { uid: INDEX, primaryKey: 'id' })).taskUid);
await waitTask(
  (await api('PATCH', `/indexes/${INDEX}/settings`, {
    searchableAttributes: ['text', 'name'],
    embedders: { [EMBEDDER]: { source: 'userProvided', dimensions: dims } },
  })).taskUid,
);

const documents = corpus.map((doc, index) => ({
  ...doc,
  _vectors: { [EMBEDDER]: embeddings.docs[index] },
}));
const indexStarted = Date.now();
await waitTask((await api('POST', `/indexes/${INDEX}/documents`, documents)).taskUid);
const indexMs = Date.now() - indexStarted;
console.log(`indexed ${documents.length} docs with ${dims}-dim vectors in ${indexMs}ms`);

const evaluate = rankings => {
  const positives = queries.filter(item => item.target != null);
  const total = positives.length || 1;
  const ranks = positives.map(item => {
    const list = rankings[item.query] || [];
    const at = list.indexOf(Number(item.target));
    return at < 0 ? null : at + 1;
  });
  const byType = {};
  for (const type of [...new Set(positives.map(item => item.type))]) {
    const subset = positives.filter(item => item.type === type);
    const subsetRanks = subset.map(item => {
      const list = rankings[item.query] || [];
      const at = list.indexOf(Number(item.target));
      return at < 0 ? null : at + 1;
    });
    byType[type] = {
      n: subset.length,
      'R@1': subsetRanks.filter(r => r === 1).length / subset.length,
      'R@3': subsetRanks.filter(r => r && r <= 3).length / subset.length,
      misses: subsetRanks.filter(r => r == null).length,
    };
  }
  return {
    'R@1': ranks.filter(r => r === 1).length / total,
    'R@3': ranks.filter(r => r && r <= 3).length / total,
    'R@5': ranks.filter(r => r && r <= 5).length / total,
    'MRR@10': ranks.reduce((sum, r) => sum + (r && r <= 10 ? 1 / r : 0), 0) / total,
    'nDCG@10': ranks.reduce((sum, r) => sum + (r && r <= 10 ? 1 / Math.log2(r + 1) : 0), 0) / total,
    misses: ranks.filter(r => r == null).length,
    byType,
  };
};

const replay = async bodyFor => {
  const rows = [];
  for (const item of queries) {
    const started = Date.now();
    const res = await api('POST', `/indexes/${INDEX}/search`, {
      limit: 50,
      attributesToRetrieve: ['id'],
      ...bodyFor(item),
    });
    rows.push({
      query: item.query,
      target: item.target,
      type: item.type,
      ids: res.hits.map(hit => Number(hit.id)),
      ms: Date.now() - started,
    });
  }
  const rankings = {};
  for (const row of rows) rankings[row.query] = row.ids;
  return { rows, rankings, metrics: evaluate(rankings) };
};

const passes = {};
passes.keyword = await replay(item => ({ q: item.query }));
passes.vector = await replay(item => ({
  vector: embeddings.queries[item.query],
  hybrid: { embedder: EMBEDDER, semanticRatio: 1.0 },
}));
passes.hybrid = await replay(item => ({
  q: item.query,
  vector: embeddings.queries[item.query],
  hybrid: { embedder: EMBEDDER, semanticRatio: 0.5 },
}));

console.log(`corpus=${corpus.length} queries=${queries.length} positives=${queries.filter(q => q.target != null).length}`);
console.log('| pass | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/query |');
console.log('|---|---|---|---|---|---|---|---|');
for (const [name, pass] of Object.entries(passes)) {
  const m = pass.metrics;
  const ms = Math.round(pass.rows.reduce((s, r) => s + r.ms, 0) / pass.rows.length);
  console.log(
    `| meili ${name} | ${m['R@1'].toFixed(3)} | ${m['R@3'].toFixed(3)} | `
    + `${m['R@5'].toFixed(3)} | ${m['MRR@10'].toFixed(3)} | ${m['nDCG@10'].toFixed(3)} | `
    + `${m.misses} | ${ms} |`,
  );
}
console.log('| product FTS5 BM25 | 0.936 | 1.000 | 1.000 | 0.964 | 0.973 | 0 | 7 |');
console.log('| offline dense (zhipu/embedding-3, cosine) | 0.927 | 1.000 | 1.000 | 0.964 | 0.973 | 0 | 9 |');

for (const [name, pass] of Object.entries(passes)) {
  console.log(`\nper type (${name}):`);
  for (const [type, row] of Object.entries(pass.metrics.byType)) {
    console.log(`  ${type}: n=${row.n} R@1=${row['R@1'].toFixed(3)} R@3=${row['R@3'].toFixed(3)} misses=${row.misses}`);
  }
}

console.log('\nnegatives (top3):');
for (const item of queries.filter(q => q.target === null)) {
  const line = name => {
    const ids = passes[name].rankings[item.query].slice(0, 3);
    return ids.map(id => `${id}:${byId[id]}`).join(', ') || '(none)';
  };
  console.log(`  ${item.query}\n    keyword: ${line('keyword')}\n    vector:  ${line('vector')}\n    hybrid:  ${line('hybrid')}`);
}

fs.writeFileSync(
  path.join(HERE, 'meili_vector_ranks.json'),
  `${JSON.stringify({
    indexMs,
    dims,
    passes: Object.fromEntries(
      Object.entries(passes).map(([name, pass]) => [name, { metrics: pass.metrics, perQuery: pass.rows }]),
    ),
  }, null, 2)}\n`,
);
console.log('\n-> meili_vector_ranks.json');
