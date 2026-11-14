<template>
  <div class="w-screen h-screen flex flex-col bg-base-300 text-base-content/70 overflow-hidden">

    <!-- Title Bar -->
    <TitleBar v-if="showDesktopTitleBar" :titlebar="$t('vl.lab.title')" :resizable="false" viewName="VlLab" class="shrink-0 z-50" />
    <div v-else class="h-10 shrink-0 flex items-center justify-center px-20 select-none" data-tauri-drag-region>
      <div class="text-sm font-medium text-base-content/70 truncate" data-tauri-drag-region>{{ $t('vl.lab.title') }}</div>
    </div>

    <div class="flex-1 overflow-y-auto min-h-0 p-3 space-y-3">

      <!-- ============ status bar ============ -->
      <section class="rounded-box p-2 bg-base-200/30 border border-base-content/5 shadow-sm space-y-2">
        <div class="flex flex-wrap items-center gap-2">
          <span class="font-bold uppercase text-[10px] tracking-widest text-base-content/40">{{ $t('vl.lab.title') }}</span>
          <span
            class="px-1.5 h-5 inline-flex items-center rounded-box text-[10px] font-semibold tracking-[0.08em] border"
            :class="configured
              ? 'border-success/30 bg-success/10 text-success'
              : 'border-warning/30 bg-warning/10 text-warning'"
          >{{ configured ? $t('vl.lab.gateway_ok') : $t('vl.lab.gateway_missing') }}</span>

          <span class="ml-auto text-xs text-base-content/60">{{ indexStatusText }}</span>
          <span v-if="embedStatusText" class="text-xs text-base-content/40">{{ embedStatusText }}</span>
          <button
            v-if="!indexConsistent"
            class="btn btn-xs btn-warning"
            :disabled="rebuilding"
            @click="rebuildIndex"
          >{{ rebuilding ? $t('vl.lab.rebuilding') : $t('vl.lab.rebuild_index') }}</button>
        </div>
        <div v-if="!indexConsistent" class="text-xs text-warning/90">{{ $t('vl.lab.index_mismatch', ftsStatus ?? {}) }}</div>
        <div v-if="!configured" class="flex items-center gap-2">
          <span class="text-xs text-base-content/50">{{ $t('vl.lab.gateway_missing_hint') }}</span>
          <button class="btn btn-xs" @click="openSettings">{{ $t('vl.lab.open_settings') }}</button>
        </div>
        <div v-if="errorMessage" class="rounded-box px-2 py-2 text-xs text-error bg-error/10 border border-error/20 whitespace-pre-wrap break-words">{{ errorMessage }}</div>
      </section>

      <!-- ============ F1: image understanding ============ -->
      <section class="rounded-box p-2 bg-base-200/30 border border-base-content/5 shadow-sm space-y-2">
        <div class="flex items-center gap-2">
          <span class="font-bold uppercase text-[10px] tracking-widest text-base-content/40">{{ $t('vl.lab.f1_title') }}</span>
          <div class="ml-auto flex items-center gap-2">
            <button class="btn btn-xs" :disabled="loadingFiles" @click="loadFiles">
              {{ loadingFiles ? $t('vl.lab.loading_files') : $t('vl.lab.load_files') }}
            </button>
          </div>
        </div>

        <!-- photo picker -->
        <div v-if="files.length" class="text-[10px] text-base-content/40">{{ $t('vl.lab.files_loaded_hint', { count: files.length }) }}</div>
        <div v-if="files.length" class="max-h-64 overflow-y-auto rounded-box bg-base-100/20 border border-base-content/5 p-1">
          <div class="grid grid-cols-[repeat(auto-fill,minmax(72px,1fr))] gap-1">
            <div
              v-for="row in pickerRowList"
              :key="row.id"
              class="relative aspect-square rounded-box overflow-hidden cursor-pointer border border-base-content/5"
              :class="row.selected ? 'outline-2 -outline-offset-2 outline-primary' : 'hover:outline-1 hover:outline-base-content/20'"
              :data-lazy-id="row.id"
              :title="row.file?.name"
              @click="toggleSelect(row.id)"
            >
              <img v-if="thumbs.get(row.id)" :src="thumbs.get(row.id)" class="w-full h-full object-cover pointer-events-none" alt="">
              <div v-else class="w-full h-full bg-base-content/5" />
              <span
                v-if="row.hasExisting"
                class="absolute bottom-0.5 left-0.5 px-1 rounded-box bg-primary/85 text-primary-content text-[9px] leading-4"
                :title="$t('vl.lab.existing_annotation')"
              >✓</span>
            </div>
          </div>
        </div>
        <div v-else-if="!loadingFiles" class="text-xs text-base-content/40 px-1">{{ $t('vl.lab.no_files') }}</div>

        <!-- run controls -->
        <div class="flex flex-wrap items-center gap-2">
          <select v-model.number="templateId" class="select select-bordered select-xs w-56" :aria-label="$t('vl.lab.template')">
            <option v-if="!latestTemplates.length" :value="0">{{ $t('vl.lab.no_template') }}</option>
            <option v-for="template in latestTemplates" :key="template.id" :value="template.id">{{ templateLabel(template) }}</option>
          </select>
          <label class="flex items-center gap-1 text-xs text-base-content/60 cursor-pointer">
            <input v-model="forceRerun" type="checkbox" class="checkbox checkbox-xs" />
            {{ $t('vl.lab.force_rerun') }}
          </label>
          <button
            class="btn btn-xs btn-primary"
            :disabled="!canRun"
            @click="clickUnderstand"
          >{{ $t('vl.lab.understand') }}</button>
          <span class="text-xs text-base-content/40">{{ selectionSummary }}</span>
        </div>

        <!-- batch confirmation -->
        <div v-if="pendingConfirm" class="rounded-box px-2 py-2 space-y-1 text-xs bg-warning/10 border border-warning/20">
          <div class="font-semibold text-warning">{{ $t('vl.lab.batch_confirm_title', { count: selectedFileIds.length }) }}</div>
          <div class="text-base-content/60">{{ $t('vl.lab.batch_confirm_text') }}</div>
          <div class="flex gap-2">
            <button class="btn btn-xs btn-warning" @click="confirmBatch">{{ $t('vl.lab.batch_confirm_ok') }}</button>
            <button class="btn btn-xs btn-ghost" @click="pendingConfirm = false">{{ $t('vl.lab.cancel') }}</button>
          </div>
        </div>

        <!-- running state -->
        <div v-if="running" class="rounded-box px-2 py-2 text-xs bg-base-100/20 border border-base-content/5" role="status" aria-live="polite">
          <div class="flex items-center gap-2 text-primary">
            <span class="loading loading-spinner loading-xs" />
            <span>{{ $t('vl.lab.running', { processed: progress.processed, total: progress.total }) }}</span>
            <button class="ml-auto text-base-content/50 underline hover:text-base-content" @click="cancelRun">{{ $t('vl.lab.cancel_run') }}</button>
          </div>
        </div>
        <div v-if="stillRunningHint" class="rounded-box px-2 py-2 text-xs bg-base-100/20 border border-base-content/5">
          {{ $t('vl.lab.still_running') }}
        </div>

        <!-- results -->
        <div v-if="resultRows.length" class="space-y-2">
          <div class="flex items-center gap-2">
            <span class="font-bold uppercase text-[10px] tracking-widest text-base-content/40">{{ $t('vl.lab.results_title') }}</span>
            <span class="text-xs text-base-content/50">{{ $t('vl.lab.run_summary', runSummaryCounts) }}</span>
          </div>
          <div
            v-for="row in resultRows"
            :key="row.id"
            class="rounded-box px-2 py-2 space-y-1.5 bg-base-100/20 border border-base-content/5"
          >
            <div class="flex items-center gap-2">
              <img v-if="thumbs.get(row.id)" :src="thumbs.get(row.id)" class="w-8 h-8 rounded-box object-cover shrink-0" alt="">
              <span class="text-xs text-base-content/80 truncate">{{ row.file?.name ?? row.id }}</span>
              <span class="ml-auto shrink-0 badge badge-sm" :class="outcomeBadgeClass(row.outcome.status)">
                {{ $t(OUTCOME_LABEL_KEYS[row.outcome.status]) }}
              </span>
            </div>

            <!-- annotation entries -->
            <template v-if="row.entries.length">
              <div v-for="entry in row.entries" :key="entry.key" class="space-y-0.5">
                <div class="text-[10px] uppercase tracking-wide text-base-content/40">
                  {{ entry.labelKey ? $t(entry.labelKey) : entry.key }}
                </div>
                <div v-if="entry.kind === 'tags'" class="flex flex-wrap gap-1">
                  <span
                    v-for="(item, index) in entry.items"
                    :key="index"
                    class="badge badge-sm badge-outline border-base-content/10 bg-base-content/5 font-medium text-base-content/70"
                  >{{ item }}</span>
                </div>
                <div v-else-if="entry.kind === 'list'" class="text-xs text-base-content/80 whitespace-pre-wrap break-words">{{ entry.items.join('\n') }}</div>
                <div v-else-if="entry.kind === 'object'" class="space-y-0.5">
                  <div v-for="field in entry.fields" :key="field.key" class="text-xs text-base-content/80 break-words">
                    <span class="text-base-content/40">{{ field.key }}:</span> {{ field.text }}
                  </div>
                </div>
                <div v-else class="text-xs text-base-content/80 whitespace-pre-wrap break-words">{{ entry.text }}</div>
              </div>
            </template>
            <div v-if="row.parseError" class="text-[10px] text-error break-words">{{ $t('vl.result_unreadable') }}: {{ row.parseError }}</div>
            <div v-if="row.outcome.status === 'failed'" class="text-[11px] text-error break-words whitespace-pre-wrap">
              {{ row.outcome.call?.error || row.outcome.call?.status || row.outcome.call?.rawResponse || '—' }}
            </div>

            <!-- raw call evidence -->
            <div v-if="latestCallOf(row)" class="rounded-box border border-base-content/5 bg-base-300/40">
              <button
                type="button"
                class="w-full flex items-center gap-1 px-2 py-1 text-left text-base-content/70 hover:text-base-content"
                :aria-expanded="expandedRaw.has(row.id)"
                @click="toggleRaw(row.id)"
              >
                <IconRight class="w-3 h-3 transition-transform duration-200" :class="{ 'rotate-90': expandedRaw.has(row.id) }" />
                <span class="font-bold uppercase text-[10px] tracking-widest text-base-content/30">{{ $t('vl.raw_response') }}</span>
              </button>
              <div v-if="expandedRaw.has(row.id)" class="px-2 pb-2 space-y-1.5 text-[11px]">
                <div class="grid grid-cols-[88px_minmax(0,1fr)] gap-x-2 gap-y-0.5">
                  <span class="text-base-content/40">{{ $t('vl.status') }}</span>
                  <span class="text-base-content/80 break-all">{{ latestCallOf(row)?.status || '-' }}</span>
                  <span class="text-base-content/40">{{ $t('vl.model') }}</span>
                  <span class="text-base-content/80 break-all">{{ latestCallOf(row)?.model || '-' }}</span>
                  <span class="text-base-content/40">{{ $t('vl.request_id') }}</span>
                  <span class="text-base-content/80 break-all">{{ latestCallOf(row)?.requestId || '-' }}</span>
                  <span class="text-base-content/40">{{ $t('vl.duration') }}</span>
                  <span class="text-base-content/80">{{ formatDuration(latestCallOf(row)?.durationMs) || '-' }}</span>
                  <template v-if="usageEntriesOf(row).length">
                    <span class="text-base-content/40">{{ $t('vl.usage') }}</span>
                    <span class="text-base-content/80">
                      <span v-for="(entry, index) in usageEntriesOf(row)" :key="entry.key">
                        <span v-if="index" class="text-base-content/30"> / </span>{{ $t(entry.labelKey) }}: {{ entry.value }}
                      </span>
                    </span>
                  </template>
                </div>
                <pre class="max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-box bg-base-300/40 p-1.5 text-[10px] leading-4 text-base-content/70">{{ latestCallOf(row)?.rawResponse || '-' }}</pre>
              </div>
            </div>
          </div>
        </div>
      </section>

      <!-- ============ F2: exact search ============ -->
      <section class="rounded-box p-2 bg-base-200/30 border border-base-content/5 shadow-sm space-y-2">
        <div class="font-bold uppercase text-[10px] tracking-widest text-base-content/40">{{ $t('vl.lab.f2_title') }}</div>
        <div class="flex flex-wrap items-center gap-2">
          <input
            v-model="searchQuery"
            type="text"
            class="input input-bordered input-sm flex-1 min-w-48"
            :placeholder="$t('vl.lab.search_placeholder')"
            spellcheck="false"
            @keydown.enter="runSearch()"
          >
          <button class="btn btn-sm btn-primary" :disabled="!searchQuery.trim() || searching" @click="runSearch()">
            {{ searching && searchKind === 'precise' ? $t('vl.lab.searching') : $t('vl.lab.search_button') }}
          </button>
          <button
            class="btn btn-sm btn-outline"
            :disabled="!searchQuery.trim() || searching"
            @click="runSearch('semantic')"
          >
            {{ searching && searchKind === 'semantic' ? $t('vl.lab.searching') : $t('vl.lab.semantic_button') }}
          </button>
        </div>
        <div v-if="isSingleCharQuery(searchQuery)" class="text-xs text-warning/80">{{ $t('search.mode.single_char_hint') }}</div>
        <div class="text-[11px] text-base-content/40">{{ $t('vl.lab.precise_hint') }}</div>
        <div class="text-[11px] text-base-content/40">{{ $t('vl.lab.semantic_hint') }}</div>
        <label class="flex items-center gap-1 text-xs text-base-content/60 cursor-pointer">
          <input v-model="llmRerank" type="checkbox" class="checkbox checkbox-xs" :disabled="searching" />
          {{ $t('vl.lab.llm_rerank') }}
        </label>
        <div class="text-[11px] text-base-content/40">{{ $t('vl.lab.llm_rerank_hint') }}</div>
        <div v-if="llmRerank && vlRerankModel" class="text-[11px] text-base-content/40">
          {{ $t('vl.lab.llm_rerank_model', { model: vlRerankModel }) }}
        </div>
        <div v-if="searchError" class="text-xs text-error break-words">{{ searchError }}</div>

        <template v-if="searched">
          <div class="text-xs text-base-content/60">
            <span v-if="searchKind === 'semantic'" class="text-base-content/40">{{ $t('search.mode.semantic') }} · </span>
            <span v-if="reranked" class="text-base-content/40">{{ $t('vl.lab.llm_rerank') }} · </span>
            {{ $t('vl.lab.hits_summary', { count: searchHits.length }) }}
            <span v-if="matchColumnSummary" class="text-base-content/40">· {{ matchColumnSummary }}</span>
          </div>
          <div v-if="reranked && unannotatedCount" class="text-xs text-base-content/40">
            {{ $t('vl.lab.llm_rerank_unannotated', { count: unannotatedCount }) }}
          </div>
          <div v-if="reranked && !searchHits.length" class="text-xs text-warning/80">
            {{ $t('vl.lab.llm_rerank_no_match') }}
          </div>
          <div v-if="searchHits.length" class="max-h-64 overflow-y-auto rounded-box bg-base-100/20 border border-base-content/5 p-1">
            <div class="grid grid-cols-[repeat(auto-fill,minmax(88px,1fr))] gap-1">
              <div
                v-for="item in searchResultItems"
                :key="item.hit.fileId"
                class="relative rounded-box overflow-hidden border border-base-content/5"
                :data-lazy-id="item.hit.fileId"
              >
                <img v-if="thumbs.get(item.hit.fileId)" :src="thumbs.get(item.hit.fileId)" class="w-full aspect-square object-cover" alt="">
                <div v-else class="w-full aspect-square bg-base-content/5" />
                <div class="px-1 pt-0.5 pb-1">
                  <div class="text-[10px] text-base-content/70 truncate" :title="item.file?.name ?? String(item.hit.fileId)">
                    {{ item.file?.name ?? item.hit.fileId }}
                  </div>
                  <div class="flex flex-wrap gap-0.5">
                    <span
                      v-for="badge in item.badges"
                      :key="badge.column"
                      class="px-1 rounded-box bg-base-content/10 text-[9px] leading-4 text-base-content/60 truncate"
                    >{{ badge.labelText }}</span>
                  </div>
                </div>
              </div>
            </div>
          </div>
          <div v-else class="text-xs text-base-content/40 px-1">{{ $t('vl.lab.no_hits') }}</div>
        </template>
      </section>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { emit, listen } from '@tauri-apps/api/event';
