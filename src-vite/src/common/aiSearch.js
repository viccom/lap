/**
 * AI Search — the search-entry planner of the image-understanding feature.
 *
 * Pure logic only: which legs a mode runs, how the hit lists reassemble and
 * which filters apply client-side. The invoke calls live in the components.
 *
 * This planner is deliberately independent of the official `searchMode.js`:
 * the official unified search keeps its LIKE + CLIP pipeline untouched, while
 * the modes here pair BM25 (precise) with the caption embeddings (semantic).
 */

/// The modes the AI search panel offers.
export const AI_SEARCH_MODES = ['precise', 'semantic', 'all'];

/// A stored mode value normalized to one of `AI_SEARCH_MODES`.
/// Unknown values (and unset settings) fall back to `all`.
export function normalizeAiSearchMode(value) {
  const text = String(value ?? '').trim().toLowerCase();
  return AI_SEARCH_MODES.includes(text) ? text : 'all';
}

/// Which search legs a mode runs.
///
/// - `precise`: BM25 over names, metadata, tags, people and captions.
/// - `semantic`: the caption embeddings of the annotated photos.
/// - `all`: both legs, merged per `dedupeGroups`.
export function planAiSearch(mode) {
  const normalized = normalizeAiSearchMode(mode);
  return {
    fts: normalized !== 'semantic',
    captions: normalized !== 'precise',
  };
}

/// The browser file-type mask over `file_type` values (1 image, 2 video,
/// 3 raw). `0` and the full mask keep everything.
export function matchesFileType(file, mask) {
  const type = Number(file?.file_type);
  const wanted = Number(mask) | 0;
  if (!wanted || wanted >= 7) return true;
  if (!Number.isFinite(type)) return false;
  if (wanted & 1 && type === 1) return true;
  if (wanted & 2 && type === 2) return true;
  if (wanted & 4 && type === 3) return true;
  return false;
}

/// The client-side filters every AI-search result passes, mirroring the
/// unified search: album visibility, favorites-only and the file type.
export function passesAiSearchFilters(file, { favoritesOnly = false, fileType = 0 } = {}) {
  if (!file) return false;
  if (file.album_visible === false) return false;
  if (favoritesOnly && !file.is_favorite) return false;
  if (!matchesFileType(file, fileType)) return false;
  return true;
}

/// Restore the ranking order of a hit list after the id lookup.
///
/// The lookup goes through a SQL `IN`, whose return order is undefined; the
/// hits carry the order the user should see. Unknown ids (a file deleted
/// between the search and the lookup) drop out silently.
export function orderFilesByHits(hitIds, files) {
  const order = new Map();
  (Array.isArray(hitIds) ? hitIds : []).forEach((id, index) => {
    order.set(String(id), index);
  });
  return (Array.isArray(files) ? files : [])
    .filter(file => order.has(String(file?.id)))
    .sort((a, b) => (order.get(String(a.id)) ?? Infinity) - (order.get(String(b.id)) ?? Infinity));
}

/// Merge the two legs of the `all` mode: a file found by both legs shows once,
/// in the precise group — exact evidence outranks a similarity score.
export function dedupeGroups({ ftsFiles = [], semanticFiles = [] } = {}) {
  const known = new Set((Array.isArray(ftsFiles) ? ftsFiles : []).map(file => String(file?.id)));
  const semanticSeen = new Set();
  const keptSemantic = (Array.isArray(semanticFiles) ? semanticFiles : []).filter((file) => {
    const key = String(file?.id);
    if (known.has(key) || semanticSeen.has(key)) return false;
    semanticSeen.add(key);
    return true;
  });
  return { ftsFiles: Array.isArray(ftsFiles) ? ftsFiles : [], semanticFiles: keptSemantic };
}

/// The score span of a semantic group, for the group label.
export function semanticScoreRange(hits) {
  if (!Array.isArray(hits) || hits.length === 0) return null;
  let min = Infinity;
  let max = -Infinity;
  for (const hit of hits) {
    const score = Number(hit?.score);
    if (!Number.isFinite(score)) continue;
    if (score < min) min = score;
    if (score > max) max = score;
  }
  if (min === Infinity) return null;
  return { min, max };
}

/// Why the semantic leg is empty, or null when no hint is needed.
///
/// `no-annotations`: nothing was ever annotated, so there is nothing to
/// match against. `no-semantic-match`: photos are annotated but nothing
/// cleared the similarity threshold.
export function aiSearchEmptyHint({ mode, captions = 0, semanticHits = 0, ftsHits = 0 } = {}) {
  const normalized = normalizeAiSearchMode(mode);
  if (normalized === 'precise') return null;
  if (ftsHits > 0 || semanticHits > 0) return null;
  return Number(captions) > 0 ? 'no-semantic-match' : 'no-annotations';
}
