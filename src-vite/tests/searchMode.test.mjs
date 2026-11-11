import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import {
  DEFAULT_SEARCH_MODE,
  MATCH_COLUMN_KEYS,
  SEARCH_MODES,
  SEARCH_RESULT_LIMIT,
  describeMatches,
  isSingleCharQuery,
  normalizeSearchMode,
  planSearch,
} from '../src/common/searchMode.js';
import {
  CULLING,
  DATE_SORT,
  LIB_ITEM,
  QUERY_KIND,
  RATE,
  SIDEBAR,
  TITLE_KIND,
  planNavigationQuery,
} from '../src/common/queryDispatch.js';

const here = path.dirname(fileURLToPath(import.meta.url));
const projectRoot = path.resolve(here, '..');

/// --- search mode ---

test('normalizeSearchMode keeps the known modes', () => {
  assert.deepEqual(SEARCH_MODES, ['all', 'precise', 'semantic']);
  assert.equal(DEFAULT_SEARCH_MODE, 'all');
  for (const mode of SEARCH_MODES) {
    assert.equal(normalizeSearchMode(mode), mode);
  }
});

test('normalizeSearchMode falls back to all for unknown values', () => {
  assert.equal(normalizeSearchMode(undefined), 'all');
  assert.equal(normalizeSearchMode(null), 'all');
  assert.equal(normalizeSearchMode(''), 'all');
  assert.equal(normalizeSearchMode('fuzzy'), 'all');
  assert.equal(normalizeSearchMode(42), 'all');
  assert.equal(normalizeSearchMode('ALL'), 'all');
});

test('planSearch runs the word and the vector query in the default mode', () => {
  const plan = planSearch('all', { text: 'cat', fileId: 0, limit: 200 });
  assert.deepEqual(plan, { lexical: true, vector: true, fts: false, limit: 200 });
});

test('planSearch runs only the full text index in the precise mode', () => {
  const plan = planSearch('precise', { text: '猫', fileId: 0, limit: 50 });
  assert.deepEqual(plan, { lexical: false, vector: false, fts: true, limit: 50 });
});

test('planSearch runs only the vector query in the semantic mode', () => {
  const plan = planSearch('semantic', { text: 'cat', fileId: 0, limit: 200 });
  assert.deepEqual(plan, { lexical: false, vector: true, fts: false, limit: 200 });
});

test('planSearch treats an unknown mode like the default mode', () => {
  const plan = planSearch('nonsense', { text: 'cat' });
  assert.deepEqual(plan, { lexical: true, vector: true, fts: false, limit: SEARCH_RESULT_LIMIT });
});

test('planSearch keeps a usable result limit', () => {
  assert.equal(planSearch('precise', { limit: 0 }).limit, SEARCH_RESULT_LIMIT);
  assert.equal(planSearch('precise', { limit: -5 }).limit, SEARCH_RESULT_LIMIT);
  assert.equal(planSearch('precise', { limit: 'abc' }).limit, SEARCH_RESULT_LIMIT);
  assert.equal(planSearch('precise', { limit: 12.7 }).limit, 12);
  assert.equal(planSearch('precise').limit, SEARCH_RESULT_LIMIT);
});

test('describeMatches renders the matched columns', () => {
  assert.equal(describeMatches([]), '');
  assert.equal(describeMatches(undefined), '');
  assert.equal(describeMatches(['people']), 'People');
  assert.equal(describeMatches(['people', 'tags']), 'People / Tags');
  assert.equal(describeMatches(['tags', 'people']), 'Tags / People');
});

test('describeMatches uses the translated labels and drops duplicates', () => {
  const labels = { people: '人名', tags: '标签' };
  assert.equal(describeMatches(['people', 'tags'], labels), '人名 / 标签');
  assert.equal(describeMatches(['people', 'people'], labels), '人名');
  assert.equal(describeMatches(['caption'], labels), 'Caption');
});

test('describeMatches knows every column of the index', () => {
  assert.deepEqual(Object.keys(MATCH_COLUMN_KEYS), ['name', 'people', 'meta', 'tags', 'caption', 'like']);
});

test('isSingleCharQuery detects a single CJK character', () => {
  assert.equal(isSingleCharQuery('猫'), true);
  assert.equal(isSingleCharQuery(' 猫 '), true);
  assert.equal(isSingleCharQuery('年'), true);
  assert.equal(isSingleCharQuery('年回'), false);
  assert.equal(isSingleCharQuery('cat'), false);
  assert.equal(isSingleCharQuery('c'), false);
  assert.equal(isSingleCharQuery(''), false);
  assert.equal(isSingleCharQuery(undefined), false);
});

