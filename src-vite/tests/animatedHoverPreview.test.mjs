import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';

// Contract note: upstream a11887e2 ("prevent automatic downloads of
// online-only media") removed the per-hover async isCloudOnlyFile check from
// the animated-preview timer. Cloud-only gating now happens synchronously at
// entry through canPreviewAnimatedImage (catalog flag), and the timer guards
// only request identity and file switches. These tests pin THAT contract.
const source = fs.readFileSync(new URL('../src/components/Thumbnail.vue', import.meta.url), 'utf8');
const body = ts.transpile(source.slice(source.indexOf('let animatedImagePreviewRequest ='),
  source.indexOf('function handleDoubleClick(')), { target: ts.ScriptTarget.ES2022 });

function fixture({ canPreview = true } = {}) {
  let timer = null;
  const props = { file: { file_path: '/first.gif' } };
  const show = { value: false };
  const ready = { value: false };
  const canPreviewAnimatedImage = { value: canPreview };
  const api = new Function('props', 'canPreviewAnimatedImage', 'setTimeout', 'clearTimeout',
    'showAnimatedImagePreview', 'isAnimatedImagePreviewReady',
    'let animatedImagePreviewTimer = null; ' + body + '; return {startAnimatedImagePreview, stopAnimatedImagePreview};')(
    props, canPreviewAnimatedImage,
    callback => { timer = callback; return 1; }, () => { timer = null; }, show, ready);
  return {
    ...api, props, show, canPreviewAnimatedImage,
    peek: () => timer,
    fireTimer: () => { const callback = timer; timer = null; return callback ? callback() : undefined; },
  };
}

test('animated hover preview still starts for local files and skips cloud-only files', () => {
  const local = fixture({ canPreview: true });
  local.startAnimatedImagePreview();
  assert.ok(local.peek(), 'a hover timer must be scheduled for local files');
  local.fireTimer();
  assert.equal(local.show.value, true);

  // Cloud-only files are gated at entry via canPreviewAnimatedImage; no timer
  // is ever scheduled, so hovering can never trigger a download.
  const cloud = fixture({ canPreview: false });
  cloud.startAnimatedImagePreview();
  assert.equal(cloud.peek(), null);
  cloud.fireTimer();
  assert.equal(cloud.show.value, false);
});

test('leaving the thumbnail cancels a pending preview timer', () => {
  const f = fixture();
  f.startAnimatedImagePreview();
  assert.ok(f.peek());
  f.stopAnimatedImagePreview();
  assert.equal(f.peek(), null, 'the pending timer must be cleared');
  f.fireTimer();
  assert.equal(f.show.value, false);
});

test('switching files invalidates the pending preview', () => {
  const f = fixture();
  f.startAnimatedImagePreview();
  f.props.file = { file_path: '/second.gif' };
  f.fireTimer();
  assert.equal(f.show.value, false, 'a timer captured for another file must not show');
});

test('a new hover request supersedes the previous request even for the same file', () => {
  const f = fixture();
  f.startAnimatedImagePreview();
  const stale = f.peek();
  f.stopAnimatedImagePreview();
  f.startAnimatedImagePreview();
  stale();
  assert.equal(f.show.value, false, 'the superseded request must not show');
  f.fireTimer();
  assert.equal(f.show.value, true, 'the current request must show');
});

test('unmount and file-change cleanup stop pending or shown animated previews', () => {
  for (const start of ['onBeforeUnmount(() => {', 'watch(() => [props.file.file_path, props.file.live_photo_video_path], () => {']) {
    const begin = source.indexOf(start);
    assert.notEqual(begin, -1);
    const cleanup = source.slice(begin + start.length, source.indexOf('});', begin));
    const f = fixture();
    f.startAnimatedImagePreview();
    f.fireTimer();
    assert.equal(f.show.value, true);
    const stopMediaPreview = () => f.stopAnimatedImagePreview();
    // The unmount hook also releases the grid-style transition timer, so the
    // simulated component scope carries that local too.
    new Function('stopMediaPreview', 'resizeObserver', 'transitionTimeout', cleanup)(stopMediaPreview, null, null);
    assert.equal(f.show.value, false);
  }
});
