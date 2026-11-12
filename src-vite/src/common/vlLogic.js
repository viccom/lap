/**
 * Image understanding (vision-language) - pure helpers.
 *
 * Every function here is side effect free so the behaviour can be unit tested
 * without a Tauri runtime. The invoke calls live in ./vlApi.js.
 */

/**
 * The settings tab that hosts the image-understanding page.
 *
 * `SETTINGS_TAB` in common/constants.ts is left untouched: this feature ships
 * on its own and 10 is the next free id after SETTINGS_TAB.METADATA.
 */
export const VL_SETTINGS_TAB = 10;

/// Placeholders the backend substitutes into a prompt template.
export const VL_PROMPT_PLACEHOLDERS = ['{file_name}', '{exif_summary}', '{taken_at}', '{camera}'];

/// Result fields with a translated label; other fields fall back to their key.
export const VL_FIELD_LABEL_KEYS = {
  caption: 'vl.field.caption',
  tags: 'vl.field.tags',
  summary: 'vl.field.summary',
  objects: 'vl.field.objects',
  scene: 'vl.field.scene',
  title: 'vl.field.title',
};

/**
 * What the user wants to do with the API key.
 * - keep: the stored key stays as it is
 * - clear: the stored key is removed
 * - set: the given key replaces the stored one
 */
export function resolveApiKeyAction(value) {
  if (value === null || value === undefined) return 'keep';
  if (String(value) === '') return 'clear';
  return 'set';
}

/**
 * The argument sent to `set_vl_config`.
 * `null` keeps the stored key, `''` clears it.
 */
export function toApiKeyArgument(value) {
  const action = resolveApiKeyAction(value);
  if (action === 'keep') return null;
  if (action === 'clear') return '';
  return String(value).trim();
}

/// The arguments of `set_vl_config`.
export function buildVlConfigArgs({ gatewayUrl = '', apiKey = null, model = '', rerankModel = '' } = {}) {
  return {
    gatewayUrl: String(gatewayUrl ?? '').trim(),
    apiKey: toApiKeyArgument(apiKey),
    model: String(model ?? '').trim(),
    rerankModel: String(rerankModel ?? '').trim(),
  };
}

/**
 * The `apiKey` argument of `set_vl_config` from the settings form state.
 *
 * A pending clear always sends '' (remove the stored key); otherwise a typed
 * key is sent and an empty field sends null (keep the stored key). The flag is
 * consumed by one save attempt, success or failure, so a failed save never
 * silently clears the key on the next one.
 */
export function resolveApiKeyInput(clearPending, typedKey) {
  if (clearPending) return '';
  const trimmed = String(typedKey ?? '').trim();
  return trimmed ? trimmed : null;
}

/// The arguments of `run_vl_analysis`.
export function buildRunArgs(fileIds, templateId, force = false) {
  const ids = (Array.isArray(fileIds) ? fileIds : [fileIds])
    .map(id => Number(id))
    .filter(id => Number.isFinite(id));
  return {
    fileIds: ids,
    templateId: Number(templateId),
    force: Boolean(force),
  };
}

/// The arguments of `create_vl_template` / `update_vl_template`.
/// The backend requires `systemTemplate`; an empty string means no system message.
export function buildTemplateArgs({ name = '', promptTemplate = '', responseSpec = '', systemTemplate = '' } = {}) {
  return {
    name: String(name ?? '').trim(),
    promptTemplate: String(promptTemplate ?? ''),
    systemTemplate: String(systemTemplate ?? ''),
    responseSpec: String(responseSpec ?? ''),
  };
}

/// The label of a template in a dropdown: `name (v2)`.
export function templateLabel(template) {
  if (!template) return '';
  const name = String(template.name ?? '');
  const version = Number(template.version ?? 0);
  return version ? `${name} (v${version})` : name;
}

/// The newest version of every template name.
export function pickLatestVersions(templates) {
  if (!Array.isArray(templates)) return [];
  const byName = new Map();
  for (const template of templates) {
    if (!template) continue;
    const key = String(template.name ?? '');
    const current = byName.get(key);
    if (!current || Number(template.version ?? 0) > Number(current.version ?? 0)) byName.set(key, template);
  }
  return [...byName.values()].sort((a, b) => String(a.name ?? '').localeCompare(String(b.name ?? '')));
}

/**
 * The edit and delete actions of a template row.
 *
 * A built-in template can be edited: the save creates a custom copy of it
 * and the built-in row itself stays untouched. A built-in template can never
 * be deleted.
 *
 * The returned keys are i18n keys, so the wording lives in the locales.
 */
export function templateEditKeys(template) {
  if (!template) {
    return {
      editable: false,
      deletable: false,
      editTitleKey: 'vl.edit_template',
      noticeKey: 'vl.new_version_notice',
    };
  }
  const builtin = Boolean(template.isBuiltin);
  return {
    editable: true,
    deletable: !builtin,
    editTitleKey: builtin ? 'vl.builtin_edit_copy' : 'vl.edit_template',
    noticeKey: builtin ? 'vl.builtin_edit_copy' : 'vl.new_version_notice',
  };
}

