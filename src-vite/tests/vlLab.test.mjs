import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import test from 'node:test';

import {
  LAB_MAX_FILES,
  LAB_POLL_INTERVAL_MS,
  LAB_POLL_LIMIT_MS,
  OUTCOME_LABEL_KEYS,
  annotatedIdSet,
  applyRerankSelection,
  buildLabQueryParams,
  buildLabRunArgs,
  captionHitsFrom,
  collectMatchedColumns,
  deriveFileOutcome,
  ftsReindex,
  getFtsStatus,
  hitBadges,
  indexIsConsistent,
  needsBatchConfirm,
  pickerRows,
  rerankHitList,
  rerankSearchWithLlm,
  searchFts,
  shouldFinishRun,
} from '../src/common/vlLabLogic.js';

/// An invoker that records the calls and answers with the given result.
function recorder(result = null) {
  const calls = [];
  const invoker = async (command, args) => {
    calls.push({ command, args });
    return typeof result === 'function' ? result(command, args) : result;
  };
  return { calls, invoker };
}

test('the picker caps the file list and polls with the panel margins', () => {
  assert.equal(LAB_MAX_FILES, 200, 'at most 200 files are loaded');
  assert.equal(LAB_POLL_LIMIT_MS, 200_000, 'polling gives up after 200 s');
  assert.equal(LAB_POLL_INTERVAL_MS, 3000, 'the poll runs every 3 s');
});

test('the lab query asks for the whole library with the default filters', () => {
  assert.deepEqual(buildLabQueryParams(), {
    searchFileType: 0,
    sortType: 0,
    sortOrder: 0,
    randomSeed: 0,
    searchFileName: '',
    searchAllSubfolders: '',
    searchFolder: '',
    startDate: 0,
    endDate: 0,
    calendarSort: 0,
    folderSort: 0,
    categorySort: 0,
    make: '',
    model: '',
    lensMake: '',
    lensModel: '',
    locationAdmin1: '',
    locationName: '',
    isFavorite: false,
    rating: -1,
    cullingFlag: -1,
    tagId: 0,
    personId: 0,
  });
});

test('annotatedIdSet marks only the files with an annotation for the template', () => {
  const annotationsByFileId = {
    1: [{ id: 10, templateId: 4 }],
    2: [{ id: 11, templateId: 5 }],
    3: [],
  };
  const ids = annotatedIdSet(annotationsByFileId, 4);
  assert.equal(ids.has(1), true);
  assert.equal(ids.has(2), false, 'a different template is not an idempotence hit');
  assert.equal(ids.has(3), false);
  assert.deepEqual(annotatedIdSet(null, 4).size, 0);
});

test('picker rows carry the selection and the idempotence mark', () => {
  const files = [{ id: 1, name: 'a.jpg' }, { id: 2, name: 'b.jpg' }, { id: 3, name: 'c.jpg' }];
  const annotated = new Set([2]);
  const rows = pickerRows(files, [2, '3'], annotated);
  assert.deepEqual(rows.map(row => [row.id, row.selected, row.hasExisting]), [
    [1, false, false],
    [2, true, true],
    [3, true, false],
  ]);
  assert.equal(rows[0].file.name, 'a.jpg', 'the row keeps the file');
});

test('a batch over more than one file asks for a confirmation', () => {
  assert.equal(needsBatchConfirm(0), false);
  assert.equal(needsBatchConfirm(1), false, 'a single photo runs directly');
  assert.equal(needsBatchConfirm(2), true);
  assert.equal(needsBatchConfirm(40), true);
});

test('the run arguments stay numeric and carry the force flag', () => {
  assert.deepEqual(buildLabRunArgs(['8', 9], '5', 1), { fileIds: [8, 9], templateId: 5, force: true });
  assert.deepEqual(buildLabRunArgs([], 5, false), { fileIds: [], templateId: 5, force: false });
});

test('a new annotation means success', () => {
  const outcome = deriveFileOutcome({
    force: false,
    baselineAnnotationId: 10,
    baselineCallId: 20,
    annotations: [{ id: 11, templateId: 4 }],
    calls: [{ id: 21 }],
  });
  assert.equal(outcome.status, 'success');
  assert.equal(outcome.annotation.id, 11);
});

test('an existing annotation without force means skipped', () => {
  const outcome = deriveFileOutcome({
    force: false,
    baselineAnnotationId: 10,
    baselineCallId: 20,
    annotations: [{ id: 10, templateId: 4 }],
    calls: [{ id: 20 }],
  });
  assert.equal(outcome.status, 'skipped');
  assert.equal(outcome.annotation.id, 10, 'the kept annotation is shown');
});

