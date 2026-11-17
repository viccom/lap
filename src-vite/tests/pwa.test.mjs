import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile, access } from 'node:fs/promises';

const root = new URL('../', import.meta.url);
const read = (p) => readFile(new URL(p, root), 'utf8');

test('the PWA manifest declares the installable app shape', async () => {
  const raw = await read('public/manifest.webmanifest');
  const manifest = JSON.parse(raw);
  assert.equal(manifest.display, 'standalone');
  assert.equal(manifest.start_url, '/');
  assert.equal(manifest.scope, '/');
  const sizes = manifest.icons.map((i) => i.sizes);
  // Chrome's installability check needs at least a 192px (or larger maskable) icon.
  assert.ok(sizes.includes('192x192'), 'manifest needs a 192x192 icon');
  assert.ok(sizes.includes('512x512'), 'manifest needs a 512x512 icon');
  for (const icon of manifest.icons) {
    await access(new URL(icon.src.replace(/^\//, ''), new URL('public/', root)));
  }
});

test('index.html links the icon and manifest', async () => {
  const html = await read('index.html');
  assert.match(html, /rel="manifest"\s+href="\/manifest\.webmanifest"/);
  assert.match(html, /rel="icon"[^>]*href="\/favicon\.ico"/);
});

test('the service worker caches only the shell and never the API', async () => {
  const sw = await read('public/sw.js');
  // Never cache API/signed media: the whole point of the boundary.
  assert.match(sw, /url\.pathname\.startsWith\('\/api\/'\)/);
  assert.ok(!/cache\.put\([^)]*\/api\//.test(sw), 'must never cache an /api/ response');
  // Navigation must be network-first so a new lapd build wins when online.
  const nav = sw.slice(sw.indexOf("request.mode === 'navigate'"));
  assert.match(nav.slice(0, 200), /fetch\(request\)\.catch/, 'navigation is network-first with a cached fallback');
  // A cache version constant exists so the strategy can be invalidated.
  assert.match(sw, /const CACHE = 'lap-shell-v\d+'/);
});

test('the service worker is registered on the Web client only', async () => {
  const main = await read('src/main.js');
  // The registration must sit inside the http-mode branch (never the desktop shell).
  const httpBranch = main.slice(main.indexOf("if (transportMode === 'http')"));
  assert.match(httpBranch, /serviceWorker\.register\('\/sw\.js'\)/);
  // And nowhere else: exactly one registration call in the file.
  assert.equal((main.match(/serviceWorker\.register\(/g) || []).length, 1);
  // Registration must not depend solely on the `load` event: the awaits above it
  // (checkSession / serverInfo) can finish after `load` has already fired, in
  // which case a listener attached at this point never runs. Register eagerly
  // when the document is already complete.
  assert.match(
    httpBranch,
    /document\.readyState\s*===\s*'complete'/,
    'must register eagerly when the load event has already fired',
  );
});
