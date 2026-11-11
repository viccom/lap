<template>
  <div class="rounded-box p-2 space-y-2 bg-base-300/30 border border-base-content/5 shadow-sm">
    <div class="flex items-center gap-2 text-base-content/30">
      <span class="font-bold uppercase text-[10px] tracking-widest">{{ $t('vl.template_manager_title') }}</span>
      <TButton
        :icon="IconAdd"
        :tooltip="$t('vl.new_template')"
        :buttonSize="'small'"
        class="ml-auto"
        @click.stop="startCreate"
      />
    </div>
    <div class="text-xs text-base-content/40">{{ $t('vl.template_manager_hint') }}</div>

    <div v-if="groups.length" class="space-y-2">
      <div v-for="group in groups" :key="group.name" class="rounded-box p-2 bg-base-100/30 border border-base-content/5 space-y-1">
        <div class="flex items-center gap-2">
          <span class="text-sm font-medium truncate">{{ group.name }}</span>
          <span class="text-[10px] text-base-content/30 shrink-0">{{ $t('vl.version', { version: group.versions[0].version }) }}</span>
        </div>
        <div
          v-for="template in group.versions"
          :key="template.id"
          class="flex items-center gap-2 px-1 py-0.5 text-xs text-base-content/70"
        >
          <span class="w-16 shrink-0 text-base-content/40">{{ $t('vl.version', { version: template.version }) }}</span>
          <span
            v-if="template.isBuiltin"
            class="shrink-0 px-1.5 rounded-box text-[10px] border border-base-content/10 bg-base-content/5 text-base-content/50"
          >{{ $t('vl.builtin') }}</span>
          <span
            class="shrink-0 px-1.5 rounded-box text-[10px] border border-base-content/10 bg-base-content/5 text-base-content/50"
          >{{ template.enabled ? $t('vl.enabled') : $t('vl.disabled') }}</span>
          <div class="ml-auto flex items-center gap-1 shrink-0">
            <button
              type="button"
              class="p-1 text-base-content/40 hover:text-base-content cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed"
              :disabled="!templateEditKeys(template).editable"
              :title="$t(templateEditKeys(template).editTitleKey)"
              @click.stop="startEdit(template)"
            >
              <IconEdit class="w-4 h-4" />
            </button>
            <button
              type="button"
              class="p-1 text-base-content/40 hover:text-error cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed"
              :disabled="!templateEditKeys(template).deletable"
              :title="templateEditKeys(template).deletable ? $t('vl.delete_template') : $t('vl.builtin_readonly')"
              @click.stop="askDelete(template)"
            >
              <IconTrash class="w-4 h-4" />
            </button>
          </div>
        </div>
      </div>
    </div>
    <div v-else class="rounded-box px-2 py-3 text-xs text-base-content/30 bg-base-100/20 border border-base-content/5">
      {{ $t('vl.no_template') }}
    </div>

    <!-- create / edit form -->
    <div v-if="form" class="rounded-box p-2 space-y-2 bg-base-100/30 border border-primary/20">
      <div class="text-[10px] font-bold uppercase tracking-widest text-base-content/40">
        {{ form.id === null ? $t('vl.new_template') : $t('vl.edit_template') }}
      </div>

      <label class="block space-y-1">
        <span class="text-xs text-base-content/70">{{ $t('vl.template_name') }}</span>
        <input
          v-model="form.name"
          type="text"
          class="input input-sm w-full"
          maxlength="120"
          :placeholder="$t('vl.template_name')"
        />
      </label>

      <label class="block space-y-1">
        <span class="text-xs text-base-content/70">{{ $t('vl.prompt_template') }}</span>
        <textarea
          v-model="form.promptTemplate"
          rows="5"
          class="textarea textarea-sm w-full font-mono text-[11px]"
          :placeholder="$t('vl.prompt_template')"
        ></textarea>
        <span class="block text-[10px] text-base-content/40">{{ $t('vl.placeholders_hint') }}</span>
      </label>

      <label class="block space-y-1">
        <span class="text-xs text-base-content/70">{{ $t('vl.system_template') }}</span>
        <textarea
          v-model="form.systemTemplate"
          rows="3"
          class="textarea textarea-sm w-full font-mono text-[11px]"
          :placeholder="$t('vl.system_template')"
        ></textarea>
      </label>

      <label class="block space-y-1">
        <span class="text-xs text-base-content/70">{{ $t('vl.response_spec') }}</span>
        <textarea
          v-model="form.responseSpec"
          rows="3"
          class="textarea textarea-sm w-full font-mono text-[11px]"
          :class="{ 'textarea-error': specError }"
          placeholder='{"caption": "description", "tags": "tag list"}'
        ></textarea>
        <span class="block text-[10px] text-base-content/40">{{ $t('vl.response_spec_hint') }}</span>
        <span v-if="specError" class="block text-[10px] text-error">{{ $t('vl.invalid_response_spec') }}</span>
      </label>

      <div v-if="form.id !== null" class="text-[10px] text-base-content/40">{{ $t(formNoticeKey) }}</div>

      <div class="flex items-center justify-end gap-2">
        <button class="t-button-default" :disabled="saving" @click.stop="cancelForm">{{ $t('msgbox.cancel') }}</button>
        <button class="t-button-primary" :disabled="saving" @click.stop="save">
          {{ saving ? $t('vl.saving') : $t('msgbox.ok') }}
        </button>
      </div>
    </div>

    <MessageBox
      v-if="deleteTarget"
      :title="$t('vl.delete_template')"
      :message="$t('vl.delete_template_confirm_message', { name: deleteTarget.name })"
      :OkText="$t('vl.delete_template')"
      :cancelText="$t('msgbox.cancel')"
      :warningOk="true"
      @ok="confirmDelete"
      @cancel="deleteTarget = null"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { useToast } from '@/common/toast';
