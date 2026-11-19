/**
 * The library state that is written back to the backend.
 *
 * The aggregate counts are runtime-only: they describe the moment they were
 * computed, not the library. Persisting them would show a stale number for a
 * panel that was never opened in the next session — the subject counts were
 * wrong for every subject until the count was taken from the subject query
 * itself (see Content.vue, getSubjectCountAndSum).
 *
 * Plain JavaScript on purpose: the tests import it without a bundler.
 */

/**
 * The library state without the runtime-only aggregates.
 * @param {Record<string, any>} [library] the in-memory library state
 * @returns {Record<string, any>} the state that may be persisted
 */
export function toPersistedLibraryState(library = {}) {
  const {
    counts: _counts,
    activateTick: _activateTick,
    subjectCounts: _subjectCounts,
    ...rest
  } = library || {};
  return rest;
}

/**
 * The library state as it is loaded from the backend: the runtime-only
 * aggregates are dropped, a persisted number must never reach the sidebar.
 * @param {Record<string, any>} [library] the restored library state
 * @returns {Record<string, any>} the state with empty runtime aggregates
 */
export function toRuntimeLibraryState(library = {}) {
  return { ...(library || {}), subjectCounts: {} };
}
