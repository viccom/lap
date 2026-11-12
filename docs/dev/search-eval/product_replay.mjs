// Replay the evaluation queries through the real Tauri command (`search_fts`).
//
// Requires a running dev build with the WebView2 debugging port open:
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222 cargo tauri dev
// Writes product_ranks.json next to this script.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const BASE = 'http://127.0.0.1:9222';

const list = await (await fetch(`${BASE}/json/list`)).json();
const page = list.find(target => target.type === 'page' && !/devtools/.test(target.url));
if (!page) throw new Error('no page target on http://127.0.0.1:9222');

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = reject;
});

let nextId = 0;
const evaluate = expression => new Promise((resolve, reject) => {
  const id = ++nextId;
  const onMessage = event => {
    const message = JSON.parse(event.data);
    if (message.id !== id) return;
    ws.removeEventListener('message', onMessage);
    message.error ? reject(new Error(message.error.message)) : resolve(message.result);
  };
  ws.addEventListener('message', onMessage);
  ws.send(JSON.stringify({
    id,
    method: 'Runtime.evaluate',
    params: { expression, awaitPromise: true, returnByValue: true },
  }));
});

const invoke = async expression => {
  const result = await evaluate(expression);
  if (result.exceptionDetails) {
    throw new Error(result.exceptionDetails.exception?.description || result.exceptionDetails.text);
  }
  return result.result?.value;
};

const load = file => JSON.parse(fs.readFileSync(path.join(HERE, file), 'utf8'));
const queries = [...load('queries.json'), ...load('queries_hard.json')];

const out = [];
for (const item of queries) {
  const started = Date.now();
  const hits = await invoke(
    `window.__TAURI_INTERNALS__.invoke('search_fts',{query:${JSON.stringify(item.query)},limit:1000})`,
  );
  out.push({
    query: item.query,
    target: item.target,
    type: item.type,
    ids: (hits || []).map(hit => Number(hit.fileId)),
    ms: Date.now() - started,
  });
}

fs.writeFileSync(path.join(HERE, 'product_ranks.json'), `${JSON.stringify(out, null, 2)}\n`);
console.log(`queries: ${out.length} -> product_ranks.json`);
process.exit(0);
