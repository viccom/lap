/**
 * Viewer session store (phase 4, batch 4.9).
 *
 * The desktop shell opens the viewer/editor/montage as separate windows, so
 * `Content.vue` stays mounted and the two sides talk over cross-window events.
 * The Web client makes those pages same-tab routes, which unmounts Home/Content
 * — so the window-event channel has no receiver and the grid can no longer
 * resolve "which file is at index N" for the viewer.
 *
 * This store replaces that channel in the Web client only: the ordered id list
 * the viewer navigates, the viewer's current index (so the grid can follow),
 * and a one-shot "a save happened, refresh the grid" signal. It is deliberately
 * NOT persisted — it is per-tab ephemeral state, and a full id list must never
 * be written to disk.
 *
 * The desktop path is untouched: it keeps using the cross-window events.
 */
import { defineStore } from 'pinia';

export const useViewerSessionStore = defineStore('viewerSession', {
  state: () => ({
    // Ordered file ids the viewer walks, snapshotted from the grid's current
    // query when the viewer opens (so index -> id needs no live Content).
    orderedIds: [],
    // The viewer's active index; the grid reads it on remount to follow along.
    selectionIndex: -1,
    // One-shot "a save finished elsewhere" signal the grid consumes on mount.
    // tick is bumped on every request so a watcher can tell repeats apart.
    // JSDoc cast: a bare null default narrows to never in TS consumers after
    // the null-guard (Content.vue consumes this on mount).
    pendingRefresh: /** @type {{ kind: 'editor' | 'montage', payload?: Record<string, any>, tick?: number } | null} */ (null),
    refreshTick: 0,
    // Save-as context the editor/montage needs but Content owns. Content
    // unmounts while those routes are open, so it snapshots the bits here.
    editorSaveAsContexts: new Map(),   // fileId -> SaveAsContext
    montageSourceFolder: '',
    // Content hands its SaveAsContext here before unmounting; bare null would
    // narrow to null and reject the real context assignment.
    montageSaveAsContext: /** @type {any} */ (null),
  }),
  actions: {
    /// Snapshot the ordered ids + index a viewer session starts from.
    openSession(fileIds, index) {
      this.orderedIds = Array.isArray(fileIds)
        ? fileIds.map((id) => Number(id)).filter((id) => Number.isFinite(id) && id > 0)
        : [];
      this.selectionIndex = Number.isInteger(index) ? index : -1;
    },
    /// Resolve an index in the current session to a file id (0 when unknown).
    fileIdAt(index) {
      if (!Number.isInteger(index) || index < 0 || index >= this.orderedIds.length) return 0;
      return this.orderedIds[index];
    },
    /// The viewer moved; remember the index so the grid follows on remount.
    setSelectionIndex(index) {
      this.selectionIndex = Number.isInteger(index) ? index : -1;
    },
    /// The editor/montage saved something; the grid should refresh on mount.
    requestRefresh(kind, payload) {
      this.refreshTick += 1;
      this.pendingRefresh = { kind, payload: payload || {}, tick: this.refreshTick };
    },
    /// Read-and-clear the pending refresh (so it fires exactly once).
    consumeRefresh() {
      const pending = this.pendingRefresh;
      this.pendingRefresh = null;
      return pending;
    },
  },
});
