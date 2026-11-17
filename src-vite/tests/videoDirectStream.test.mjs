import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

const source = fs.readFileSync(new URL('../src/components/Video.vue', import.meta.url), 'utf8');
const start = source.indexOf('  const loadDirect = async');
const code = ts.transpile(source.slice(start, source.indexOf('  // Matroska files', start)), { target: ts.ScriptTarget.ES2022 });

function setup(isLinux, filePath, resolveUrl = async () => 'http://127.0.0.1/video', { mode = 'tauri', webSrc = async () => 'http://127.0.0.1/api/video/7?exp=1&sig=x' } = {}) {
  const calls = [], listeners = {}, timers = [];
  const player = {
    reset() { calls.push('reset'); },
    src(value) { calls.push(value); },
    one(name, fn) { listeners[name] = fn; },
    off(name) { delete listeners[name]; },
    load() { calls.push('load'); },
    videoWidth: () => 1920,
    videoHeight: () => 1080,
  };
  const create = new Function('isLinux', 'filePath', 'getVideoStreamUrl', 'getAssetSrc', 'transportMode', 'webVideoSrc', 'player', 'resetCompatibilityProcessing', 'loadPrepared', 'handleSuccessfulLoad', 'loadAttemptCleanups', 'hasError', 'errorMessage', 'getFallbackErrorMessage', 'setTimeout', 'clearTimeout', 'DIRECT_PLAYBACK_TIMEOUT_MS', `
    let currentLoadId = 1, currentLoadingId = 1, nextUpIndex = 0;
    ${code}
    return { load: loadDirect, cancel() { currentLoadingId++; } };
  `);
  const controls = create(isLinux, filePath, async path => { calls.push(['stream', path]); return resolveUrl(); }, path => `asset:${path}`, mode, webSrc, player, () => {}, mode2 => calls.push(['prepare', mode2]), () => calls.push('success'), [], { value: false }, { value: '' }, () => 'fallback-error', fn => { timers.push(fn); return timers.length; }, () => {}, 8000);
  return { ...controls, calls, listeners, timers };
}

test('Linux MP4/M4V uses the original HTTP stream and retains playback fallback', async () => {
  for (const extension of ['mp4', 'M4V']) {
    const t = setup(true, `/nas/clip.${extension}`);
    await t.load('video/mp4');
    assert.deepEqual(t.calls[0], ['stream', `/nas/clip.${extension}`]);
    assert.ok(t.calls.some(c => c?.src === 'http://127.0.0.1/video'));
    assert.ok(!t.calls.some(c => c?.[0] === 'prepare'));
    t.listeners.error();
    t.timers[0]();
    assert.equal(t.calls.filter(c => c?.[0] === 'prepare').length, 1);
  }
});

test('other platforms and Linux WebM retain asset transport', async () => {
  for (const [linux, path] of [[false, '/clip.mp4'], [true, '/clip.webm']]) {
    const t = setup(linux, path);
    await t.load('video/mp4');
    assert.ok(t.calls.some(c => c?.src === `asset:${path}`));
    assert.ok(!t.calls.some(c => c?.[0] === 'stream'));
  }
});

test('URL resolution failure falls back to compatibility processing', async () => {
  const t = setup(true, '/clip.mp4', async () => { throw new Error('unavailable'); });
  await t.load('video/mp4');
  assert.deepEqual(t.calls.at(-1), ['prepare', 'compatible']);
  assert.ok(!t.calls.includes('load'));
});

test('direct timeout retains compatibility fallback while a loaded frame settles the attempt', async () => {
  const stalled = setup(true, '/clip.mp4');
  await stalled.load('video/mp4');
  stalled.timers[0]();
  assert.deepEqual(stalled.calls.at(-1), ['prepare', 'compatible']);

  const loaded = setup(true, '/clip.mp4');
  await loaded.load('video/mp4');
  loaded.listeners.loadeddata();
  loaded.timers[0]();
  assert.equal(loaded.calls.at(-1), 'success');
  assert.ok(!loaded.calls.some(c => c?.[0] === 'prepare'));
});

test('switching video during URL resolution prevents stale player changes or fallback', async () => {
  for (const fail of [false, true]) {
    let finish;
    const t = setup(true, '/clip.mp4', () => new Promise((resolve, reject) => { finish = () => fail ? reject(new Error('unavailable')) : resolve('http://old'); }));
    const pending = t.load('video/mp4');
    t.cancel();
    finish();
    await pending;
    assert.deepEqual(t.calls, [['stream', '/clip.mp4']]);
  }
});

test('Web client resolves playback through /api/video/{file_id} instead of a path', async () => {
  const t = setup(false, 'D:/photos/clip.mp4', async () => 'http://127.0.0.1/x', { mode: 'http' });
  await t.load('video/mp4');
  assert.ok(
    t.calls.some(c => c?.src === 'http://127.0.0.1/api/video/7?exp=1&sig=x'),
    'the Web source must be the signed /api/video URL',
  );
  assert.ok(!t.calls.some(c => c?.src === 'asset:D:/photos/clip.mp4'), 'must not use the path-based asset URL');
});

test('a failed Web direct attempt surfaces an error instead of recursing into prepare', async () => {
  const t = setup(false, 'D:/photos/clip.mp4', async () => 'http://127.0.0.1/x', { mode: 'http' });
  await t.load('video/mp4');
  // The direct attempt errors: on the Web there is no transcode fallback, so it
  // must NOT call loadPrepared (which delegates back into loadDirect → loop).
  t.listeners.error();
  assert.ok(!t.calls.some(c => c?.[0] === 'prepare'), 'Web failure must not trigger compatibility processing');
});