/// --- wiring: Content.vue must import every searchMode export it references ---

test('Content.vue imports every searchMode export it references', async () => {
  const source = await readFile(path.join(projectRoot, 'src/components/Content.vue'), 'utf8');

  // The names the module actually exports decide the check.
  const searchModeExports = Object.keys(await import('../src/common/searchMode.js'));

  // The names Content.vue pulls from '@/common/searchMode'.
  const importMatch = source.match(/import\s*\{([^}]*)\}\s*from\s*['"]@\/common\/searchMode['"]/);
  assert.ok(importMatch, 'Content.vue does not import from @/common/searchMode');
  const imported = new Set(importMatch[1].split(',').map((name) => name.trim()).filter(Boolean));

  for (const name of searchModeExports) {
    const referenced = new RegExp(`\\b${name}\\b`).test(source);
    if (!referenced) continue;
    assert.ok(
      imported.has(name),
      `${name} is referenced in Content.vue but missing from the @/common/searchMode import`,
    );
  }
});

/// --- navigation dispatch (the behavior of the former switch in Content.vue) ---

/// A state where nothing is selected; the tests override single fields.
function baseState(overrides = {}) {
  return {
    sidebarIndex: SIDEBAR.LIBRARY,
    library: { item: LIB_ITEM.ALL, smartId: null, smartTag: null },
    album: { id: null, selected: false, folderPath: '', showSubfolderFiles: false, record: null },
    smartAlbum: { type: 'custom', id: 0, items: [] },
    search: { text: '' },
    calendar: { year: null, month: -1, date: -1, startDate: 0, endDate: 0 },
    tag: { id: null },
    person: { id: null, name: '' },
    location: { admin1: null, name: '' },
    camera: { isCamera: true, make: null, model: '', lensMake: null, lensModel: '' },
    rating: { item: RATE.NONE },
    culling: { item: CULLING.UNREVIEWED },
    ...overrides,
  };
}

test('the library panel queries favorites', () => {
  const plan = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.FAV, smartId: null, smartTag: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, { isFavorite: true });
  assert.deepEqual(plan.title, { type: TITLE_KIND.LOCALE, key: 'favorite.files' });
});

test('the library panel queries the files of today', () => {
  const plan = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.TODAY, smartId: null, smartTag: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, { startDate: -1, endDate: -1, calendarSort: DATE_SORT.TAKEN_DESC });
  assert.deepEqual(plan.title, { type: TITLE_KIND.LOCALE, key: 'library.on_this_day' });
});

test('the library panel queries the ratings', () => {
  const rated = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.RATINGS, smartId: null, smartTag: null },
    rating: { item: RATE.ALL },
  }));
  assert.deepEqual(rated.params, { rating: RATE.ALL });
  assert.deepEqual(rated.title, { type: TITLE_KIND.LOCALE_PAIR, key: 'rating.title', subKey: 'rating.rated' });

  const unrated = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.RATINGS, smartId: null, smartTag: null },
    rating: { item: RATE.UNRATED },
  }));
  assert.deepEqual(unrated.params, { rating: RATE.UNRATED });
  assert.deepEqual(unrated.title, { type: TITLE_KIND.LOCALE_PAIR, key: 'rating.title', subKey: 'rating.unrated' });

  const five = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.RATINGS, smartId: null, smartTag: null },
    rating: { item: 5 },
  }));
  assert.deepEqual(five.params, { rating: 5 });
  assert.deepEqual(five.title, { type: TITLE_KIND.RATING, rating: 5 });

  const none = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.RATINGS, smartId: null, smartTag: null },
    rating: { item: RATE.NONE },
  }));
  assert.equal(none.kind, QUERY_KIND.EMPTY);
  assert.deepEqual(none.title, { type: TITLE_KIND.LOCALE, key: 'rating.title' });
});

test('the library panel queries the culling flags', () => {
  const pick = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.CULLING, smartId: null, smartTag: null },
    culling: { item: CULLING.PICK },
  }));
  assert.deepEqual(pick.params, { cullingFlag: 1 });
  assert.deepEqual(pick.title, { type: TITLE_KIND.LOCALE_PAIR, key: 'culling.title', subKey: 'culling.picks' });

  const reject = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.CULLING, smartId: null, smartTag: null },
    culling: { item: CULLING.REJECT },
  }));
  assert.deepEqual(reject.params, { cullingFlag: 2 });
  assert.deepEqual(reject.title, { type: TITLE_KIND.LOCALE_PAIR, key: 'culling.title', subKey: 'culling.rejected' });

  const unreviewed = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.CULLING, smartId: null, smartTag: null },
    culling: { item: CULLING.UNREVIEWED },
  }));
  assert.deepEqual(unreviewed.params, { cullingFlag: 0 });
  assert.deepEqual(unreviewed.title, { type: TITLE_KIND.LOCALE_PAIR, key: 'culling.title', subKey: 'culling.unreviewed' });
});

