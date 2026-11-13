// Replay the Lap eval queries through the real semantic-search command
// (`search_similar_images`) over CDP, for both text models.
//
// Requires a running debug build with the WebView2 debugging port open
// (tauri.windows.conf.json -> additionalBrowserArgs, or the env var in dev).
//
// Datasets:
//   queries.json + queries_hard.json  Chinese, targets = annotated photo ids
//   queries_en.json                   English rewrites of the same captions
//
// Usage: node semantic_replay.mjs [--port 9333]
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const PORT = process.argv.includes('--port')
  ? process.argv[process.argv.indexOf('--port') + 1]
  : '9333';

const load = (file, fallback = null) => {
  const full = path.join(HERE, file);
  return fs.existsSync(full) ? JSON.parse(fs.readFileSync(full, 'utf8')) : fallback;
};

const targets = await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json();
const page = targets.find(target => target.type === 'page');
if (!page) throw new Error('no CDP page target');
const ws = new WebSocket(page.webSocketDebuggerUrl);
let nextId = 1;
const pending = new Map();

ws.addEventListener('message', event => {
  const message = JSON.parse(event.data);
  if (message.id && pending.has(message.id)) {
    pending.get(message.id)(message);
    pending.delete(message.id);
  }
});
await new Promise((resolve, reject) => {
  ws.addEventListener('open', resolve, { once: true });
  ws.addEventListener('error', reject, { once: true });
});