import { useI18n } from 'vue-i18n';
import TitleBar from '@/components/TitleBar.vue';
import { IconRight } from '@/common/icons';
import { config } from '@/common/config';
import { useToast } from '@/common/toast';
import { getThumbnailDataUrl, isWin, isLinux } from '@/common/utils';
import { invoke } from '@tauri-apps/api/core';
import { getFileThumb, getFilesByIds, getQueryFiles } from '@/common/api';
import { getVlConfig, getVlAnnotations, getVlCalls, listVlTemplates, runVlAnalysis, cancelVlRun } from '@/common/vlApi';
import {
  VL_SETTINGS_TAB,
  buildResultEntries,
  findTemplateVersion,
  formatDuration,
  formatUsage,
  parseResponseSpec,
  parseResultJson,
  pickLatestCall,
  pickLatestVersions,
  templateLabel,
} from '@/common/vlLogic';
import { MATCH_COLUMN_KEYS, SEARCH_RESULT_LIMIT, describeMatches, isSingleCharQuery } from '@/common/searchMode';
import {
  LAB_MAX_FILES,
  LAB_POLL_INTERVAL_MS,
  LAB_POLL_LIMIT_MS,
  OUTCOME_LABEL_KEYS,
  RERANK_MAX_CANDIDATES,
  annotatedIdSet,
  buildLabQueryParams,
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
  searchFts,
  shouldFinishRun,
} from '@/common/vlLabLogic';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';