test('the library panel queries a smart tag with the vector search', () => {
  const plan = planNavigationQuery(baseState({
    library: {
      item: LIB_ITEM.SUBJECTS,
      smartId: 'beach',
      smartTag: { id: 'beach', prompt: 'a beach', threshold: 0.3 },
    },
  }));
  assert.equal(plan.kind, QUERY_KIND.IMAGE_SEARCH);
  assert.equal(plan.prompt, 'a beach');
  assert.equal(plan.threshold, 0.3);
  assert.deepEqual(plan.title, { type: TITLE_KIND.SUBJECT, smartTagId: 'beach' });
});

test('the library panel shows an empty state when the smart tag is unknown', () => {
  const noId = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.SUBJECTS, smartId: null, smartTag: null },
  }));
  assert.equal(noId.kind, QUERY_KIND.EMPTY);
  assert.deepEqual(noId.title, { type: TITLE_KIND.LOCALE, key: 'subject.title' });

  const noTag = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.SUBJECTS, smartId: 'gone', smartTag: null },
  }));
  assert.equal(noTag.kind, QUERY_KIND.EMPTY);
  assert.deepEqual(noTag.title, { type: TITLE_KIND.LOCALE, key: 'subject.title' });
});

test('the library panel falls back to all files', () => {
  const plan = planNavigationQuery(baseState({
    library: { item: LIB_ITEM.ALL, smartId: null, smartTag: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, {});
  assert.deepEqual(plan.title, { type: TITLE_KIND.LOCALE, key: 'library.all_files' });
});

test('the album panel asks for the album record first', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.ALBUM,
    album: { id: 7, selected: true, folderPath: '', showSubfolderFiles: false, record: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.ALBUM);
});

test('the album panel queries a selected album with all its subfolders', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.ALBUM,
    album: {
      id: 7,
      selected: true,
      folderPath: '',
      showSubfolderFiles: false,
      record: { id: 7, name: 'Holiday', path: '/photos/holiday' },
    },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, { searchAllSubfolders: '/photos/holiday' });
  assert.deepEqual(plan.title, { type: TITLE_KIND.ALBUM_NAME, name: 'Holiday' });
});

test('the album panel queries the selected folder', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.ALBUM,
    album: {
      id: 7,
      selected: false,
      folderPath: '/photos/holiday/day1',
      showSubfolderFiles: false,
      record: { id: 7, name: 'Holiday', path: '/photos/holiday' },
    },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, { searchFolder: '/photos/holiday/day1' });
  assert.deepEqual(plan.title, {
    type: TITLE_KIND.BREADCRUMB,
    folderPath: '/photos/holiday/day1',
    albumPath: '/photos/holiday',
  });
});

test('the album panel includes the subfolders when the setting is on', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.ALBUM,
    album: {
      id: 7,
      selected: false,
      folderPath: '/photos/holiday/day1',
      showSubfolderFiles: true,
      record: { id: 7, name: 'Holiday', path: '/photos/holiday' },
    },
  }));
  assert.deepEqual(plan.params, { searchAllSubfolders: '/photos/holiday/day1' });
});

test('the album panel shows all files for the album id zero', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.ALBUM,
    album: { id: 0, selected: false, folderPath: '', showSubfolderFiles: false, record: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, {});
  assert.deepEqual(plan.title, { type: TITLE_KIND.LOCALE, key: 'library.all_files' });
});

test('the album panel stays blank when no album is selected', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.ALBUM,
    album: { id: null, selected: false, folderPath: '', showSubfolderFiles: false, record: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.BLANK);
  assert.deepEqual(plan.title, { type: TITLE_KIND.BLANK });
});

test('the smart album panel queries the selected smart album', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.SMART_ALBUM,
    smartAlbum: {
      type: 'custom',
      id: 3,
      items: [{ id: 3, name: 'Best of' }],
    },
  }));
  assert.equal(plan.kind, QUERY_KIND.SMART_ALBUM);
  assert.deepEqual(plan.smartAlbum, { id: 3, name: 'Best of' });
  assert.deepEqual(plan.title, { type: TITLE_KIND.SMART_ALBUM, name: 'Best of' });
});

