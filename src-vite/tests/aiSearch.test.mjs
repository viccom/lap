import assert from 'node:assert/strict';
import test from 'node:test';

import {
  AI_SEARCH_MODES,
  aiSearchEmptyHint,
  dedupeGroups,
  matchesFileType,
  normalizeAiSearchMode,
  orderFilesByHits,
  passesAiSearchFilters,
  planAiSearch,
  semanticScoreRange,
} from '../src/common/aiSearch.js';

test('the three modes normalize with all as the default', () => {
  assert.deepEqual(AI_SEARCH_MODES, ['precise', 'semantic', 'all']);
  assert.equal(normalizeAiSearchMode(undefined), 'all');
  assert.equal(normalizeAiSearchMode(''), 'all');
  assert.equal(normalizeAiSearchMode('nonsense'), 'all');
  assert.equal(normalizeAiSearchMode('precise'), 'precise');
  assert.equal(normalizeAiSearchMode(' SEMANTIC '), 'semantic');
  assert.equal(normalizeAiSearchMode('all'), 'all');
});

test('each mode plans exactly the legs it needs', () => {
  assert.deepEqual(planAiSearch('precise'), { fts: true, captions: false });
  assert.deepEqual(planAiSearch('semantic'), { fts: false, captions: true });
  assert.deepEqual(planAiSearch('all'), { fts: true, captions: true });
  // An unplanned value never searches with the CLIP-style leftovers: the
  // planner defaults to the all legs instead.
  assert.deepEqual(planAiSearch('unknown'), { fts: true, captions: true });
});

test('the file type mask follows the browser semantics', () => {
  const image = { file_type: 1 };
  const video = { file_type: 2 };
  const raw = { file_type: 3 };
  // 0 (all) and the full mask keep everything.
  for (const mask of [0, 7]) {
    assert.equal(matchesFileType(image, mask), true);
    assert.equal(matchesFileType(video, mask), true);
    assert.equal(matchesFileType(raw, mask), true);
  }
  // bit 1: images, bit 2: videos, bit 4: raw.
  assert.equal(matchesFileType(image, 1), true);
  assert.equal(matchesFileType(raw, 1), false);
  assert.equal(matchesFileType(video, 2), true);
  assert.equal(matchesFileType(image, 2), false);
  assert.equal(matchesFileType(raw, 4), true);
  assert.equal(matchesFileType(video, 4), false);
  // 5 = images + raw (the "Images" browser filter).
  assert.equal(matchesFileType(image, 5), true);
  assert.equal(matchesFileType(raw, 5), true);
  assert.equal(matchesFileType(video, 5), false);
});

test('the client-side filters mirror the unified search behavior', () => {
  const visible = { id: 1, album_visible: true, is_favorite: true, file_type: 1 };
  const hidden = { id: 2, album_visible: false, is_favorite: true, file_type: 1 };
  const unloved = { id: 3, album_visible: true, is_favorite: false, file_type: 1 };
  assert.equal(passesAiSearchFilters(visible), true);
  assert.equal(passesAiSearchFilters(hidden), false, 'album-invisible files never show');
  assert.equal(passesAiSearchFilters(unloved, { favoritesOnly: true }), false);
  assert.equal(passesAiSearchFilters(unloved, { favoritesOnly: false }), true);
  const video = { id: 4, album_visible: true, is_favorite: false, file_type: 2 };
  assert.equal(passesAiSearchFilters(video, { fileType: 5 }), false);
  assert.equal(passesAiSearchFilters(video, { fileType: 2 }), true);
  // A missing file never passes.
  assert.equal(passesAiSearchFilters(null), false);
});

test('hit ids restore their ranking order after the id lookup', () => {
  const hits = [7, 3, 9];
  const files = [
    { id: 3, name: 'c' },
    { id: 9, name: 'd' },
    { id: 7, name: 'a' },
    { id: 1, name: 'unknown to this search' },
  ];
  assert.deepEqual(orderFilesByHits(hits, files).map(file => file.id), [7, 3, 9]);
  // String ids (IPC payloads) and numeric ids match each other.
  assert.deepEqual(orderFilesByHits(['7', '3'], files).map(file => file.id), [7, 3]);
  // Files the lookup did not return are skipped silently.
  assert.deepEqual(orderFilesByHits([42], files), []);
});

test('the all mode dedupes across the legs, fts wins the shared ids', () => {
  const ftsFiles = [{ id: 1 }, { id: 2 }];
  const semanticFiles = [{ id: 2 }, { id: 3 }];
  const merged = dedupeGroups({ ftsFiles, semanticFiles });
  assert.deepEqual(merged.ftsFiles.map(file => file.id), [1, 2]);
  assert.deepEqual(merged.semanticFiles.map(file => file.id), [3]);
  // A leg may be empty.
  const onlySemantic = dedupeGroups({ ftsFiles: [], semanticFiles: [{ id: 5 }] });
  assert.deepEqual(onlySemantic.semanticFiles.map(file => file.id), [5]);
});

test('the semantic score range spans the hits of the group', () => {
  const hits = [
    { fileId: 1, score: 0.78 },
    { fileId: 2, score: 0.46 },
    { fileId: 3, score: 0.6 },
  ];
  assert.deepEqual(semanticScoreRange(hits), { min: 0.46, max: 0.78 });
  assert.equal(semanticScoreRange([{ fileId: 1, score: 0.5 }]).min, 0.5);
  assert.equal(semanticScoreRange([]), null);
  assert.equal(semanticScoreRange(null), null);
});

test('the empty state distinguishes a library without annotations', () => {
  // Nothing annotated yet: the semantic leg has nothing to match against.
  assert.equal(aiSearchEmptyHint({ mode: 'semantic', captions: 0, semanticHits: 0 }), 'no-annotations');
  // Annotated but no match above the threshold.
  assert.equal(aiSearchEmptyHint({ mode: 'semantic', captions: 40, semanticHits: 0 }), 'no-semantic-match');
  // The precise leg and non-empty results need no hint.
  assert.equal(aiSearchEmptyHint({ mode: 'precise', captions: 0, semanticHits: 0 }), null);
  assert.equal(aiSearchEmptyHint({ mode: 'semantic', captions: 40, semanticHits: 3 }), null);
  assert.equal(aiSearchEmptyHint({ mode: 'all', captions: 0, semanticHits: 0, ftsHits: 5 }), null);
});

test('a failed semantic leg is reported as unavailable instead of claiming no match', () => {
  // The embedding call failed (model missing / cloud gateway down): the empty
  // result is not evidence of anything, so the hint must not send the user
  // off to annotate more photos.
  assert.equal(aiSearchEmptyHint({ mode: 'semantic', captions: 40, semanticFailed: true }), 'semantic-unavailable');
  assert.equal(aiSearchEmptyHint({ mode: 'all', captions: 40, semanticFailed: true }), 'semantic-unavailable');
  // Without annotations the "nothing annotated yet" guidance still holds.
  assert.equal(aiSearchEmptyHint({ mode: 'semantic', captions: 0, semanticFailed: true }), 'no-annotations');
  // Precise hits still produced results in all mode: no hint.
  assert.equal(aiSearchEmptyHint({ mode: 'all', captions: 40, ftsHits: 2, semanticFailed: true }), null);
  // Precise mode never runs the semantic leg.
  assert.equal(aiSearchEmptyHint({ mode: 'precise', captions: 40, semanticFailed: true }), null);
  // A healthy semantic search keeps the original hints.
  assert.equal(aiSearchEmptyHint({ mode: 'semantic', captions: 40 }), 'no-semantic-match');
});
