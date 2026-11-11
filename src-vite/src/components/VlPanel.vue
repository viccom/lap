<template>
  <section class="shrink-0 border-t border-base-content/5 px-1 py-3 select-none">
    <div class="flex items-center gap-1">
      <IconSparkles class="w-3.5 h-3.5 shrink-0 text-base-content/30" />
      <span class="font-bold uppercase text-xs tracking-wide text-base-content/30">{{ $t('vl.panel_title') }}</span>
      <TButton
        :icon="IconRefresh"
        :tooltip="$t('vl.refresh')"
        :buttonSize="'small'"
        class="ml-auto"
        @click.stop="reloadAll"
      />
    </div>

    <!-- The gateway is not configured yet -->
    <div v-if="!configured" class="mt-2 space-y-2">
      <div class="rounded-box px-2 py-2 text-xs bg-base-100/20 border border-base-content/5">
        <div class="text-base-content/70">{{ $t('vl.not_configured') }}</div>
        <div class="mt-1 text-base-content/40">{{ $t('vl.not_configured_hint') }}</div>
      </div>
      <button class="t-button-default btn-outline gap-1.5 w-full justify-center" @click.stop="openSettings">
        <IconSettings class="w-4 h-4" />
        {{ $t('vl.open_settings') }}
      </button>
    </div>

    <!-- The gateway is configured -->
    <div v-else class="mt-2 space-y-2">
      <select
        class="select select-bordered select-xs w-full"
        :value="selectedTemplateId"
        :disabled="running || !templates.length"
        :aria-label="$t('vl.template')"
        @change="onTemplateChange"
      >
        <option v-if="!templates.length" :value="0">{{ $t('vl.no_template') }}</option>
        <option v-for="template in templates" :key="template.id" :value="template.id">
          {{ templateLabel(template) }}
        </option>
      </select>

      <div class="flex items-center gap-2">
        <button
          class="t-button-primary btn-outline gap-1.5 flex-1 justify-center"
          :disabled="running || !selectedTemplateId"
          @click.stop="startRun(false)"
        >
          <IconSparkles class="w-4 h-4" />
          {{ $t('vl.understand') }}
        </button>
        <button
          v-if="hasAnnotation"
          class="t-button-default gap-1.5"
          :disabled="running"
          :title="$t('vl.re_understand_hint')"
          @click.stop="startRun(true)"
        >
          {{ $t('vl.re_understand') }}
        </button>
      </div>

      <div v-if="running" class="rounded-box px-2 py-2 text-xs bg-base-100/20 border border-base-content/5" role="status" aria-live="polite">
        <div class="flex items-center gap-2 text-primary">
          <span class="loading loading-spinner loading-xs" />
          <span>{{ $t('vl.running') }}</span>
        </div>
        <div class="mt-1 text-base-content/40">{{ $t('vl.running_hint') }}</div>
        <button class="mt-1 text-base-content/50 underline hover:text-base-content" @click.stop="cancelRun">
          {{ $t('vl.cancel_run') }}
        </button>
      </div>

      <div v-if="errorMessage" class="rounded-box px-2 py-2 text-xs text-error bg-error/10 border border-error/20 whitespace-pre-wrap break-words">
        {{ errorMessage }}
      </div>

      <!-- annotations -->
      <div v-if="annotationEntries.length" class="space-y-2">
        <div
          v-for="group in annotationEntries"
          :key="group.id"
          class="rounded-box px-2 py-2 space-y-1.5 bg-base-100/20 border border-base-content/5"
        >
          <div class="flex items-center gap-2">
            <span class="text-xs font-semibold text-base-content/80 truncate">{{ group.templateLabel }}</span>
            <span class="text-[10px] text-base-content/30 shrink-0">{{ group.createdAt }}</span>
          </div>
          <div v-if="group.parseError" class="text-[10px] text-error break-words">
            {{ $t('vl.result_unreadable') }}: {{ group.parseError }}
          </div>
          <div v-for="entry in group.entries" :key="entry.key" class="space-y-0.5">
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
        </div>
      </div>
      <div v-else-if="!running" class="rounded-box px-2 py-2 text-xs text-base-content/40 bg-base-100/20 border border-base-content/5">
        {{ $t('vl.no_annotation') }}
      </div>

      <!-- the evidence of the last call -->
      <div v-if="latestCall" class="rounded-box border border-base-content/5 bg-base-100/20">
        <button
          type="button"
          class="w-full flex items-center gap-1 px-2 py-1.5 text-left text-base-content/70 hover:text-base-content"
          :aria-expanded="showRaw"
          @click.stop="showRaw = !showRaw"
        >
          <IconRight class="w-3 h-3 transition-transform duration-200" :class="{ 'rotate-90': showRaw }" />
          <span class="font-bold uppercase text-[10px] tracking-widest text-base-content/30">{{ $t('vl.raw_response') }}</span>
        </button>
        <div v-if="showRaw" class="px-2 pb-2 space-y-1.5 text-[11px]">
          <div class="grid grid-cols-[72px_minmax(0,1fr)] gap-x-2 gap-y-0.5">
            <span class="text-base-content/40">{{ $t('vl.status') }}</span>
            <span class="text-base-content/80 break-all">{{ latestCall.status }}</span>
            <span class="text-base-content/40">{{ $t('vl.model') }}</span>
            <span class="text-base-content/80 break-all">{{ latestCall.model || '-' }}</span>
            <span class="text-base-content/40">{{ $t('vl.request_id') }}</span>
            <span class="text-base-content/80 break-all">{{ latestCall.requestId || '-' }}</span>
            <span class="text-base-content/40">{{ $t('vl.duration') }}</span>
            <span class="text-base-content/80">{{ formatDuration(latestCall.durationMs) || '-' }}</span>
            <template v-if="usageEntries.length">
              <span class="text-base-content/40">{{ $t('vl.usage') }}</span>
              <span class="text-base-content/80">
                <span v-for="(entry, index) in usageEntries" :key="entry.key">
                  <span v-if="index" class="text-base-content/30"> / </span>{{ $t(entry.labelKey) }}: {{ entry.value }}
                </span>
              </span>
            </template>
          </div>
          <div v-if="latestCall.error" class="text-error break-words whitespace-pre-wrap">{{ latestCall.error }}</div>
          <div v-if="latestCall.thinkingText">
            <div class="text-base-content/40">{{ $t('vl.thinking') }}</div>
            <pre class="mt-0.5 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-box bg-base-300/40 p-1.5 text-[10px] leading-4 text-base-content/70">{{ latestCall.thinkingText }}</pre>
          </div>
          <div v-if="latestCall.promptActual">
            <div class="text-base-content/40">{{ $t('vl.prompt_actual') }}</div>
            <pre class="mt-0.5 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-box bg-base-300/40 p-1.5 text-[10px] leading-4 text-base-content/70">{{ latestCall.promptActual }}</pre>
          </div>
          <div v-if="latestCall.rawResponse">
            <div class="text-base-content/40">{{ $t('vl.raw_response') }}</div>
            <pre class="mt-0.5 max-h-60 overflow-auto whitespace-pre-wrap break-words rounded-box bg-base-300/40 p-1.5 text-[10px] leading-4 text-base-content/70">{{ latestCall.rawResponse }}</pre>
          </div>
        </div>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { useI18n } from 'vue-i18n';
