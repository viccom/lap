/**
 * Search mode helpers.
 *
 * Pure logic only — no Vue, no i18n, no Tauri. The sidebar (mode picker) and
 * the content view (query planner) share the same plan, so the three modes
 * behave identically everywhere and can be tested without a browser.
 */

/** The available search modes. */
export const SEARCH_MODES = ['all', 'precise', 'semantic'];

/** The mode a fresh installation uses: every query path runs. */
export const DEFAULT_SEARCH_MODE = 'all';

/** The number of hits the precise (FTS) path asks the backend for. */
export const SEARCH_RESULT_LIMIT = 200;

/**
 * Normalize a stored or user supplied mode.
 *
 * Unknown values fall back to the default mode so a broken setting never
 * breaks the search.
 */
export function normalizeSearchMode(value) {
  return SEARCH_MODES.includes(value) ? value : DEFAULT_SEARCH_MODE;
}

/**
 * The query paths a search runs.
 *
 * - all:     the word based query and the vector query (the historical behavior)
 * - precise: only the full text index (FTS)
 * - semantic: only the vector query
 */
export function planSearch(mode, options = {}) {
  const normalized = normalizeSearchMode(mode);
  const plan = {
    lexical: false,
    vector: false,
    fts: false,
    limit: normalizeLimit(options?.limit ?? SEARCH_RESULT_LIMIT),
  };

  if (normalized === 'precise') {
    plan.fts = true;
  } else if (normalized === 'semantic') {
    plan.vector = true;
  } else {
    plan.lexical = true;
    plan.vector = true;
  }

  return plan;
}

/** The matched index columns and the labels they use. */
export const MATCH_COLUMN_KEYS = {
  name: 'search.match_column.name',
  people: 'search.match_column.people',
  meta: 'search.match_column.meta',
  tags: 'search.match_column.tags',
  caption: 'search.match_column.caption',
  like: 'search.match_column.like',
};

/** The labels used when the caller passes no translated labels. */
export const DEFAULT_MATCH_LABELS = {
  name: 'Name',
  people: 'People',
  meta: 'Metadata',
  tags: 'Tags',
  caption: 'Caption',
  like: 'Fuzzy',
};

/**
 * Render the matched columns as a short readable label.
 *
 * `['people', 'tags']` becomes `People / Tags` with the default labels, or
 * `人名 / 标签` when the caller passes translated labels.
 */
export function describeMatches(matchedColumns, labels = {}) {
  if (!Array.isArray(matchedColumns)) return '';

  const seen = new Set();
  const parts = [];
  for (const column of matchedColumns) {
    const key = String(column ?? '').trim();
    if (!key || seen.has(key)) continue;
    seen.add(key);
    const label = labels?.[key] ?? DEFAULT_MATCH_LABELS[key] ?? key;
    parts.push(String(label));
  }
  return parts.join(' / ');
}

/**
 * True when the query is a single CJK character.
 *
 * The backend cannot match a single CJK character with the bigrams of the
 * index and falls back to a plain text search, which is slower.
 */
export function isSingleCharQuery(text) {
  const value = String(text ?? '').trim();
  if (!value) return false;
  const chars = Array.from(value);
  if (chars.length !== 1) return false;
  return isCjkChar(chars[0]);
}

/** True when the character belongs to a CJK block. */
export function isCjkChar(ch) {
  const code = typeof ch === 'string' ? (ch.codePointAt(0) ?? 0) : Number(ch);
  return (
    (code >= 0x1100 && code <= 0x11ff) || // Hangul jamo
    (code >= 0x3040 && code <= 0x309f) || // Hiragana
    (code >= 0x30a0 && code <= 0x30ff) || // Katakana
    (code >= 0x3130 && code <= 0x318f) || // Hangul compatibility jamo
    (code >= 0x31f0 && code <= 0x31ff) || // Katakana phonetic extensions
    (code >= 0x3400 && code <= 0x4dbf) || // CJK unified ideographs extension A
    (code >= 0x4e00 && code <= 0x9fff) || // CJK unified ideographs
    (code >= 0xa960 && code <= 0xa97f) || // Hangul jamo extended-A
    (code >= 0xac00 && code <= 0xd7a3) || // Hangul syllables
    (code >= 0xd7b0 && code <= 0xd7ff) || // Hangul jamo extended-B
    (code >= 0xf900 && code <= 0xfaff) || // CJK compatibility ideographs
    (code >= 0xff66 && code <= 0xff9f)    // Halfwidth katakana
  );
}

/** Keep a usable result limit. */
function normalizeLimit(limit) {
  const value = Number(limit);
  if (!Number.isFinite(value) || value <= 0) return SEARCH_RESULT_LIMIT;
  return Math.floor(value);
}