const { t } = useI18n();
const toast = useToast();
const showDesktopTitleBar = isWin || isLinux;

// ----- gateway and index status -----

const configured = ref(false);
const vlRerankModel = ref('');
const ftsStatus = ref<{ documents: number; files: number } | null>(null);
const embedStatus = ref<{ source: string; localAvailable: boolean; model: string; embedded: number; captions: number } | null>(null);
const rebuilding = ref(false);
const indexConsistent = computed(() => indexIsConsistent(ftsStatus.value));
const indexStatusText = computed(() => (
  ftsStatus.value ? t('vl.lab.index_status', { documents: ftsStatus.value.documents, files: ftsStatus.value.files }) : ''
));
const embedStatusText = computed(() => {
  if (!embedStatus.value) return '';
  const status = embedStatus.value;
  const source = status.source === 'cloud'
    ? t('vl.lab.embed_source_cloud')
    : t('vl.lab.embed_source_local');
  return t('vl.lab.embed_status', {
    source,
    embedded: status.embedded,
    captions: status.captions,
    model: status.model,
  });
});

async function loadStatus() {
  try {
    const view = await getVlConfig();
    configured.value = Boolean(view?.configured);
    vlRerankModel.value = String(view?.rerankModel ?? '');
  } catch {
    configured.value = false;
    vlRerankModel.value = '';
  }
  try {
    ftsStatus.value = await getFtsStatus();
  } catch {
    ftsStatus.value = null;
  }
  try {
    embedStatus.value = await invoke('embed_status');
  } catch {
    embedStatus.value = null;
  }
}

