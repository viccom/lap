/**
 * The image-understanding lab page - pure helpers and thin invoke wrappers.
 *
 * The lab is an isolated playground for the image-understanding (F1) and the
 * exact search (F2) features: nothing here is wired into the browsing pages.
 * The logic is side effect free so it can be tested without a Tauri runtime;
 * the invoke wrappers follow the ./vlApi.js style with an injectable invoker.
 */
import { invoke } from '@tauri-apps/api/core';
import { MATCH_COLUMN_KEYS, SEARCH_RESULT_LIMIT } from './searchMode.js';
import { buildRunArgs } from './vlLogic.js';

/// The picker loads at most this many files of the library.
export const LAB_MAX_FILES = 200;

/// The rerank request carries at most this many candidates (backend cap).
export const RERANK_MAX_CANDIDATES = 20;

/// The poll runs every 3 s; the backend gives up on a gateway call after
/// ~180 s (VL_REQUEST_TIMEOUT_SECS), the rest is margin. Same value as the
/// file info panel.
export const LAB_POLL_LIMIT_MS = 200_000;
export const LAB_POLL_INTERVAL_MS = 3000;

/// The query of the picker: the whole library, no filters, default order.
export function buildLabQueryParams() {
  return {
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
  };
}

/**
 * The ids of the files that already carry an annotation for the template.
 *
 * `annotationsByFileId` maps a file id to its annotations (newest first); the
 * run without `force` skips these files.
 */
export function annotatedIdSet(annotationsByFileId, templateId) {
  const ids = new Set();
  const wanted = Number(templateId);
  for (const [fileId, annotations] of Object.entries(annotationsByFileId ?? {})) {
    const hit = (Array.isArray(annotations) ? annotations : []).some(
      annotation => Number(annotation?.templateId) === wanted,
    );
    if (hit) ids.add(Number(fileId));
  }
  return ids;
}

/// The rows of the picker: one per file with its selection and idempotence mark.
export function pickerRows(files, selectedIds, annotatedIds) {
  const selected = new Set((Array.isArray(selectedIds) ? selectedIds : []).map(Number));
  const annotated = annotatedIds instanceof Set ? annotatedIds : new Set();
  return (Array.isArray(files) ? files : []).map(file => {
    const id = Number(file?.id);
    return {
      id,
      file,
      selected: selected.has(id),
      hasExisting: annotated.has(id),
    };
  });
}

/// A run over more than one file asks for a confirmation first: every selected
/// photo sends one request to the cloud gateway.
export function needsBatchConfirm(selectedCount) {
  return Number(selectedCount) > 1;
}

/// The arguments of `run_vl_analysis`; delegates to the shared builder so the
/// lab and the file info panel send the very same shape.
export function buildLabRunArgs(fileIds, templateId, force = false) {
  return buildRunArgs(fileIds, templateId, force);
}

/**
 * The outcome of one file of a finished run.
 *
 * - success: a new annotation arrived (its id is above the baseline), or a
 *   forced rerun refreshed the existing row in place (the backend upserts, so
 *   the id stays but the callId moves past the baseline call)
 * - skipped: an annotation existed and force was off, so the file kept it
 * - failed: a new call arrived but produced no annotation
 * - notRun: no new evidence at all — the run was cancelled or never arrived
 */
export function deriveFileOutcome({
  force = false,
  baselineAnnotationId = 0,
  baselineCallId = 0,
  annotations = [],
  calls = [],
} = {}) {
  const annotationList = Array.isArray(annotations) ? annotations : [];
  const callList = Array.isArray(calls) ? calls : [];
  const newAnnotation = annotationList.find(a => Number(a?.id) > Number(baselineAnnotationId)) ?? null;
  if (newAnnotation) {
    return { status: 'success', annotation: newAnnotation, call: null };
  }
  const refreshed = annotationList.find(
    a => Number(a?.id) <= Number(baselineAnnotationId) && Number(a?.callId) > Number(baselineCallId),
  ) ?? null;
  if (refreshed) {
    return { status: 'success', annotation: refreshed, call: null };
  }
  const newCall = callList.find(c => Number(c?.id) > Number(baselineCallId)) ?? null;
  if (!newCall && !force && annotationList.length) {
    return { status: 'skipped', annotation: annotationList[0], call: null };
  }
  if (!newCall) {
    return { status: 'notRun', annotation: null, call: null };
  }
  return { status: 'failed', annotation: null, call: newCall };
}