import { config } from '@/common/config';
import { useToast } from '@/common/toast';
import { formatTimestamp } from '@/common/utils';
import { IconRefresh, IconRight, IconSettings, IconSparkles } from '@/common/icons';
import TButton from '@/components/TButton.vue';
import {
  cancelVlRun,
  getVlAnnotations,
  getVlCalls,
  getVlConfig,
  listVlTemplates,
  runVlAnalysis,
} from '@/common/vlApi';
import {
  VL_SETTINGS_TAB,
  buildResultEntries,
  findTemplateVersion,
  formatDuration,
  formatUsage,
  parseResponseSpec,
  parseResultJson,
  pickLatestCall,
  templateLabel,
} from '@/common/vlLogic';

const props = defineProps<{ fileId: number }>();

const { t } = useI18n();
const toast = useToast();

const configured = ref(false);
const templates = ref<any[]>([]);
const selectedTemplateId = ref(0);
const annotations = ref<any[]>([]);
const calls = ref<any[]>([]);
const running = ref(false);
const runId = ref<number | null>(null);
const errorMessage = ref('');
const showRaw = ref(false);

/// The id of the newest call before the current run started.
let baselineCallId = 0;
/// The run of the last finished event; it may arrive before the run id is known.
let lastFinishedRunId: number | null = null;
let pollTimer: ReturnType<typeof setInterval> | null = null;
let stopProgressListener: (() => void) | null = null;

const hasAnnotation = computed(() => annotations.value.some(annotation => (
  annotation.templateId === selectedTemplateId.value
)));

