import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

const source = fs.readFileSync(new URL('../src/components/Thumbnail.vue', import.meta.url), 'utf8');
const body = ts.transpile(source.slice(source.indexOf('let animatedImagePreviewRequest ='),
  source.indexOf('function handleDoubleClick(')), { target: ts.ScriptTarget.ES2022 });
function fixture() {
  let timer;
  const checks = [];
  const props = { file: { file_path: '/first.gif' } };
  const show = { value: false };
  const api = new Function('props', 'canPreviewAnimatedImage', 'isCloudOnlyFile', 'setTimeout', 'clearTimeout',
    'showAnimatedImagePreview', 'isAnimatedImagePreviewReady',
    'let animatedImagePreviewTimer = null; ' + body + '; return {startAnimatedImagePreview, stopAnimatedImagePreview};')(
    props, { value: true }, path => new Promise(resolve => checks.push({ path, resolve })),
    callback => { timer = callback; return 1; }, () => { timer = null; }, show, { value: false });
  return { ...api, props, show, checks, fireTimer: () => { const callback = timer; timer = null; return callback(); } };
}

test('animated hover preview still starts for local files and skips cloud-only files', async () => {
  for (const cloud of [false, true]) {
    const f = fixture(); f.startAnimatedImagePreview();
    const pending = f.fireTimer(); f.checks[0].resolve(cloud); await pending;
    assert.equal(f.show.value, !cloud);
  }
});

test('leaving the thumbnail invalidates a pending attribute check', async () => {
  const f = fixture(); f.startAnimatedImagePreview();
  const pending = f.fireTimer();
  f.stopAnimatedImagePreview();
  f.checks[0].resolve(false); await pending;
  assert.equal(f.show.value, false);
});

test('switching files cannot reuse the previous file attribute check', async () => {
  const f = fixture(); f.startAnimatedImagePreview();
  const pending = f.fireTimer();
  f.props.file = { file_path: '/second.gif' };
  f.checks[0].resolve(false); await pending;
  assert.equal(f.show.value, false);
});

test('a new hover request supersedes the previous request even for the same file', async () => {
  const f = fixture(); f.startAnimatedImagePreview();
  const old = f.fireTimer();
  f.stopAnimatedImagePreview(); f.startAnimatedImagePreview();
  const current = f.fireTimer();
  f.checks[0].resolve(false); await old;
  assert.equal(f.show.value, false);
  f.checks[1].resolve(false); await current;
  assert.equal(f.show.value, true);
});

test('unmount and file-change cleanup invalidate pending animated previews', async () => {
  for (const start of ['onBeforeUnmount(() => {', 'watch(() => [props.file.file_path, props.file.live_photo_video_path], () => {']) {
    const begin = source.indexOf(start);
    assert.notEqual(begin, -1);
    const cleanup = source.slice(begin + start.length, source.indexOf('});', begin));
    const f = fixture(); f.startAnimatedImagePreview();
    const pending = f.fireTimer();
    const stopMediaPreview = () => f.stopAnimatedImagePreview();
    // The unmount hook also releases the grid-style transition timer, so the
    // simulated component scope carries that local too.
    new Function('stopMediaPreview', 'resizeObserver', 'transitionTimeout', cleanup)(stopMediaPreview, null, null);
    f.checks[0].resolve(false); await pending;
    assert.equal(f.show.value, false);
  }
});