test('a new call without an annotation means failed', () => {
  const outcome = deriveFileOutcome({
    force: true,
    baselineAnnotationId: 10,
    baselineCallId: 20,
    annotations: [{ id: 10, templateId: 4 }],
    calls: [{ id: 21, error: 'gateway down' }],
  });
  assert.equal(outcome.status, 'failed');
  assert.equal(outcome.call.error, 'gateway down', 'the failed call carries the reason');
});

test('a forced rerun that refreshes the existing annotation in place means success', () => {
  // The backend upserts on a forced rerun: the annotation row keeps its id
  // but its callId moves to the new successful call.
  const outcome = deriveFileOutcome({
    force: true,
    baselineAnnotationId: 10,
    baselineCallId: 20,
    annotations: [{ id: 10, templateId: 4, callId: 21 }],
    calls: [{ id: 20 }, { id: 21, status: 'ok' }],
  });
  assert.equal(outcome.status, 'success');
  assert.equal(outcome.annotation.id, 10, 'the refreshed annotation is shown');
});

test('no new evidence means the file was not reached', () => {
  const cancelled = deriveFileOutcome({
    force: false,
    baselineAnnotationId: 0,
    baselineCallId: 0,
    annotations: [],
    calls: [],
  });
  assert.equal(cancelled.status, 'notRun');

  // force without any new evidence is also "not reached", never "skipped".
  const forced = deriveFileOutcome({
    force: true,
    baselineAnnotationId: 10,
    baselineCallId: 20,
    annotations: [{ id: 10 }],
    calls: [{ id: 20 }],
  });
  assert.equal(forced.status, 'notRun');
});

test('every outcome has a label key', () => {
  assert.deepEqual(Object.keys(OUTCOME_LABEL_KEYS).sort(), ['failed', 'notRun', 'skipped', 'success']);
  for (const key of Object.values(OUTCOME_LABEL_KEYS)) {
    assert.equal(key.startsWith('vl.lab.outcome_'), true, key);
  }
});

test('the index is consistent when both counts match', () => {
  assert.equal(indexIsConsistent({ documents: 404, files: 404 }), true);
  assert.equal(indexIsConsistent({ documents: 3, files: 4 }), false);
  assert.equal(indexIsConsistent(null), true, 'a missing status shows no warning');
});

test('hit badges map the matched columns to their i18n keys', () => {
  const badges = hitBadges({ fileId: 7, matchedColumns: ['name', 'like', 'bogus'] });
  assert.deepEqual(badges, [
    { column: 'name', labelKey: 'search.match_column.name' },
    { column: 'like', labelKey: 'search.match_column.like' },
    { column: 'bogus', labelKey: null },
  ]);
});

test('the matched columns of all hits are collected without duplicates', () => {
  const columns = collectMatchedColumns([
    { fileId: 1, matchedColumns: ['name', 'tags'] },
    { fileId: 2, matchedColumns: ['tags', 'caption'] },
  ]);
  assert.deepEqual(columns, ['name', 'tags', 'caption']);
});

test('the fts invoke wrappers send the documented commands', async () => {
  const status = recorder({ documents: 4, files: 4 });
  assert.deepEqual(await getFtsStatus(status.invoker), { documents: 4, files: 4 });
  assert.deepEqual(status.calls, [{ command: 'fts_status', args: undefined }]);

  const reindex = recorder(4);
  assert.equal(await ftsReindex(reindex.invoker), 4);
  assert.deepEqual(reindex.calls, [{ command: 'fts_reindex', args: undefined }]);

  const search = recorder([{ fileId: 1, matchedColumns: ['name'] }]);
  const hits = await searchFts('cat', 200, search.invoker);
  assert.equal(hits.length, 1);
  assert.deepEqual(search.calls, [{ command: 'search_fts', args: { query: 'cat', limit: 200 } }]);
  // the default limit matches the search result limit of the app
  const plain = recorder([]);
  await searchFts('cat', undefined, plain.invoker);
  assert.equal(plain.calls[0].args.limit, 200);
});

test('every locale carries the lab keys', async () => {
  const directory = new URL('../src/locales/', import.meta.url);
  const files = (await readdir(directory)).filter(name => name.endsWith('.json'));
  assert.equal(files.length, 17);
  const english = JSON.parse(await readFile(new URL('en.json', directory), 'utf8'));
  const expectedKeys = Object.keys(english.vl.lab).sort();
  assert.ok(expectedKeys.includes('title'));
  assert.ok(expectedKeys.includes('outcome_not_run'));
  assert.ok(expectedKeys.length >= 30, 'the lab page has a substantial key set');

  for (const file of files) {
    const messages = JSON.parse(await readFile(new URL(file, directory), 'utf8'));
    const lab = messages.vl?.lab ?? {};
    assert.deepEqual(Object.keys(lab).sort(), expectedKeys, `${file} misses vl.lab keys`);
    for (const key of expectedKeys) {
      assert.equal(typeof lab[key], 'string', `${file} misses vl.lab.${key}`);
      assert.ok(lab[key].trim().length > 0, `${file} has an empty vl.lab.${key}`);
    }
    assert.equal(typeof messages.settings?.vl?.open_lab, 'string', `${file} misses settings.vl.open_lab`);
  }

  // The Chinese locales carry a real translation, not the English placeholder.
  for (const file of ['zh-CN.json', 'zh-TW.json']) {
    const messages = JSON.parse(await readFile(new URL(file, directory), 'utf8'));
    assert.notEqual(messages.vl.lab.title, english.vl.lab.title, file);
    assert.notEqual(messages.settings.vl.open_lab, english.settings.vl.open_lab, file);
  }
});