const annotationEntries = computed(() => annotations.value.map((annotation) => {
  const template = findTemplateVersion(templates.value, annotation.templateId, annotation.templateVersion);
  const parsed = parseResultJson(annotation.resultJson);
  const spec = template?.responseSpec ? parseResponseSpec(template.responseSpec).spec : {};
  return {
    id: annotation.id,
    templateLabel: template ? templateLabel(template) : `v${annotation.templateVersion}`,
    createdAt: annotation.createdAt ? formatTimestamp(annotation.createdAt, 'date_time') : '',
    entries: parsed.ok ? buildResultEntries(parsed.value, spec) : [],
    parseError: parsed.ok ? '' : parsed.error,
  };
}));

const latestCall = computed(() => pickLatestCall(calls.value));
const usageEntries = computed(() => formatUsage(latestCall.value));

function onTemplateChange(event: Event) {
  selectedTemplateId.value = Number((event.target as HTMLSelectElement).value);
}

async function loadConfig() {
  try {
    const view = await getVlConfig();
    configured.value = Boolean(view?.configured);
  } catch {
    configured.value = false;
  }
}

async function loadTemplates() {
  try {
    templates.value = await listVlTemplates();
  } catch {
    templates.value = [];
  }
  if (!templates.value.some(template => template.id === selectedTemplateId.value)) {
    selectedTemplateId.value = templates.value.length ? templates.value[0].id : 0;
  }
}

async function loadResults() {
  const fileId = Number(props.fileId);
  if (!fileId) return;
  try {
    const [annotationList, callList] = await Promise.all([getVlAnnotations(fileId), getVlCalls(fileId)]);
    annotations.value = annotationList;
    calls.value = callList;
  } catch (error) {
    errorMessage.value = error instanceof Error ? error.message : String(error);
  }
}

async function reloadAll() {
  errorMessage.value = '';
  await loadConfig();
  await loadTemplates();
  await loadResults();
}

async function startRun(force: boolean) {
  const fileId = Number(props.fileId);
  const templateId = Number(selectedTemplateId.value);
  if (!fileId || !templateId || running.value) return;
  errorMessage.value = '';
  try {
    const id = await runVlAnalysis([fileId], templateId, force);
    runId.value = Number(id);
    running.value = true;
    baselineCallId = Number(calls.value[0]?.id || 0);
    startPolling();
    // The backend may have finished the run before the answer arrived.
    if (lastFinishedRunId !== null && lastFinishedRunId === Number(id)) finishRun();
  } catch (error) {
    running.value = false;
    errorMessage.value = error instanceof Error ? error.message : String(error);
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
  // A single photo often needs a minute or more; the backend also emits a
  // finished event, the poll is the safety net for a missed event.
  pollTimer = setInterval(() => { void checkRunFinished(); }, 3000);
}

function stopPolling() {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function checkRunFinished() {
  if (!running.value) return;
  const fileId = Number(props.fileId);
  try {
    const callList = await getVlCalls(fileId);
    calls.value = callList;
    const newest = callList[0];
    if (newest && Number(newest.id) > baselineCallId) {
      finishRun();
    }
  } catch {
    // The run keeps going; the next tick tries again.
  }
}

function finishRun() {
  running.value = false;
  runId.value = null;
  stopPolling();
  void loadResults();
}

async function openSettings() {
  config.settings.tabIndex = VL_SETTINGS_TAB;
  const focused = await focusSettingsWindow();
  if (!focused) {
    toast.info(t('vl.settings_window_not_open'));
  }
}

async function focusSettingsWindow(): Promise<boolean> {
  try {
    const settingsWindow = await WebviewWindow.getByLabel('settings');
    if (!settingsWindow) return false;
    await settingsWindow.show();
    await settingsWindow.setFocus();
    return true;
  } catch (error) {
    console.error('Failed to show the settings window:', error);
    return false;
  }
}

onMounted(async () => {
  await reloadAll();
  stopProgressListener = await listen('vl-analysis-finished', (event: any) => {
    const payload = event?.payload;
    if (!payload) return;
    lastFinishedRunId = Number(payload.runId);
    if (!running.value) return;
    if (runId.value !== null && Number(payload.runId) !== runId.value) return;
    finishRun();
  });
});

onBeforeUnmount(() => {
  stopPolling();
  stopProgressListener?.();
  stopProgressListener = null;
});

watch(() => props.fileId, () => {
  errorMessage.value = '';
  showRaw.value = false;
  void loadResults();
});
</script>