async function rebuildIndex() {
  if (rebuilding.value) return;
  rebuilding.value = true;
  try {
    await ftsReindex();
    await invoke('rebuild_caption_embeds');
    await loadStatus();
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    rebuilding.value = false;
  }
}

async function openSettings() {
  config.settings.tabIndex = VL_SETTINGS_TAB;
  // The settings window syncs its tab through this event (see main.js);
  // the local Pinia write only moves this window's own instance.
  await emit('settings-settingsTabIndex-changed', VL_SETTINGS_TAB);
  try {
    const settingsWindow = await WebviewWindow.getByLabel('settings');
    if (!settingsWindow) {
      toast.info(t('vl.settings_window_not_open'));
      return;
    }
    await settingsWindow.show();
    await settingsWindow.setFocus();
  } catch (error) {
    console.error('Failed to show the settings window:', error);
    toast.info(t('vl.settings_window_not_open'));
  }
}

// ----- photo picker -----

const files = ref<any[]>([]);
const loadingFiles = ref(false);
// Bumped on every library switch: a load that lands afterwards carries the
// previous library's ids and must be dropped instead of refilling the view.
let libraryEpoch = 0;
const selectedIds = ref<Set<number>>(new Set());
const annotationsByFileId = ref<Record<number, any[]>>({});
const templateId = ref(0);
const forceRerun = ref(false);
const templates = ref<any[]>([]);
const latestTemplates = computed(() => pickLatestVersions(templates.value));
const annotated = computed(() => annotatedIdSet(annotationsByFileId.value, templateId.value));
const pickerRowList = computed(() => pickerRows(files.value, [...selectedIds.value], annotated.value));
const selectedFileIds = computed(() => pickerRowList.value.filter(row => row.selected).map(row => row.id));
const canRun = computed(() => configured.value && Boolean(templateId.value) && selectedFileIds.value.length > 0 && !running.value && !starting.value);
const selectionSummary = computed(() => {
  const total = selectedFileIds.value.length;
  if (!total) return '';
  const existing = pickerRowList.value.filter(row => row.selected && row.hasExisting).length;
  const suffix = existing && !forceRerun.value ? ` · ${existing} ✕ ${t('vl.lab.existing_annotation')}` : '';
  return `${total}${suffix}`;
});