test('the smart album panel shows an empty state for an unknown smart album', () => {
  const missing = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.SMART_ALBUM,
    smartAlbum: { type: 'custom', id: 3, items: [] },
  }));
  assert.equal(missing.kind, QUERY_KIND.EMPTY);

  const wrongType = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.SMART_ALBUM,
    smartAlbum: { type: 'system', id: 3, items: [{ id: 3, name: 'Best of' }] },
  }));
  assert.equal(wrongType.kind, QUERY_KIND.EMPTY);
});

test('the search panel runs the unified search', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.SEARCH,
    search: { text: 'cat' },
  }));
  assert.equal(plan.kind, QUERY_KIND.UNIFIED_SEARCH);
  assert.equal(plan.text, 'cat');
  assert.deepEqual(plan.title, { type: TITLE_KIND.SEARCH, text: 'cat' });
});

test('the search panel shows an empty state without a query', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.SEARCH,
    search: { text: '' },
  }));
  assert.equal(plan.kind, QUERY_KIND.EMPTY);
  assert.deepEqual(plan.title, { type: TITLE_KIND.LOCALE, key: 'search.title' });
});

test('the calendar panel queries the selected range', () => {
  const year = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CALENDAR,
    calendar: { year: 2024, month: -1, date: -1, startDate: 1, endDate: 2 },
  }));
  assert.equal(year.kind, QUERY_KIND.FILES);
  assert.deepEqual(year.params, { startDate: 1, endDate: 2 });
  assert.equal(year.title.format, 'year');

  const month = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CALENDAR,
    calendar: { year: 2024, month: 5, date: -1, startDate: 3, endDate: 4 },
  }));
  assert.equal(month.title.format, 'month');

  const day = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CALENDAR,
    calendar: { year: 2024, month: 5, date: 6, startDate: 5, endDate: 6 },
  }));
  assert.equal(day.title.format, 'date_long');
});

test('the calendar panel shows an empty state without a year', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CALENDAR,
    calendar: { year: null, month: -1, date: -1, startDate: 0, endDate: 0 },
  }));
  assert.equal(plan.kind, QUERY_KIND.EMPTY);
  assert.deepEqual(plan.title, { type: TITLE_KIND.BLANK });
});

test('the tag panel queries the selected tag', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.TAG,
    tag: { id: 7 },
  }));
  assert.equal(plan.kind, QUERY_KIND.TAG);
  assert.deepEqual(plan.params, { tagId: 7 });
  assert.equal(plan.tagId, 7);
});

test('the tag panel shows an empty state without a tag', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.TAG,
    tag: { id: null },
  }));
  assert.equal(plan.kind, QUERY_KIND.EMPTY);
  assert.deepEqual(plan.title, { type: TITLE_KIND.BLANK });
});

test('the person panel queries the selected person', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.PERSON,
    person: { id: 3, name: 'Alice' },
  }));
  assert.equal(plan.kind, QUERY_KIND.FILES);
  assert.deepEqual(plan.params, { personId: 3 });
  assert.deepEqual(plan.title, { type: TITLE_KIND.LOCALE_OR_TEXT, key: 'sidebar.people', text: 'Alice' });
});

test('the person panel shows an empty state without a person', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.PERSON,
    person: { id: null, name: '' },
  }));
  assert.equal(plan.kind, QUERY_KIND.EMPTY);
});

test('the location panel queries the selected place', () => {
  const place = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.LOCATION,
    location: { admin1: 'Bavaria', name: 'Munich' },
  }));
  assert.deepEqual(place.params, { locationAdmin1: 'Bavaria', locationName: 'Munich' });
  assert.deepEqual(place.title, { type: TITLE_KIND.TEXT, text: 'Bavaria > Munich' });

  const region = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.LOCATION,
    location: { admin1: 'Bavaria', name: '' },
  }));
  assert.deepEqual(region.params, { locationAdmin1: 'Bavaria' });
  assert.deepEqual(region.title, { type: TITLE_KIND.TEXT, text: 'Bavaria' });
});

test('the location panel shows an empty state without a region', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.LOCATION,
    location: { admin1: null, name: '' },
  }));
  assert.equal(plan.kind, QUERY_KIND.EMPTY);
});

test('the camera panel queries the selected camera', () => {
  const camera = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CAMERA,
    camera: { isCamera: true, make: 'Canon', model: 'EOS R5', lensMake: null, lensModel: '' },
  }));
  assert.deepEqual(camera.params, { make: 'Canon', model: 'EOS R5' });
  assert.deepEqual(camera.title, { type: TITLE_KIND.TEXT, text: 'Canon > EOS R5' });

  const makeOnly = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CAMERA,
    camera: { isCamera: true, make: 'Canon', model: '', lensMake: null, lensModel: '' },
  }));
  assert.deepEqual(makeOnly.params, { make: 'Canon' });
  assert.deepEqual(makeOnly.title, { type: TITLE_KIND.TEXT, text: 'Canon' });
});