/// The i18n key of every outcome status.
export const OUTCOME_LABEL_KEYS = {
  success: 'vl.lab.outcome_success',
  skipped: 'vl.lab.outcome_skipped',
  failed: 'vl.lab.outcome_failed',
  notRun: 'vl.lab.outcome_not_run',
};

/// True when the index row count matches the library row count. A missing
/// status shows no warning instead of a false alarm.
export function indexIsConsistent(status) {
  if (!status || typeof status !== 'object') return true;
  return Number(status.documents) === Number(status.files);
}

/// The matched columns of one hit as badges with their i18n keys.
export function hitBadges(hit) {
  const columns = Array.isArray(hit?.matchedColumns) ? hit.matchedColumns : [];
  return columns
    .map(column => String(column ?? '').trim())
    .filter(Boolean)
    .map(column => ({ column, labelKey: MATCH_COLUMN_KEYS[column] ?? null }));
}

/// The matched columns of all hits, without duplicates, in first-seen order.
export function collectMatchedColumns(hits) {
  const seen = new Set();
  const columns = [];
  for (const hit of Array.isArray(hits) ? hits : []) {
    for (const column of Array.isArray(hit?.matchedColumns) ? hit.matchedColumns : []) {
      const key = String(column ?? '').trim();
      if (!key || seen.has(key)) continue;
      seen.add(key);
      columns.push(key);
    }
  }
  return columns;
}

// ----- thin invoke wrappers -----

/// The row counts of the index and of the library.
export async function getFtsStatus(invoker = invoke) {
  return invoker('fts_status');
}

/// Rebuild the whole index; resolves with the number of indexed files.
export async function ftsReindex(invoker = invoke) {
  return invoker('fts_reindex');
}

/// The hits of an exact search.
export async function searchFts(query, limit = SEARCH_RESULT_LIMIT, invoker = invoke) {
  return invoker('search_fts', { query: String(query ?? ''), limit: Number(limit) });
}

/// Keep only the hits the cloud model judged relevant, in its order.
///
/// Ids the model invented are ignored; ids it omitted stay out because the
/// model is allowed to reject photos (an empty selection means "no match").
/// The hit objects (and their matched columns) are reused.
export function applyRerankSelection(hits, order) {
  const list = Array.isArray(hits) ? hits : [];
  const byId = new Map(list.map(hit => [Number(hit?.fileId), hit]));
  const selected = [];
  const seen = new Set();
  for (const raw of Array.isArray(order) ? order : []) {
    const id = Number(raw);
    const hit = byId.get(id);
    if (!hit || seen.has(id)) continue;
    selected.push(hit);
    seen.add(id);
  }
  return selected;
}

/// Let the cloud model keep only the candidates that match the query.
export async function rerankSearchWithLlm(query, fileIds, invoker = invoke) {
  return invoker('rerank_search_with_llm', {
    query: String(query ?? ''),
    fileIds: (Array.isArray(fileIds) ? fileIds : []).map(Number),
  });
}

/// Rerank a hit list; returns the filtered hits and whether the call ran.
export async function rerankHitList(query, hits, invoker = invoke) {
  const list = Array.isArray(hits) ? hits : [];
  if (!list.length) return { hits: [], reranked: false };
  const order = await rerankSearchWithLlm(
    query,
    list.map(hit => Number(hit.fileId)),
    invoker,
  );
  return { hits: applyRerankSelection(list, order), reranked: true };
}