function send(method, params = {}) {
  const id = nextId++;
  return new Promise(resolve => {
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression) {
  const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (response.error) throw new Error(JSON.stringify(response.error));
  const result = response.result;
  if (result.exceptionDetails) {
    throw new Error(
      result.exceptionDetails.exception?.description
      ?? JSON.stringify(result).slice(0, 600),
    );
  }
  return result.result.value;
}

const invoke = (command, args) =>
  evaluate(
    `(window.__TAURI__?.core?.invoke ?? window.__TAURI_INTERNALS__.invoke)(${JSON.stringify(command)},${JSON.stringify(args ?? null)})`,
  );

const chinese = [...(load('queries.json') ?? []), ...(load('queries_hard.json') ?? [])];
const english = load('queries_en.json') ?? [];
const datasets = { 'zh (110)': chinese };
if (english.length) datasets[`en (${english.length})`] = english;

const evaluateRanks = rows => {
  const positives = rows.filter(row => row.target != null);
  const total = positives.length || 1;
  const ranks = positives.map(row => {
    const at = row.ids.indexOf(Number(row.target));
    return at < 0 ? null : at + 1;
  });
  const byType = {};
  for (const type of [...new Set(positives.map(row => row.type))]) {
    const subset = positives.filter(row => row.type === type);
    const subsetRanks = subset.map(row => {
      const at = row.ids.indexOf(Number(row.target));
      return at < 0 ? null : at + 1;
    });
    byType[type] = {
      n: subset.length,
      'R@1': subsetRanks.filter(r => r === 1).length / subset.length,
      'R@3': subsetRanks.filter(r => r && r <= 3).length / subset.length,
      misses: subsetRanks.filter(r => r == null).length,
    };
  }
  const ok = ranks.filter(r => r != null);
  return {
    'R@1': ranks.filter(r => r === 1).length / total,
    'R@3': ranks.filter(r => r && r <= 3).length / total,
    'R@5': ranks.filter(r => r && r <= 5).length / total,
    'MRR@10': ok.reduce((sum, r) => sum + (r <= 10 ? 1 / r : 0), 0) / total,
    'nDCG@10': ok.reduce((sum, r) => sum + (r <= 10 ? 1 / Math.log2(r + 1) : 0), 0) / total,
    misses: ranks.filter(r => r == null).length,
    returned: Math.round(rows.reduce((sum, row) => sum + row.ids.length, 0) / rows.length),
    byType,
  };
};

const status = await invoke('get_image_search_model_status');
console.log('model status:', JSON.stringify(status));

const results = {};
const modelOrder = status.activeModel === 1 ? [1, 0] : [0, 1];
for (const model of modelOrder) {
  const modelLabel = model === 0 ? 'default CLIP (ViT-B/32, English)' : 'multilingual (519MB)';
  if (model === 1 && !status.multilingualAvailable) {
    console.log('multilingual model not available, skipping');
    continue;
  }
  try {
    await invoke('set_image_search_model', { model });
  } catch (error) {
    console.log(`switch to ${modelLabel} failed: ${error.message}`);
    continue;
  }
  for (const [datasetLabel, rows] of Object.entries(datasets)) {
    const out = [];
    for (const item of rows) {
      const started = Date.now();
      let hits = null;
      let error = null;
      let truncated = false;
      try {
        hits = await invoke('search_similar_images', {
          params: { searchText: item.query, fileId: null, threshold: 0.0, fileType: 0 },
          libraryId: null,
        });
      } catch (failure) {
        // The default CLIP text tower has a fixed 77-position input and the
        // command does not truncate; retry with a shorter prefix so the rest
        // of the eval still produces a ranking.
        error = failure.message;
        truncated = true;
        try {
          hits = await invoke('search_similar_images', {
            params: {
              searchText: [...item.query].slice(0, 45).join(''),
              fileId: null,
              threshold: 0.0,
              fileType: 0,
            },
            libraryId: null,
          });
        } catch (second) {
          error = `${error} | retry: ${second.message}`;
          hits = null;
        }
      }
      out.push({
        query: item.query,
        target: item.target,
        type: item.type,
        ids: (hits ?? []).map(hit => Number(hit.id)),
        top3: (hits ?? []).slice(0, 3).map(hit => hit.name),
        ms: Date.now() - started,
        error,
        truncated,
      });
    }
    const metrics = evaluateRanks(out);
    results[`${modelLabel} | ${datasetLabel}`] = { metrics, perQuery: out };
    const ms = Math.round(out.reduce((sum, row) => sum + row.ms, 0) / out.length);
    const failed = out.filter(row => row.ids.length === 0 && row.error);
    const truncatedCount = out.filter(row => row.truncated).length;
    console.log(
      `\n${modelLabel} | ${datasetLabel} | R@1 ${metrics['R@1'].toFixed(3)} | `
      + `R@3 ${metrics['R@3'].toFixed(3)} | misses ${metrics.misses} | `
      + `avg returned ${metrics.returned} | ${ms}ms/query | `
      + `errors ${failed.length} | truncated ${truncatedCount}`,
    );
    for (const [type, row] of Object.entries(metrics.byType)) {
      console.log(`  ${type}: n=${row.n} R@1=${row['R@1'].toFixed(3)} R@3=${row['R@3'].toFixed(3)} misses=${row.misses}`);
    }
    for (const row of out.filter(item => item.target != null && item.ids[0] !== Number(item.target)).slice(0, 3)) {
      console.log(`  miss sample: ${row.query} -> ${row.top3.join(' | ')}`);
    }
    if (failed.length) {
      console.log(`  error sample: ${failed[0].query.slice(0, 40)} -> ${failed[0].error.slice(0, 160)}`);
    }
  }
}

// The user's own recent queries, no ground truth — just show what comes back.
const userQueries = [
  'Dong Kaiyuan plays with snow',
  'winter',
  'autumn',
  'summer',
  'picnic',
  'seaside',
];
await invoke('set_image_search_model', { model: status.activeModel ?? 0 }).catch(() => {});
console.log('\nuser history queries (active model):');
for (const query of userQueries) {
  const hits = await invoke('search_similar_images', {
    params: { searchText: query, fileId: null, threshold: 0.26, fileType: 0 },
    libraryId: null,
  });
  console.log(
    `  ${query} -> ${(hits ?? []).length} hits: `
    + (hits ?? []).slice(0, 5).map(hit => hit.name).join(' | '),
  );
}

fs.writeFileSync(
  path.join(HERE, 'semantic_ranks.json'),
  `${JSON.stringify({ status, results }, null, 2)}\n`,
);
console.log('\n-> semantic_ranks.json');
ws.close();