async function loadFiles() {
  if (loadingFiles.value) return;
  loadingFiles.value = true;
  const epoch = libraryEpoch;
  try {
    const list = await getQueryFiles(buildLabQueryParams(), 0, LAB_MAX_FILES);
    // A library switch while the query was in flight invalidates the answer.
    if (epoch !== libraryEpoch) return;
    files.value = Array.isArray(list) ? list.slice(0, LAB_MAX_FILES) : [];
    // The ids that went away cannot be selected any more.
    const present = new Set(files.value.map((file: any) => Number(file.id)));
    selectedIds.value = new Set([...selectedIds.value].filter(id => present.has(id)));
    observeThumbs();
  } finally {
    loadingFiles.value = false;
  }
}

async function toggleSelect(fileId: number) {
  const id = Number(fileId);
  const next = new Set(selectedIds.value);
  if (next.has(id)) {
    next.delete(id);
  } else {
    next.add(id);
  }
  // Write before awaiting: two clicks inside the annotation-fetch round-trip
  // would otherwise each start from the same stale set and the later write
  // would silently drop the earlier selection.
  selectedIds.value = next;
  if (next.has(id) && !annotationsByFileId.value[id]) {
    // The idempotence mark needs the annotations of the file; they load on
    // selection only, not for every file of the picker.
    // Await first, spread later: spreading the cache inside the object literal
    // snapshots it before the await, and a sibling toggle resolving in between
    // would be silently dropped by the stale snapshot.
    const fetched = await getVlAnnotations(id).catch(() => []);
    annotationsByFileId.value = { ...annotationsByFileId.value, [id]: fetched };
  }
}

async function loadTemplates() {
  try {
    templates.value = await listVlTemplates();
  } catch {
    templates.value = [];
  }
  if (!latestTemplates.value.some(template => template.id === templateId.value)) {
    templateId.value = latestTemplates.value.length ? latestTemplates.value[0].id : 0;
  }
}

// ----- thumbnails (lazy via IntersectionObserver) -----

const thumbs = reactive(new Map<number, string>());
const thumbInflight = new Set<number>();
let observer: IntersectionObserver | null = null;
const fileById = computed(() => {
  const map = new Map<number, any>();
  for (const file of files.value) map.set(Number(file?.id), file);
  for (const file of searchFiles.value) map.set(Number(file?.id), file);
  return map;
});

