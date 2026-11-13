// Replay the Lap annotation-search evaluation against a local Meilisearch
// instance and compare it with the product FTS5 rankings on the same queries.
//
// Three configurations are measured, because charabia auto-detects the
// language via whatlang and Chinese can be misdetected (kanji overlap):
//   A auto:            no localizedAttributes, no locales param
//   B localizedAttrs:  index-side localizedAttributes = ['*'] -> zho
//   C pinned:          B + query-side locales: ['zho']
//
// Usage: node meili_replay.mjs [--url http://127.0.0.1:7700]
// Writes meili_ranks.json next to this script and prints metric tables.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = process.argv.includes('--url')
  ? process.argv[process.argv.indexOf('--url') + 1]
  : 'http://127.0.0.1:7700';
const INDEX = 'lap-eval';

const load = file => JSON.parse(fs.readFileSync(path.join(HERE, file), 'utf8'));
const corpus = load('corpus.json');
const queries = [...load('queries.json'), ...load('queries_hard.json')];
const productRanks = fs.existsSync(path.join(HERE, 'product_ranks.json'))
  ? load('product_ranks.json')
  : null;
const byId = Object.fromEntries(corpus.map(doc => [doc.id, doc.name]));

const api = async (method, path, body) => {
  const res = await fetch(`${BASE}${path}`, {
    method,
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(`${method} ${path} -> ${res.status} ${JSON.stringify(data).slice(0, 300)}`);
  }
  return data;
};

const waitTask = async taskUid => {
  for (;;) {
    const task = await api('GET', `/tasks/${taskUid}`);
    if (task.status === 'succeeded') return task;
    if (task.status === 'failed') {
      throw new Error(`task ${taskUid} failed: ${JSON.stringify(task.error).slice(0, 400)}`);
    }
    await new Promise(r => setTimeout(r, 50));
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
  // index did not exist yet
}
await waitTask((await api('POST', '/indexes', { uid: INDEX, primaryKey: 'id' })).taskUid);
await waitTask(
  (await api('PATCH', `/indexes/${INDEX}/settings`, {
    searchableAttributes: ['text', 'name'],
  })).taskUid,
);
const indexStarted = Date.now();
await waitTask((await api('POST', `/indexes/${INDEX}/documents`, corpus)).taskUid);
const indexMs = Date.now() - indexStarted;
console.log(`indexed ${corpus.length} docs in ${indexMs}ms (incl. task polling)`);

const replay = async extraBody => {
  const rankings = {};
  const perQuery = [];
  for (const item of queries) {
    const started = Date.now();
    const res = await api('POST', `/indexes/${INDEX}/search`, {
      q: item.query,
      limit: 50,
      attributesToRetrieve: ['id'],
      ...extraBody,
    });
    rankings[item.query] = res.hits.map(hit => Number(hit.id));
    perQuery.push({
      query: item.query,
      target: item.target,
      type: item.type,
      ids: rankings[item.query],
      ms: Date.now() - started,
      degraded: res.degraded ?? false,
    });
  }
  return { rankings, perQuery };
};

const evaluate = rankings => {
  const positives = queries.filter(item => item.target != null);
  const total = positives.length || 1;
  const ranks = positives.map(item => {
    const list = rankings[item.query] || [];
    const index = list.indexOf(Number(item.target));
    return index < 0 ? null : index + 1;
  });
  const byType = {};
  for (const type of [...new Set(positives.map(item => item.type))]) {
    const subset = positives.filter(item => item.type === type);
    const subsetRanks = subset.map(item => {
      const list = rankings[item.query] || [];
      const index = list.indexOf(Number(item.target));
      return index < 0 ? null : index + 1;
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

const metricsFor = rows => {
  const rankings = {};
  for (const row of rows) rankings[row.query] = row.ids;
  return evaluate(rankings);
};

const formatRow = (name, metrics, ms) =>
  `| ${name} | ${metrics['R@1'].toFixed(3)} | ${metrics['R@3'].toFixed(3)} | `
  + `${metrics['R@5'].toFixed(3)} | ${metrics['MRR@10'].toFixed(3)} | `
  + `${metrics['nDCG@10'].toFixed(3)} | ${metrics.misses} | ${ms} |`;

const summarize = (name, pass) => {
  const metrics = metricsFor(pass.perQuery);
  metrics.msPerQuery = Math.round(pass.perQuery.reduce((s, r) => s + r.ms, 0) / pass.perQuery.length);
  console.log(formatRow(name, metrics, metrics.msPerQuery));
  return metrics;
};

const positives = queries.filter(item => item.target != null);
console.log(`corpus=${corpus.length} queries=${queries.length} positives=${positives.length}`);
console.log('| method | R@1 | R@3 | R@5 | MRR@10 | nDCG@10 | misses | ms/query |');
console.log('|---|---|---|---|---|---|---|---|');

// Pass A: full auto (whatlang decides both sides).
const passA = await replay({});
const metricsA = summarize('meili A: full auto', passA);

// Pass B: pin the index side to Chinese (this reindexes the documents).
await waitTask(
  (await api('PATCH', `/indexes/${INDEX}/settings`, {
    localizedAttributes: [{ attributePatterns: ['*'], locales: ['zho'] }],
  })).taskUid,
);
const passB = await replay({});
const metricsB = summarize('meili B: localizedAttrs', passB);

// Pass C: pin the query side as well.
const passC = await replay({ locales: ['zho'] });
const metricsC = summarize('meili C: +locales=zho', passC);

let productMetrics = null;
let productMs = 0;
if (productRanks) {
  productMetrics = metricsFor(productRanks);
  productMs = Math.round(
    productRanks.reduce((s, r) => s + (r.ms || 0), 0) / productRanks.length,
  );
  console.log(formatRow('product FTS5 BM25', productMetrics, productMs));
}

for (const [label, metrics, pass] of [
  ['A full-auto', metricsA, passA],
  ['B localizedAttrs', metricsB, passB],
  ['C +locales=zho', metricsC, passC],
]) {
  console.log(`\nper type (${label}):`);
  for (const [type, row] of Object.entries(metrics.byType)) {
    console.log(`  ${type}: n=${row.n} R@1=${row['R@1'].toFixed(3)} R@3=${row['R@3'].toFixed(3)} misses=${row.misses}`);
  }
  console.log(`negatives (${label}):`);
  for (const item of queries.filter(q => q.target == null)) {
    const ids = pass.rankings[item.query].slice(0, 3);
    console.log(`  ${item.query} -> ${ids.map(id => `${id}:${byId[id]}`).join(', ') || '(none)'}`);
  }
}

// Behaviour probes under the fully pinned configuration.
const probes = [];
console.log('\nprobes (config C):');
for (const probe of [
  { label: 'prefix 葡', q: '葡' },
  { label: 'cjk-typo-short 葡荀', q: '葡荀' },
  { label: 'order-swap 草地 男孩', q: '草地 男孩' },
  { label: 'filename C (166).jpg', q: 'C (166).jpg' },
]) {
  const res = await api('POST', `/indexes/${INDEX}/search`, {
    q: probe.q,
    limit: 10,
    attributesToRetrieve: ['id'],
    locales: ['zho'],
  });
  probes.push({ ...probe, ids: res.hits.map(hit => Number(hit.id)) });
  console.log(`  ${probe.label}: ${JSON.stringify(res.hits.map(hit => Number(hit.id)))}`);
}

fs.writeFileSync(
  path.join(HERE, 'meili_ranks.json'),
  `${JSON.stringify(
    {
      indexMs,
      passes: {
        auto: { metrics: metricsA, perQuery: passA.perQuery },
        localized: { metrics: metricsB, perQuery: passB.perQuery },
        pinned: { metrics: metricsC, perQuery: passC.perQuery },
      },
      productMetrics,
      probes,
    },
    null,
    2,
  )}\n`,
);
console.log('\n-> meili_ranks.json');
