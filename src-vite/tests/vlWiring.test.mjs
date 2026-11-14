import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const read = (relative) => readFile(new URL(relative, import.meta.url), 'utf8');

function sliceFunction(source, marker, endMarker = '\n}') {
  const start = source.indexOf(marker);
  assert.ok(start >= 0, `source must define ${marker}`);
  return source.slice(start, source.indexOf(endMarker, start));
}

test('a mixed image+video selection cannot start a batch understanding run', async () => {
  const source = await read('../src/common/fileMenu.ts');
  const entryAt = source.indexOf('vl_analyze_selected');
  assert.ok(entryAt >= 0, 'fileMenu.ts must define the batch VL entry');
  // Slice the whole menu item object around the entry.
  const item = source.slice(source.lastIndexOf('{', entryAt), source.indexOf("action: createAction('vl-analyze-selected')", entryAt));
  // Videos only carry a first-frame thumbnail: a mixed or video selection
  // must be blocked exactly like the single-file entry (`!isImage`) and the
  // compare-selected entry (`kind !== 'image'`), not only a pure-video one.
  assert.ok(
    /disabled:\s*selectionCount === 0 \|\| kind !== 'image'/.test(item),
    'the batch VL entry must disable everything that is not a pure image selection',
  );
});

test('VL run listeners claim their slot synchronously and tear down if the view died meanwhile', async () => {
  const source = await read('../src/components/Content.vue');
  const body = sliceFunction(source, 'function ensureVlRunListeners');
  // The claim must be synchronous: two rapid starts inside the async
  // registration window would otherwise register two listener sets.
  const claimAt = body.indexOf('vlRunListenersRequested = true');
  const firstAwait = body.indexOf('await ');
  assert.ok(claimAt >= 0, 'ensureVlRunListeners must set vlRunListenersRequested = true');
  assert.ok(firstAwait < 0 || claimAt < firstAwait,
    'the listener claim must run before the first await');
  // Unmount before the registration lands must still release the listeners.
  assert.ok(body.includes('vlRunListenersDisposed'),
    'the registration must check the disposal flag after its awaits');
  // The disposal flag must be raised by an unmount hook (Content has several;
  // anchor on the flag, not on the first hook in the file).
  const flagAt = source.indexOf('vlRunListenersDisposed = true');
  const hookAt = flagAt >= 0 ? source.lastIndexOf('onBeforeUnmount', flagAt) : -1;
  assert.ok(flagAt >= 0 && hookAt >= 0 && flagAt - hookAt < 200,
    'onBeforeUnmount must raise the disposal flag');
});

test('a finished run refreshes the badges of this view even when it was started elsewhere', async () => {
  const source = await read('../src/components/Content.vue');
  const listenerAt = source.indexOf("listen('vl-analysis-finished'");
  assert.ok(listenerAt >= 0, 'Content.vue must listen for vl-analysis-finished');
  const body = source.slice(listenerAt, source.indexOf('});', listenerAt));
  // The toast stays gated on the runs this view started ...
  assert.ok(
    /if \(vlActiveRunIds\.has\(String\(status\.runId\)\)\) \{/.test(body),
    'the toast must stay inside the vlActiveRunIds gate',
  );
  // ... but the old early-return gate (`!status || !vlActiveRunIds.has(...)
  // return;`) kept the badge patch and the membership refresh out of reach
  // for runs started in the lab window while the sidebar counts refreshed.
  assert.ok(
    !body.includes('!vlActiveRunIds.has(String(status.runId))) return'),
    'the badge patch must not hide behind the vlActiveRunIds early return',
  );
  const gateAt = body.indexOf('vlActiveRunIds.has');
  const patchAt = body.indexOf('succeededFileIds');
  const refreshAt = body.indexOf('updateContent(true)');
  assert.ok(gateAt >= 0 && patchAt > gateAt && refreshAt > patchAt,
    'the badge patch and the membership refresh must follow the toast gate');
});

test('the clear-key button is disabled while a save is in flight (the clear must not be silently dropped)', async () => {
  const source = await read('../src/views/Settings.vue');
  const buttonAt = source.indexOf('@click="clearVlApiKey"');
  assert.ok(buttonAt >= 0, 'Settings.vue must keep the clear-key button');
  const button = source.slice(source.lastIndexOf('<button', buttonAt), buttonAt);
  // Clicking clear during an in-flight save hits the saveVlConfig guard and
  // the pending flag gets reset by that save's finally: the revocation would
  // be silently dropped.
  assert.ok(
    button.includes(':disabled="!vlConfigured || savingVlConfig"'),
    'the clear-key button must be disabled while savingVlConfig is true',
  );
});

test('a successful save writes the normalized view back into the form', async () => {
  const source = await read('../src/views/Settings.vue');
  const body = sliceFunction(source, 'async function saveVlConfig');
  // The backend clamps (concurrency 1-16, default models, trimmed URL);
  // leaving the typed values in the fields sells a "saved" state that was
  // silently corrected. The write-back mirrors loadVlConfig.
  for (const expected of [
    'vlGatewayUrl.value = String(view?.gatewayUrl',
    'vlMaxConcurrency.value = Number(view?.maxConcurrency',
  ]) {
    assert.ok(body.includes(expected), `saveVlConfig must write back ${expected.split(' =')[0]} from the view`);
  }
});

test('the template manager reloads when the main window switches libraries', async () => {
  const source = await read('../src/components/VlTemplateManager.vue');
  assert.ok(source.includes("listen('library-switched'"),
    'VlTemplateManager.vue must listen for library-switched');
  const listenerAt = source.indexOf("listen('library-switched'");
  const listener = source.slice(listenerAt, source.indexOf('});', listenerAt));
  assert.ok(listener.includes('reload'),
    'the library-switched listener must reload the templates');
  assert.ok(source.includes('stopLibraryListener?.()'),
    'the listener must be released on unmount');
});

test('the lab openSettings button also switches the settings window to the VL tab', async () => {
  const source = await read('../src/views/VlLab.vue');
  const body = sliceFunction(source, 'async function openSettings');
  // The settings window syncs its tab through this event (main.js listener);
  // setting the local Pinia tab only moves the lab window's own instance.
  assert.ok(body.includes("emit('settings-settingsTabIndex-changed', VL_SETTINGS_TAB)"),
    'openSettings must broadcast the tab switch to the settings window');
});
