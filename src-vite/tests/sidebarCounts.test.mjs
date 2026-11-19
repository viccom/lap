import ts from 'typescript';
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { toPersistedLibraryState, toRuntimeLibraryState } from '../src/common/libraryState.js';
const source = readFileSync(new URL('../src/components/Content.vue', import.meta.url), 'utf8');
const body = source.slice(source.indexOf('const getSidebarQueryCountAndSum ='), source.indexOf('async function getSearchSidebarCountAndSum'));
test('sidebar query and collection totals ignore browser file type without changing the content query', async () => {
  for (const kind of ['query', 'collection']) {
    for (const mask of [0, 2, 4, 5]) {
      const params = { searchFileType: mask, searchFolder: '/photos', tagId: 3 };
      let received;
      const deps = { currentQuerySource: { value: kind }, currentQueryParams: { value: params },
        currentCollectionId: { value: 42 }, getCurrentQueryCountAndSum: () => {},
        getCollectionCountAndSum: (id, p) => { assert.equal(id, 42); received = p; },
        getQueryCountAndSum: p => { received = p; } };
      const fn = new Function(...Object.keys(deps), body + ';return getSidebarQueryCountAndSum;')(...Object.values(deps));
      await fn();
      assert.deepEqual(received, { ...params, searchFileType: 0 });
      assert.equal(params.searchFileType, mask);
    }
  }
});

test('search sidebar count queries All and deduplicates text and visual matches', async () => {
  const start = source.indexOf('async function getSearchSidebarCountAndSum');
  const body = source.slice(start, source.indexOf('async function refreshRequestedSidebarCount', start));
  const text = { searchFileType: 2, searchFileName: 'cat' };
  const image = { fileType: 2, searchText: 'cat' };
  const deps = { currentQueryParams: {value: text}, currentImageSearchParams: {value: image},
    getMediaItemSize: file => Number(file?.size ?? 0) + Number(file?.companion_size ?? 0),
    imageSearchLanguageUnsupported: {value: false}, passesAlbumFilters: f => f.id !== 4,
    getQueryFiles: async p => { assert.equal(p.searchFileType, 0); return [{id:1,size:10},{id:2,size:20}]; },
    searchSimilarImages: async p => { assert.equal(p.fileType, 0); return [{id:2,size:20},{id:3,size:30},{id:4,size:40}]; } };
  const fn = new Function(...Object.keys(deps), ts.transpile(body, {target: ts.ScriptTarget.ES2022}) + ';return getSearchSidebarCountAndSum;')(...Object.values(deps));
  assert.deepEqual(await fn(), [3, 60]);
  assert.equal(text.searchFileType, 2);
  assert.equal(image.fileType, 2);
});

test('a subject count comes from the subject query, never from the library query', async () => {
  const start = source.indexOf('async function getSubjectCountAndSum');
  assert.ok(start > 0, 'getSubjectCountAndSum is missing');
  const body = source.slice(start, source.indexOf('async function getRequestedSidebarCountAndSum', start));
  let received;
  const deps = {
    getSmartTagById: id => (id === 'night' ? { id, prompt: 'a nighttime photo' } : null),
    config: { smartTagSearchThreshold: 0.25 },
    searchSimilarImages: async params => { received = params; return [{ id: 1, size: 10 }, { id: 2, size: 20 }]; },
    passesAlbumFilters: file => file.id !== 2,
    getMediaItemSize: file => Number(file?.size ?? 0),
  };
  const fn = new Function(...Object.keys(deps), ts.transpile(body, { target: ts.ScriptTarget.ES2022 }) + ';return getSubjectCountAndSum;')(...Object.values(deps));
  // The subject prompt decides the count; the album filters still apply.
  assert.deepEqual(await fn('night'), [1, 10]);
  assert.deepEqual(received, { searchText: 'a nighttime photo', fileId: 0, threshold: 0.25, fileType: 0 });
  // An unknown subject has no query at all.
  assert.equal(await fn('nope'), null);
});

test('the requested sidebar count routes a subject to the subject counter', async () => {
  const start = source.indexOf('async function getRequestedSidebarCountAndSum');
  assert.ok(start > 0, 'getRequestedSidebarCountAndSum is missing');
  const body = source.slice(start, source.indexOf('async function getAiSearchSidebarCountAndSum', start));
  const calls = [];
  const deps = {
    LIB_ITEM: { SUBJECTS: 'subjects' },
    getSubjectCountAndSum: async id => { calls.push(['subject', id]); return [7, 70]; },
    getAiSearchSidebarCountAndSum: async text => { calls.push(['ai', text]); return [1, 1]; },
    getSidebarQueryCountAndSum: async () => { calls.push(['ambient']); return [2773, 0]; },
  };
  const fn = new Function(...Object.keys(deps), ts.transpile(body, { target: ts.ScriptTarget.ES2022 }) + ';return getRequestedSidebarCountAndSum;')(...Object.values(deps));
  assert.deepEqual(await fn({ source: 'library', item: 'subjects', smartId: 'night' }), [7, 70]);
  assert.deepEqual(calls, [['subject', 'night']]);
  calls.length = 0;
  assert.deepEqual(await fn({ source: 'library', item: 'all-files' }), [2773, 0]);
  assert.deepEqual(calls, [['ambient']]);
  calls.length = 0;
  assert.deepEqual(await fn({ source: 'ai-search', text: 'cat' }), [1, 1]);
  assert.deepEqual(calls, [['ai', 'cat']]);
});

test('the aggregate counts of the library are runtime-only', () => {
  const library = {
    item: 'subjects',
    smartId: 'night',
    activateTick: 3,
    subjectCounts: { night: 2773 },
    counts: { all: 2773 },
  };
  // A persisted count would show a stale number for a panel that was never
  // opened in the next session.
  assert.deepEqual(toPersistedLibraryState(library), { item: 'subjects', smartId: 'night' });
  assert.deepEqual(toRuntimeLibraryState(library), {
    item: 'subjects',
    smartId: 'night',
    activateTick: 3,
    subjectCounts: {},
    counts: { all: 2773 },
  });
  assert.deepEqual(toPersistedLibraryState({}), {});
  assert.deepEqual(toRuntimeLibraryState({}), { subjectCounts: {} });
});