function observeThumbs() {
  void nextTick(() => {
    document.querySelectorAll('[data-lazy-id]').forEach(el => observer?.observe(el));
  });
}

async function loadThumb(fileId: number) {
  if (!fileId || thumbs.has(fileId) || thumbInflight.has(fileId)) return;
  thumbInflight.add(fileId);
  const epoch = libraryEpoch;
  try {
    const file = fileById.value.get(fileId);
    if (!file) return;
    const thumb = await getFileThumb(
      fileId,
      file.file_path,
      file.file_type,
      file.e_orientation || 0,
      256,
      false,
    );
    if (epoch !== libraryEpoch) return;
    thumbs.set(fileId, getThumbnailDataUrl(thumb, '', false, 256));
  } catch {
    // The cell keeps its placeholder; the next intersection tries again.
  } finally {
    thumbInflight.delete(fileId);
  }
}

// ----- F1 run -----

const running = ref(false);
const starting = ref(false);
const runId = ref<string | null>(null);
const progress = ref({ processed: 0, total: 0 });
const stillRunningHint = ref(false);
// Set when the poll gave up but the run may still be going in the backend:
// a matching finished event must still land the results.
const awaitingFinish = ref(false);
const errorMessage = ref('');
const pendingConfirm = ref(false);
const expandedRaw = reactive(new Set<number>());
const resultRows = ref<any[]>([]);
let lastFinishedRunId: string | null = null;
let baselineAnnotationIds = new Map<number, number>();
let baselineCallIds = new Map<number, number>();
let lastRunIds: number[] = [];
let lastRunForce = false;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let pollStartedAt = 0;
let stopFinishedListener: (() => void) | null = null;
let stopProgressListener: (() => void) | null = null;
let stopLibraryListener: (() => void) | null = null;

const runSummaryCounts = computed(() => {
  const counts = { succeeded: 0, failed: 0, skipped: 0 };
  for (const row of resultRows.value) {
    if (row.outcome.status === 'success') counts.succeeded += 1;
    else if (row.outcome.status === 'failed') counts.failed += 1;
    else if (row.outcome.status === 'skipped') counts.skipped += 1;
  }
  return counts;
});

function clickUnderstand() {
  if (!canRun.value) return;
  if (needsBatchConfirm(selectedFileIds.value.length)) {
    pendingConfirm.value = true;
    return;
  }
  void startRun();
}

function confirmBatch() {
  pendingConfirm.value = false;
  void startRun();
}

async function startRun() {
  const ids = selectedFileIds.value.slice();
  const currentTemplate = Number(templateId.value);
  if (!ids.length || !currentTemplate || running.value || starting.value) return;
  // Claim the busy flag before the first await: the baselines cost one IPC
  // per file, and a second click inside that window would pass the running
  // guard again and queue a duplicate run.
  starting.value = true;
  errorMessage.value = '';
  stillRunningHint.value = false;
  awaitingFinish.value = false;
  resultRows.value = [];
  try {
    // The baselines tell which annotations and calls the run produced.
    const callsPerFile = await Promise.all(ids.map(id => getVlCalls(id).catch(() => [])));
    baselineCallIds = new Map(ids.map((id, index) => [id, Number(callsPerFile[index]?.[0]?.id || 0)]));
    baselineAnnotationIds = new Map(ids.map(id => [id, Number((annotationsByFileId.value[id] ?? [])[0]?.id || 0)]));
    const id = await runVlAnalysis(ids, currentTemplate, forceRerun.value);
    lastRunIds = ids;
    lastRunForce = forceRerun.value;
    // Run ids are UUID strings; they survive restarts and must never pass
    // through Number().
    runId.value = String(id);
    progress.value = { processed: 0, total: ids.length };
    running.value = true;
    startPolling();
    // The backend may have finished the run before the answer arrived.
    if (lastFinishedRunId !== null && lastFinishedRunId === String(id)) await finishRun();
  } catch (error) {
    running.value = false;
    errorMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    starting.value = false;
  }
}

async function cancelRun() {
  if (runId.value === null) return;
  try {
    await cancelVlRun(runId.value);
  } catch (error) {
    errorMessage.value = error instanceof Error ? error.message : String(error);
  }
}

function startPolling() {
  stopPolling();
  // The finished event is the primary signal; the poll is the safety net for
  // a missed event, exactly like the file info panel.
  pollStartedAt = Date.now();
  pollTimer = setInterval(() => { void checkRunFinished(); }, LAB_POLL_INTERVAL_MS);
}