test('the camera panel queries the selected lens', () => {
  const lens = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CAMERA,
    camera: { isCamera: false, make: null, model: '', lensMake: 'Sigma', lensModel: '35mm' },
  }));
  assert.deepEqual(lens.params, { lensMake: 'Sigma', lensModel: '35mm' });
  assert.deepEqual(lens.title, { type: TITLE_KIND.TEXT, text: 'Sigma > 35mm' });

  const lensMakeOnly = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CAMERA,
    camera: { isCamera: false, make: null, model: '', lensMake: 'Sigma', lensModel: '' },
  }));
  assert.deepEqual(lensMakeOnly.params, { lensMake: 'Sigma' });
  assert.deepEqual(lensMakeOnly.title, { type: TITLE_KIND.TEXT, text: 'Sigma' });
});

test('the camera panel shows an empty state without a camera', () => {
  const plan = planNavigationQuery(baseState({
    sidebarIndex: SIDEBAR.CAMERA,
    camera: { isCamera: true, make: null, model: '', lensMake: null, lensModel: '' },
  }));
  assert.equal(plan.kind, QUERY_KIND.EMPTY);
});

test('an unknown sidebar index runs nothing', () => {
  const plan = planNavigationQuery(baseState({ sidebarIndex: 99 }));
  assert.equal(plan.kind, QUERY_KIND.NONE);
});

/// --- the enums of the planner and of the app have to stay in sync ---

test('the planner enums match the enums of the app', async () => {
  const source = await readFile(path.join(projectRoot, 'src/common/constants.ts'), 'utf8');

  const readBlock = (name) => {
    const start = source.indexOf(`export const ${name} = {`);
    assert.notEqual(start, -1, `${name} is missing in constants.ts`);
    const end = source.indexOf('};', start);
    return source.slice(start, end);
  };

  const sidebar = readBlock('SIDEBAR');
  for (const [key, value] of Object.entries(SIDEBAR)) {
    assert.match(sidebar, new RegExp(`${key}:\\s*${value}\\b`), `SIDEBAR.${key} drifted`);
  }

  const libItem = readBlock('LIB_ITEM');
  for (const [key, value] of Object.entries(LIB_ITEM)) {
    assert.match(libItem, new RegExp(`${key}:\\s*'${value}'`), `LIB_ITEM.${key} drifted`);
  }

  const rate = readBlock('RATE');
  for (const [key, value] of Object.entries(RATE)) {
    assert.match(rate, new RegExp(`${key}:\\s*${value}\\b`), `RATE.${key} drifted`);
  }

  const culling = readBlock('CULLING');
  for (const [key, value] of Object.entries(CULLING)) {
    assert.match(culling, new RegExp(`${key}:\\s*'${value}'`), `CULLING.${key} drifted`);
  }

  const dateSort = readBlock('DATE_SORT');
  for (const [key, value] of Object.entries(DATE_SORT)) {
    assert.match(dateSort, new RegExp(`${key}:\\s*${value}\\b`), `DATE_SORT.${key} drifted`);
  }
});

/// --- translations ---

const SEARCH_KEYS = [
  'search.mode.label',
  'search.mode.all',
  'search.mode.precise',
  'search.mode.semantic',
  'search.mode.single_char_hint',
  'search.matches_from',
  'search.precise_matches',
  'search.match_column.name',
  'search.match_column.people',
  'search.match_column.meta',
  'search.match_column.tags',
  'search.match_column.caption',
  'search.match_column.like',
];

function lookup(message, dottedKey) {
  return dottedKey.split('.').reduce((value, part) => (value == null ? undefined : value[part]), message);
}

test('every locale has the search mode translations', async () => {
  const localesDir = path.join(projectRoot, 'src/locales');
  const files = (await readdir(localesDir)).filter((name) => name.endsWith('.json')).sort();
  assert.equal(files.length, 17, 'the number of locale files changed');

  for (const file of files) {
    const message = JSON.parse(await readFile(path.join(localesDir, file), 'utf8'));
    for (const key of SEARCH_KEYS) {
      const value = lookup(message, key);
      assert.equal(typeof value, 'string', `${file} misses ${key}`);
      assert.ok(value.length > 0, `${file} has an empty ${key}`);
    }
  }
});
