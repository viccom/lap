// Transport seam tests (phase 1 of the frontend/back-end split).
//
// These exercise the pure pieces and the two transport factories with injected
// dependencies, so no Tauri runtime and no network are needed under Node.
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

import { resolveTransportMode } from '../src/common/transport/mode.js';
import { resolveCapabilities } from '../src/common/transport/capabilities.js';
import { createTauriTransport } from '../src/common/transport/tauri.js';
import { createHttpTransport } from '../src/common/transport/http.js';

test('resolveTransportMode opts into http only on the explicit flag', () => {
  assert.equal(resolveTransportMode('http'), 'http');
  assert.equal(resolveTransportMode('tauri'), 'tauri');
  // Unset or misspelled must never silently leave the desktop transport.
  assert.equal(resolveTransportMode(undefined), 'tauri');
  assert.equal(resolveTransportMode(''), 'tauri');
  assert.equal(resolveTransportMode('HTTP'), 'tauri');
});

test('resolveCapabilities: http mode is never native', () => {
  assert.deepEqual(resolveCapabilities('http'), { native: false });
});

test('resolveCapabilities: tauri mode is native with the internals present', () => {
  assert.deepEqual(resolveCapabilities('tauri', { window: { __TAURI_INTERNALS__: {} } }), {
    native: true,
  });
});

test('resolveCapabilities: a tauri bundle opened in a plain browser degrades', () => {
  assert.deepEqual(resolveCapabilities('tauri', { window: {} }), { native: false });
  // No window at all (Node/SSR) keeps the compile-time answer.
  assert.deepEqual(resolveCapabilities('tauri', {}), { native: true });
});

test('tauri transport passes invoke through unchanged', async () => {
  const calls = [];
  const t = createTauriTransport({
    invoker: async (name, args) => {
      calls.push({ name, args });
      return 42;
    },
  });
  const out = await t.invoke('some_cmd', { a: 1 });
  assert.equal(out, 42);
  assert.deepEqual(calls, [{ name: 'some_cmd', args: { a: 1 } }]);
  assert.deepEqual(t.capabilities(), { native: true });
});

test('tauri transport keeps the custom-scheme media URLs', () => {
  const t = createTauriTransport({ convertFileSrcImpl: (p) => `asset:${p}` });
  assert.equal(t.assetUrl('thumb', { libraryId: 3, fileId: 7, win: true }), 'http://thumb.localhost/3/7');
  assert.equal(t.assetUrl('preview', { libraryId: 3, fileId: 7, win: false }), 'preview://localhost/3/7');
  assert.equal(t.assetUrl('asset', { filePath: 'C:/x.jpg' }), 'asset:C:/x.jpg');
  assert.equal(t.assetUrl('asset', { filePath: '' }), '');
});

test('http transport posts commands to /api/cmd/{name} and parses JSON', async () => {
  const calls = [];
  const t = createHttpTransport({
    baseUrl: 'http://host:9999',
    fetchImpl: async (url, opts) => {
      calls.push({ url, opts });
      return {
        ok: true,
        status: 200,
        text: async () => JSON.stringify({ ok: 1 }),
      };
    },
  });
  const out = await t.invoke('list_files', { folderId: 5 });
  assert.deepEqual(out, { ok: 1 });
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, 'http://host:9999/api/cmd/list_files');
  assert.equal(calls[0].opts.method, 'POST');
  assert.equal(calls[0].opts.headers['Content-Type'], 'application/json');
  assert.equal(calls[0].opts.body, JSON.stringify({ folderId: 5 }));
  assert.deepEqual(t.capabilities(), { native: false });
});

test('http transport: a no-arg command sends an empty object', async () => {
  let body;
  const t = createHttpTransport({
    fetchImpl: async (_url, opts) => {
      body = opts.body;
      return { ok: true, status: 200, text: async () => '' };
    },
  });
  const out = await t.invoke('get_app_config');
  assert.equal(out, null);
  assert.equal(body, '{}');
});

test('http transport: a non-2xx response throws with the command name', async () => {
  const t = createHttpTransport({
    fetchImpl: async () => ({ ok: false, status: 403, text: async () => '' }),
  });
  await assert.rejects(() => t.invoke('forbidden_cmd'), /forbidden_cmd.*403/);
});

test('http transport: emit reaches a same-tab listen handler', async () => {
  const seen = [];
  const t = createHttpTransport({ EventSourceImpl: null });
  const off = await t.listen('progress', (ev) => seen.push(ev.payload));
  t.emit('progress', { done: 3 });
  assert.deepEqual(seen, [{ done: 3 }]);
  off();
  t.emit('progress', { done: 4 });
  assert.deepEqual(seen, [{ done: 3 }]);
});