function stopPolling() {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/// The poll watches the last file the run will actually process: the backend
/// walks the list in order, so a new call or annotation there means the end.
function pollTargetId(): number | null {
  for (let index = lastRunIds.length - 1; index >= 0; index -= 1) {
    const id = lastRunIds[index];
    if (lastRunForce || !annotated.value.has(id)) return id;
  }
  return null;
}

async function checkRunFinished() {
  if (!running.value) return;
  if (Date.now() - pollStartedAt >= LAB_POLL_LIMIT_MS) {
    // Give up waiting, not on the run: it may still be going in the backend.
    // The run id stays: through the awaitingFinish state the finished event
    // can still land the results of the run we stopped watching.
    running.value = false;
    stopPolling();
    awaitingFinish.value = true;
    stillRunningHint.value = true;
    return;
  }
  const targetId = pollTargetId();
  if (targetId === null) {
    // Every selected file is skipped: the run ends without a network call.
    await finishRun();
    return;
  }
  try {
    const calls = await getVlCalls(targetId);
    if (calls.some(call => Number(call?.id) > (baselineCallIds.get(targetId) ?? 0))) {
      await finishRun();
    }
  } catch {
    // The run keeps going; the next tick tries again.
  }
}

function resetWaitState() {
  running.value = false;
  runId.value = null;
  stopPolling();
}

async function finishRun() {
  awaitingFinish.value = false;
  stillRunningHint.value = false;
  resetWaitState();
  await loadResults();
}

async function loadResults() {
  const rows = await Promise.all(lastRunIds.map(async (id) => {
    const [annotations, calls] = await Promise.all([
      getVlAnnotations(id).catch(() => []),
      getVlCalls(id).catch(() => []),
    ]);
    const outcome = deriveFileOutcome({
      force: lastRunForce,
      baselineAnnotationId: baselineAnnotationIds.get(id) ?? 0,
      baselineCallId: baselineCallIds.get(id) ?? 0,
      annotations,
      calls,
    });
    const annotation = outcome.annotation;
    const template = annotation
      ? findTemplateVersion(templates.value, annotation.templateId, annotation.templateVersion)
      : null;
    const spec = template?.responseSpec ? parseResponseSpec(template.responseSpec).spec : {};
    const parsed = annotation ? parseResultJson(annotation.resultJson) : { ok: true, value: null, error: '' };
    return {
      id,
      annotations,
      file: fileById.value.get(id),
      outcome,
      entries: parsed.ok ? buildResultEntries(parsed.value, spec) : [],
      parseError: parsed.ok ? '' : parsed.error,
      calls,
    };
  }));
  // One merged update: concurrent read-modify-write inside the loop above
  // could lose a sibling's write.
  annotationsByFileId.value = {
    ...annotationsByFileId.value,
    ...Object.fromEntries(rows.map(row => [row.id, row.annotations])),
  };
  resultRows.value = rows;
  observeThumbs();
}

function latestCallOf(row: any) {
  return pickLatestCall(row?.calls);
}

function usageEntriesOf(row: any) {
  return formatUsage(latestCallOf(row));
}

function toggleRaw(fileId: number) {
  if (expandedRaw.has(fileId)) expandedRaw.delete(fileId);
  else expandedRaw.add(fileId);
}

function outcomeBadgeClass(status: string) {
  if (status === 'success') return 'badge-success';
  if (status === 'skipped') return 'badge-ghost';
  if (status === 'failed') return 'badge-error';
  return 'badge-warning';
}

// ----- F2 exact search -----

const searchQuery = ref('');
const searching = ref(false);
const searched = ref(false);
const searchKind = ref<'precise' | 'semantic'>('precise');
const llmRerank = ref(false);
const reranked = ref(false);
const unannotatedCount = ref(0);
const searchError = ref('');
const searchHits = ref<any[]>([]);
const searchFiles = ref<any[]>([]);

const searchResultItems = computed(() => {
  const fileByIds = new Map(searchFiles.value.map((file: any) => [Number(file?.id), file]));
  return searchHits.value.map(hit => ({
    hit,
    file: fileByIds.get(Number(hit.fileId)),
    badges: [
      ...(hit.rank ? [{ column: 'rank', labelText: `#${hit.rank}` }] : []),
      ...(hit.score != null && Number.isFinite(hit.score)
        ? [{ column: 'score', labelText: hit.score.toFixed(2) }]
        : []),
      ...hitBadges(hit).map(badge => ({ ...badge, labelText: badge.labelKey ? t(badge.labelKey) : badge.column })),
    ],
  }));
});

const matchColumnSummary = computed(() => {
  if (!searchHits.value.length) return '';
  return describeMatches(collectMatchedColumns(searchHits.value), Object.fromEntries(
    Object.entries(MATCH_COLUMN_KEYS).map(([column, key]) => [column, t(key)]),
  ));
});

async function runSearch(kind: 'precise' | 'semantic' = 'precise') {
  const query = searchQuery.value.trim();
  if (!query || searching.value) return;
  // A switch mid-search invalidates whatever the awaits return.
  const epoch = libraryEpoch;
  searchKind.value = kind;
  searching.value = true;
  searchError.value = '';
  reranked.value = false;
  unannotatedCount.value = 0;
  const rerankFailed = (error: unknown) => {
    // Keep the original ranking when the cloud call fails; the user still
    // gets results and sees why they were not reranked.
    searchError.value = `${t('vl.lab.llm_rerank_failed')}: ${
      error instanceof Error ? error.message : String(error)
    }`;
  };
  try {
    if (kind === 'semantic') {
      // Ranked by the stored image-description embeddings: the query and the
      // captions share one text-vector space, so the wording may differ.
      // Photos without a description cannot match and are simply absent.
      const limit = llmRerank.value ? RERANK_MAX_CANDIDATES : SEARCH_RESULT_LIMIT;
      const rows = await invoke<any[]>('search_captions', {
        query,
        threshold: null,
        limit,
      });
      let hits = captionHitsFrom(rows);
      let files = hits.length ? ((await getFilesByIds(hits.map(hit => Number(hit.fileId)))) ?? []) : [];
      if (llmRerank.value && hits.length) {
        // Compare the query with the stored image descriptions; semantic
        // neighbours without a description cannot be judged and stay at the end.
        const candidates = hits.slice(0, RERANK_MAX_CANDIDATES);
        try {
          const selected = await rerankHitList(query, candidates);
          hits = selected.hits.map((hit, index) => ({ ...hit, rank: index + 1 }));
          files = hits.length ? ((await getFilesByIds(hits.map(hit => Number(hit.fileId)))) ?? []) : [];
          reranked.value = selected.reranked;
          unannotatedCount.value = selected.unannotatedCount ?? 0;
        } catch (error) {
          rerankFailed(error);
        }
      }
      if (epoch !== libraryEpoch) return;
      searchHits.value = hits;
      searchFiles.value = files;
    } else {
      // With the LLM rerank checked the text search only needs to produce a
      // candidate set; the text model keeps the photos whose description matches.
      const limit = llmRerank.value ? RERANK_MAX_CANDIDATES : SEARCH_RESULT_LIMIT;
      const hits = await searchFts(query, limit);
      let list = Array.isArray(hits) ? hits : [];
      if (llmRerank.value && list.length) {
        try {
          const selected = await rerankHitList(query, list);
          list = selected.hits;
          reranked.value = selected.reranked;
          unannotatedCount.value = selected.unannotatedCount ?? 0;
        } catch (error) {
          rerankFailed(error);
        }
      }
      const fileIds = list.map(hit => Number(hit.fileId)).filter(id => id > 0);
      const files = fileIds.length ? ((await getFilesByIds(fileIds)) ?? []) : [];
      if (epoch !== libraryEpoch) return;
      searchHits.value = list;
      searchFiles.value = files;
    }
    if (epoch !== libraryEpoch) return;
    searched.value = true;
    observeThumbs();
  } catch (error) {
    errorMessage.value = error instanceof Error ? error.message : String(error);
  } finally {
    searching.value = false;
  }
}

// ----- lifecycle -----

function resetAfterLibrarySwitch() {
  libraryEpoch += 1;
  stopPolling();
  running.value = false;
  starting.value = false;
  runId.value = null;
  awaitingFinish.value = false;
  stillRunningHint.value = false;
  pendingConfirm.value = false;
  errorMessage.value = '';
  files.value = [];
  selectedIds.value = new Set();
  annotationsByFileId.value = {};
  resultRows.value = [];
  thumbs.clear();
  searchHits.value = [];
  searchFiles.value = [];
  searched.value = false;
  searchError.value = '';
  // The templates and the index status belong to the new library.
  void loadTemplates();
  void loadStatus();
  toast.info(t('vl.lab.library_switched'));
}

onMounted(async () => {
  observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      observer?.unobserve(entry.target);
      const id = Number((entry.target as HTMLElement).dataset.lazyId);
      void loadThumb(id);
    }
  }, { rootMargin: '120px' });

  stopFinishedListener = await listen('vl-analysis-finished', (event: any) => {
    const payload = event?.payload;
    if (!payload) return;
    lastFinishedRunId = String(payload.runId);
    if (!shouldFinishRun(running.value, awaitingFinish.value, runId.value, String(payload.runId))) return;
    void finishRun();
  });
  stopProgressListener = await listen('vl-analysis-progress', (event: any) => {
    const payload = event?.payload;
    if (!running.value || runId.value === null) return;
    if (String(payload?.runId) !== runId.value) return;
    progress.value = { processed: Number(payload?.processed || 0), total: Number(payload?.total || progress.value.total) };
  });

  // This is a separate window: it must hear the main window's library switch
  // itself. Every id loaded here belongs to the previous library, and a run
  // started from them would annotate whatever file carries the same id in
  // the new library (ids are per-library).
  stopLibraryListener = await listen('library-switched', () => {
    resetAfterLibrarySwitch();
  });

  await loadStatus();
  await loadTemplates();
  await loadFiles();
});

onBeforeUnmount(() => {
  stopPolling();
  observer?.disconnect();
  observer = null;
  stopFinishedListener?.();
  stopFinishedListener = null;
  stopProgressListener?.();
  stopProgressListener = null;
  stopLibraryListener?.();
  stopLibraryListener = null;
});
</script>