/// The templates grouped by name, newest version first.
export function groupTemplatesByName(templates) {
  if (!Array.isArray(templates)) return [];
  const groups = new Map();
  for (const template of templates) {
    if (!template) continue;
    const key = String(template.name ?? '');
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(template);
  }
  return [...groups.entries()]
    .map(([name, versions]) => ({
      name,
      versions: versions.slice().sort((a, b) => Number(b.version ?? 0) - Number(a.version ?? 0)),
    }))
    .sort((a, b) => a.name.localeCompare(b.name));
}

/// Parse a response spec. It must be a JSON object.
export function parseResponseSpec(text) {
  const raw = String(text ?? '').trim();
  if (!raw) return { ok: true, spec: {} };
  let parsed;
  try {
    parsed = JSON.parse(raw);
  } catch (error) {
    return { ok: false, error: String(error?.message ?? error) };
  }
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    return { ok: false, error: 'not an object' };
  }
  return { ok: true, spec: parsed };
}

/// Parse the `resultJson` of an annotation.
export function parseResultJson(text) {
  const raw = String(text ?? '').trim();
  if (!raw) return { ok: false, value: null, error: 'empty' };
  try {
    return { ok: true, value: JSON.parse(raw), error: '' };
  } catch (error) {
    return { ok: false, value: null, error: String(error?.message ?? error) };
  }
}

/// The i18n key of a result field label, or null when the field is unknown.
export function fieldLabelKey(key) {
  return VL_FIELD_LABEL_KEYS[String(key ?? '')] ?? null;
}

/**
 * Turn a result object into readable entries.
 *
 * The declared fields of the response spec come first, in spec order; the
 * remaining fields of the result follow. The kind drives the rendering:
 * `tags` for a list of strings, `text` for a scalar, `list` for any other
 * array and `object` for a nested object.
 */
export function buildResultEntries(value, spec) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  const declared = spec && typeof spec === 'object' && !Array.isArray(spec) ? Object.keys(spec) : [];
  const keys = [...declared.filter(key => Object.prototype.hasOwnProperty.call(value, key))];
  for (const key of Object.keys(value)) {
    if (!keys.includes(key)) keys.push(key);
  }
  return keys.map((key) => {
    const raw = value[key];
    const kind = Array.isArray(raw)
      ? (raw.every(item => typeof item === 'string') ? 'tags' : 'list')
      : (raw && typeof raw === 'object' ? 'object' : 'text');
    const entry = { key, labelKey: fieldLabelKey(key), kind, value: raw, text: '', items: [], fields: [] };
    if (kind === 'tags' || kind === 'list') {
      entry.items = raw.map(item => (typeof item === 'string' ? item : JSON.stringify(item)));
    } else if (kind === 'text') {
      entry.text = raw === null || raw === undefined ? '' : String(raw);
    } else {
      entry.fields = Object.entries(raw).map(([fieldKey, fieldValue]) => ({
        key: fieldKey,
        text: fieldValue === null || fieldValue === undefined ? '' : (typeof fieldValue === 'object' ? JSON.stringify(fieldValue) : String(fieldValue)),
      }));
    }
    return entry;
  });
}

/// A readable message from whatever the backend rejected with.
export function describeVlError(error) {
  if (error === null || error === undefined) return '';
  if (typeof error === 'string') return error.trim();
  if (typeof error === 'object') {
    if (typeof error.message === 'string' && error.message.trim()) return error.message.trim();
    if (typeof error.error === 'string' && error.error.trim()) return error.error.trim();
    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }
  return String(error);
}

/// A readable duration.
export function formatDuration(durationMs) {
  const value = Number(durationMs);
  if (!Number.isFinite(value) || value < 0) return '';
  if (value < 1000) return `${Math.round(value)} ms`;
  return `${(value / 1000).toFixed(1)} s`;
}

/// The token usage of a call, without the empty entries.
export function formatUsage(call) {
  if (!call) return [];
  const entries = [
    { key: 'input_tokens', labelKey: 'vl.input_tokens', value: call.inputTokens },
    { key: 'output_tokens', labelKey: 'vl.output_tokens', value: call.outputTokens },
    { key: 'cache_read_tokens', labelKey: 'vl.cache_read_tokens', value: call.cacheReadTokens },
    { key: 'thinking_tokens', labelKey: 'vl.thinking_tokens', value: call.thinkingTokens },
  ];
  return entries
    .filter(entry => entry.value !== null && entry.value !== undefined && Number.isFinite(Number(entry.value)))
    .map(entry => ({ ...entry, value: Number(entry.value) }));
}

/// The most recent call of a file.
export function pickLatestCall(calls) {
  if (!Array.isArray(calls) || !calls.length) return null;
  return calls[0];
}

/// The template a call or annotation belongs to.
export function findTemplateVersion(templates, templateId, templateVersion) {
  if (!Array.isArray(templates)) return null;
  const id = Number(templateId);
  const version = Number(templateVersion);
  const exact = templates.find(template => Number(template?.id) === id && Number(template?.version) === version);
  if (exact) return exact;
  return templates.find(template => Number(template?.id) === id) ?? null;
}