test('http transport: SSE payloads fan out to the named listener', async () => {
  // A minimal EventSource stand-in we can drive from the test.
  let onmessage = null;
  class FakeEventSource {
    set onmessage(fn) {
      onmessage = fn;
    }
    get onmessage() {
      return onmessage;
    }
  }
  const seen = [];
  const t = createHttpTransport({ EventSourceImpl: FakeEventSource });
  await t.listen('index-progress', (ev) => seen.push(ev.payload));
  onmessage({ data: JSON.stringify({ event: 'index-progress', id: '7', payload: { pct: 50 } }) });
  // A different event name must not leak into this listener.
  onmessage({ data: JSON.stringify({ event: 'other', payload: { pct: 9 } }) });
  assert.deepEqual(seen, [{ pct: 50 }]);
  // Malformed frames are ignored rather than thrown.
  onmessage({ data: 'not json' });
  assert.deepEqual(seen, [{ pct: 50 }]);
});

test('http transport: media URLs follow the /api/{kind}/{library}/{file} shape', () => {
  const t = createHttpTransport({ baseUrl: '' });
  assert.equal(t.assetUrl('thumb', { libraryId: 2, fileId: 11 }), '/api/thumb/2/11');
  assert.equal(t.assetUrl('preview', { libraryId: 2, fileId: 11 }), '/api/preview/2/11');
  // `asset` (arbitrary path) has no Web endpoint until phase 3.
  assert.equal(t.assetUrl('asset', { filePath: 'C:/x.jpg' }), '');
});

test('http transport: a 401 signals the unauthorized handler', async () => {
  const t = createHttpTransport({
    fetchImpl: async () => ({ ok: false, status: 401, text: async () => '' }),
  });
  let fired = 0;
  const off = t.onUnauthorized(() => { fired += 1; });
  await assert.rejects(() => t.invoke('get_app_config'), /401/);
  assert.equal(fired, 1);
  off();
  await assert.rejects(() => t.invoke('get_app_config'), /401/);
  assert.equal(fired, 1);
});

test('http transport: auth endpoints post JSON and return the credential', async () => {
  const calls = [];
  const t = createHttpTransport({
    fetchImpl: async (url, opts) => {
      calls.push({ url, opts });
      const body = JSON.parse(opts.body);
      return {
        ok: true,
        status: 200,
        text: async () => JSON.stringify({ token: 'tok', scope: body.code ? 'admin' : 'standard' }),
      };
    },
  });
  const cred = await t.login('pw', 'browser');
  assert.equal(cred.scope, 'standard');
  assert.equal(calls[0].url, '/api/auth/login');
  assert.deepEqual(JSON.parse(calls[0].opts.body), { password: 'pw', deviceName: 'browser' });
  const paired = await t.pair('123456');
  assert.equal(paired.scope, 'admin');
  assert.equal(calls[1].url, '/api/auth/pair');
});

test('http transport: signedAssetUrl mints via /api/media/sign and caches', async () => {
  const calls = [];
  const t = createHttpTransport({
    fetchImpl: async (url, opts) => {
      calls.push({ url, body: JSON.parse(opts.body) });
      const urls = JSON.parse(opts.body).urls.map((u) => `${u}&exp=9999999999&sig=abc&dev=1`);
      return { ok: true, status: 200, json: async () => ({ urls }) };
    },
  });
  const a = await t.signedAssetUrl('/api/thumb/default/1?size=256');
  assert.match(a, /sig=abc&dev=1/);
  const b = await t.signedAssetUrl('/api/thumb/default/1?size=256');
  assert.equal(b, a);
  // Same URL must be served from cache, not re-minted.
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].body.urls, ['/api/thumb/default/1?size=256']);
});

test('migrated files import the transport seam, not @tauri-apps core/event', () => {
  // Guard: a file that still imports core/event directly would keep calling
  // Tauri under VITE_LAP_TRANSPORT=http, defeating the phase-1 contract.
  const files = [
    'src/common/api.js',
    'src/common/utils.ts',
    'src/common/vlApi.js',
    'src/main.js',
  ];
  for (const rel of files) {
    const src = readFileSync(new URL(`../${rel}`, import.meta.url), 'utf8').replace(/\r\n/g, '\n');
    assert.ok(
      /from '(?:@\/common\/transport|\.\/transport\/index\.js)'/.test(src),
      `${rel} must import from the transport seam`,
    );
    assert.ok(
      !/from '@tauri-apps\/api\/(core|event)'/.test(src),
      `${rel} must not import @tauri-apps core/event directly`,
    );
  }
});