test('toggleSelect writes the selection before awaiting (rapid clicks cannot lose each other)', async () => {
  const source = await readFile(new URL('../src/views/VlLab.vue', import.meta.url), 'utf8');
  const start = source.indexOf('async function toggleSelect');
  assert.ok(start >= 0, 'VlLab.vue must define toggleSelect');
  const body = source.slice(start, source.indexOf('\n}', start));
  // The read-modify-write of selectedIds must not straddle an await: two
  // clicks inside the annotation-fetch round-trip each start from the same
  // stale set, and the later write silently drops the earlier selection.
  const writeAt = body.indexOf('selectedIds.value = next');
  const firstAwait = body.indexOf('await ');
  assert.ok(writeAt >= 0, 'toggleSelect must write selectedIds.value = next');
  assert.ok(firstAwait < 0 || writeAt < firstAwait,
    'selectedIds.value = next must run before the first await in toggleSelect');
  // The annotation cache write must not await inside the assigned object
  // literal: the spread would snapshot the cache before the await, and a
  // sibling toggle resolving in between would be dropped by the stale snapshot.
  assert.ok(
    !/annotationsByFileId\.value\s*=\s*\{[^}]*await/.test(body),
    'annotationsByFileId.value must be assigned from a spread taken after the await, not inside an object literal containing await',
  );
});

test('a rerank selection keeps only the hits the model judged relevant', () => {
  const hits = [
    { fileId: 7, matchedColumns: ['caption'] },
    { fileId: 9, matchedColumns: ['name'] },
    { fileId: 11, matchedColumns: ['tags'] },
  ];
  assert.deepEqual(applyRerankSelection(hits, [11, 7]).map(hit => hit.fileId), [11, 7]);
  // An invented id and a duplicate are ignored; omitted hits are dropped.
  assert.deepEqual(applyRerankSelection(hits, [99, 11, 11]).map(hit => hit.fileId), [11]);
  assert.equal(applyRerankSelection(hits, [11])[0].matchedColumns[0], 'tags');
  // An empty selection means nothing matched.
  assert.deepEqual(applyRerankSelection(hits, []), []);
});

test('rerankHitList keeps matches and appends hits without a description', async () => {
  const calls = [];
  const invoker = async (command, args) => {
    calls.push({ command, args });
    return { matched: [9], unannotated: [11] };
  };
  const hits = [
    { fileId: 7, matchedColumns: ['caption'] },
    { fileId: 9, matchedColumns: ['name'] },
    { fileId: 11, matchedColumns: ['tags'] },
  ];
  const result = await rerankHitList('摘葡萄', hits, invoker);
  assert.deepEqual(calls, [
    { command: 'rerank_search_with_llm', args: { query: '摘葡萄', fileIds: [7, 9, 11] } },
  ]);
  assert.equal(result.reranked, true);
  // 7 was rejected by the model, 9 matched, 11 could not be judged.
  assert.deepEqual(result.hits.map(hit => hit.fileId), [9, 11]);
  assert.equal(result.unannotatedCount, 1);
  // A single candidate is still verified; an empty list is not sent.
  await rerankHitList('摘葡萄', [hits[0]], invoker);
  assert.deepEqual(calls.at(-1).args.fileIds, [7]);
  await rerankHitList('摘葡萄', [], invoker);
  assert.equal(calls.length, 2);
});

test('the rerank invoke sends the command and numeric ids', async () => {
  const calls = [];
  const invoker = async (command, args) => {
    calls.push({ command, args });
    return { matched: [9, 7], unannotated: [] };
  };
  const order = await rerankSearchWithLlm('摘葡萄', [7, '9'], invoker);
  assert.deepEqual(order, { matched: [9, 7], unannotated: [] });
  assert.deepEqual(calls, [
    { command: 'rerank_search_with_llm', args: { query: '摘葡萄', fileIds: [7, 9] } },
  ]);
});

