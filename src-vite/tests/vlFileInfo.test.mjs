import ts from 'typescript';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const source = readFileSync(new URL('../src/components/FileInfo.vue', import.meta.url), 'utf8')
  .replace(/\r\n/g, '\n');

// The watch statement that reacts to a file switch by refreshing the VL
// panel. Extracted between its leading comment and the next stable block.
const start = source.indexOf('// A file switch must refresh');
const end = source.indexOf('// A finished run may have annotated');
assert.ok(
  start >= 0 && end > start,
  'FileInfo must watch the file identity and refresh the VL panel on file switches'
    + ' (bug: after switching images the understanding text stayed on the previous photo)',
);

const slice = source.slice(start, end);
const js = ts.transpile(slice, { target: ts.ScriptTarget.ES2022 });

function mountPanel(panelOpen) {
  let captured = null;
  let reloads = 0;
  const deps = {
    showVlPanel: { value: panelOpen },
    vlAnnotations: { value: [{ id: 9 }] },
    loadVlAnnotations: async () => { reloads += 1; },
  };
  const run = new Function(
    'watch', 'showVlPanel', 'vlAnnotations', 'loadVlAnnotations',
    js + '\nreturn 0;'
  );
  run(
    (getter, cb) => { captured = { getter, cb }; },
    deps.showVlPanel, deps.vlAnnotations, deps.loadVlAnnotations
  );
  assert.ok(captured, 'the slice must register a watcher');
  const identity = String(captured.getter);
  assert.match(identity, /props\.fileInfo\?\.id/, 'the watcher must key on the file id');
  return { captured, deps, reloads: () => reloads };
}

test('an open VL panel reloads its annotations when the shown file changes', async () => {
  const panel = mountPanel(true);
  await panel.captured.cb([], ['lib-1', 5, 'lib-1', 6]);
  assert.equal(panel.reloads(), 1, 'switching files with the panel open must reload');
  assert.deepEqual(panel.deps.vlAnnotations.value, [{ id: 9 }], 'the reload owns the list, no eager wipe');
});

test('a closed VL panel drops the stale annotations on a file change', async () => {
  const panel = mountPanel(false);
  await panel.captured.cb([], ['lib-1', 5, 'lib-1', 6]);
  assert.equal(panel.reloads(), 0, 'a closed panel must not fetch');
  assert.deepEqual(panel.deps.vlAnnotations.value, [], 'stale annotations from the previous file must be cleared');
});