import { IconAdd, IconEdit, IconTrash } from '@/common/icons';
import TButton from '@/components/TButton.vue';
import MessageBox from '@/components/MessageBox.vue';
import { createVlTemplate, deleteVlTemplate, listVlTemplates, updateVlTemplate } from '@/common/vlApi';
import {
  groupTemplatesByName,
  parseResponseSpec,
  templateEditKeys,
} from '@/common/vlLogic';

type TemplateForm = {
  id: number | null;
  isBuiltin: boolean;
  name: string;
  promptTemplate: string;
  systemTemplate: string;
  responseSpec: string;
};

const { t } = useI18n();
const toast = useToast();

const templates = ref<any[]>([]);
const form = ref<TemplateForm | null>(null);
const saving = ref(false);
const deleteTarget = ref<any | null>(null);

const groups = computed(() => groupTemplatesByName(templates.value));
const specError = computed(() => {
  if (!form.value) return false;
  return !parseResponseSpec(form.value.responseSpec).ok;
});
// Editing a built-in template saves a copy of it, the built-in row stays as it is.
const formNoticeKey = computed(() =>
  templateEditKeys({ isBuiltin: form.value?.isBuiltin }).noticeKey,
);

async function reload() {
  try {
    templates.value = await listVlTemplates();
  } catch (error) {
    templates.value = [];
    toast.error(error instanceof Error ? error.message : String(error));
  }
}

function startCreate() {
  form.value = {
    id: null,
    isBuiltin: false,
    name: '',
    promptTemplate: '',
    systemTemplate: '',
    responseSpec: '{}',
  };
}

function startEdit(template: any) {
  if (!templateEditKeys(template).editable) return;
  form.value = {
    id: Number(template.id),
    isBuiltin: Boolean(template.isBuiltin),
    name: String(template.name ?? ''),
    promptTemplate: String(template.promptTemplate ?? ''),
    systemTemplate: String(template.systemTemplate ?? ''),
    responseSpec: String(template.responseSpec ?? ''),
  };
}

function cancelForm() {
  form.value = null;
}

async function save() {
  const current = form.value;
  if (!current || saving.value) return;
  const name = current.name.trim();
  if (!name) {
    toast.error(t('vl.template_name_required'));
    return;
  }
  if (!current.promptTemplate.trim()) {
    toast.error(t('vl.prompt_template_required'));
    return;
  }
  const spec = parseResponseSpec(current.responseSpec);
  if (!spec.ok) {
    toast.error(t('vl.invalid_response_spec'));
    return;
  }
  saving.value = true;
  try {
    const payload = {
      name,
      promptTemplate: current.promptTemplate,
      responseSpec: current.responseSpec,
      systemTemplate: current.systemTemplate,
    };
    if (current.id === null) {
      await createVlTemplate(payload);
      toast.success(t('vl.template_created'));
    } else {
      await updateVlTemplate(current.id, payload);
      toast.success(t('vl.template_saved'));
    }
    form.value = null;
    await reload();
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  } finally {
    saving.value = false;
  }
}

function askDelete(template: any) {
  if (!templateEditKeys(template).deletable) return;
  deleteTarget.value = template;
}

async function confirmDelete() {
  const target = deleteTarget.value;
  if (!target) return;
  deleteTarget.value = null;
  try {
    await deleteVlTemplate(Number(target.id));
    toast.success(t('vl.template_deleted'));
    await reload();
  } catch (error) {
    toast.error(error instanceof Error ? error.message : String(error));
  }
}

onMounted(() => {
  void reload();
});
</script>