test('captionHitsFrom maps search_captions rows into ranked hits with scores', () => {
  const hits = captionHitsFrom([
    { fileId: 9, score: 0.8123 },
    { fileId: '11', score: '0.55' },
  ]);
  assert.deepEqual(hits, [
    { fileId: 9, score: 0.8123, matchedColumns: [], rank: 1 },
    { fileId: 11, score: 0.55, matchedColumns: [], rank: 2 },
  ]);
  assert.deepEqual(captionHitsFrom(null), []);
  assert.deepEqual(captionHitsFrom([]), []);
});

test('shouldFinishRun lets a finished event land the results even after the poll gave up', () => {
  assert.equal(shouldFinishRun(true, false, 'run-1', 'run-1'), true);
  // The poll gave up waiting but the id matches: the run outlived our
  // patience, its results must still load.
  assert.equal(shouldFinishRun(false, true, 'run-1', 'run-1'), true);
  assert.equal(shouldFinishRun(false, false, 'run-1', 'run-1'), false);
  assert.equal(shouldFinishRun(true, false, 'run-1', 'run-2'), false);
  assert.equal(shouldFinishRun(true, false, null, 'run-1'), false);
  assert.equal(shouldFinishRun(true, false, 'run-1', ''), false);
  // Ids are compared as strings; numbers pass through unharmed.
  assert.equal(shouldFinishRun(true, false, 123, '123'), true);
});

test('startRun claims the busy flag before the first await (a second click cannot queue a duplicate run)', async () => {
  const source = await readFile(new URL('../src/views/VlLab.vue', import.meta.url), 'utf8');
  const start = source.indexOf('async function startRun');
  assert.ok(start >= 0, 'VlLab.vue must define startRun');
  const body = source.slice(start, source.indexOf('\n}', start));
  // The baselines cost one IPC per file; without a synchronous flag a second
  // click inside that window passes the running guard twice and queues two runs.
  const guardAt = body.indexOf('starting.value = true');
  const firstAwait = body.indexOf('await ');
  assert.ok(guardAt >= 0, 'startRun must set starting.value = true');
  assert.ok(firstAwait < 0 || guardAt < firstAwait,
    'starting.value = true must run before the first await in startRun');
  const canRunAt = source.indexOf('const canRun = computed(');
  assert.ok(canRunAt >= 0, 'VlLab.vue must define canRun');
  const canRunBody = source.slice(canRunAt, source.indexOf('\n', canRunAt));
  assert.ok(canRunBody.includes('!starting.value'), 'canRun must include !starting.value');
});

test('giving up the poll keeps the run id so the finished event can still load the results', async () => {
  const source = await readFile(new URL('../src/views/VlLab.vue', import.meta.url), 'utf8');
  const start = source.indexOf('async function checkRunFinished');
  assert.ok(start >= 0, 'VlLab.vue must define checkRunFinished');
  const body = source.slice(start, source.indexOf('\n}', start));
  const giveUpAt = body.indexOf('LAB_POLL_LIMIT_MS)');
  assert.ok(giveUpAt >= 0, 'checkRunFinished must keep the give-up branch');
  const giveUpBody = body.slice(giveUpAt, body.indexOf('return;', giveUpAt));
  assert.ok(!giveUpBody.includes('resetWaitState()'),
    'the give-up branch must not reset the run id: the finished event needs it to land the results');
  assert.ok(giveUpBody.includes('awaitingFinish.value = true'),
    'the give-up branch must enter the awaitingFinish state');
  const listenerAt = source.indexOf("listen('vl-analysis-finished'");
  assert.ok(listenerAt >= 0, 'VlLab.vue must listen for vl-analysis-finished');
  const listenerBody = source.slice(listenerAt, source.indexOf('});', listenerAt));
  assert.ok(listenerBody.includes('shouldFinishRun'),
    'the finished listener must route through shouldFinishRun');
});

test('a library switch in the main window resets the lab window instead of leaving stale ids selectable', async () => {
  const source = await readFile(new URL('../src/views/VlLab.vue', import.meta.url), 'utf8');
  // The lab is a separate window: it must hear the switch itself.
  assert.ok(source.includes("listen('library-switched'"),
    'VlLab.vue must listen for library-switched');
  const resetAt = source.indexOf('function resetAfterLibrarySwitch');
  assert.ok(resetAt >= 0, 'VlLab.vue must define resetAfterLibrarySwitch');
  const body = source.slice(resetAt, source.indexOf('\n}', resetAt));
  // Every id loaded here belongs to the previous library; a run started from
  // them would annotate whatever file carries the same id in the new library.
  for (const required of [
    'files.value = []',
    'selectedIds.value = new Set()',
    'runId.value = null',
    'resultRows.value = []',
  ]) {
    assert.ok(body.includes(required), `resetAfterLibrarySwitch must clear ${required}`);
  }
});
