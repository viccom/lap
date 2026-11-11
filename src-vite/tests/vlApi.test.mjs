import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import test from 'node:test';

import {
  cancelVlRun,
  createVlTemplate,
  deleteVlTemplate,
  getVlAnnotations,
  getVlCalls,
  getVlConfig,
  listVlTemplates,
  runVlAnalysis,
  setVlConfig,
  updateVlTemplate,
} from '../src/common/vlApi.js';
import {
  buildResultEntries,
  buildRunArgs,
  buildTemplateArgs,
  buildVlConfigArgs,
  describeVlError,
  detectSystemTemplateSupport,
  formatUsage,
  groupTemplatesByName,
  parseResponseSpec,
  parseResultJson,
  pickLatestVersions,
  resolveApiKeyAction,
  templateEditKeys,
  templateLabel,
  toApiKeyArgument,
} from '../src/common/vlLogic.js';

/// An invoker that records the calls and answers with the given result.
function recorder(result = null) {
  const calls = [];
  const invoker = async (command, args) => {
    calls.push({ command, args });
    return typeof result === 'function' ? result(command, args) : result;
  };
  return { calls, invoker };
}

test('run_vl_analysis sends the file ids as an array with the template id and the force flag', async () => {
  const { calls, invoker } = recorder(7);
  const runId = await runVlAnalysis([12, 13], 4, true, invoker);
  assert.equal(runId, 7);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].command, 'run_vl_analysis');
  assert.deepEqual(calls[0].args, { fileIds: [12, 13], templateId: 4, force: true });
});

test('run_vl_analysis defaults to force false and accepts a single file id', async () => {
  const { calls, invoker } = recorder(1);
  await runVlAnalysis(42, 3, undefined, invoker);
  assert.deepEqual(calls[0].args, { fileIds: [42], templateId: 3, force: false });
});

test('buildRunArgs keeps the file ids numeric and the force flag boolean', () => {
  assert.deepEqual(buildRunArgs(['8', 9], '5', 1), { fileIds: [8, 9], templateId: 5, force: true });
  assert.deepEqual(buildRunArgs([], 5, false), { fileIds: [], templateId: 5, force: false });
});

test('set_vl_config keeps the stored key when no key is given', async () => {
  const { calls, invoker } = recorder({ gatewayUrl: 'https://gw', model: 'auto/x', configured: true });
  const view = await setVlConfig({ gatewayUrl: ' https://gw ', model: ' auto/x ' }, invoker);
  assert.equal(calls[0].command, 'set_vl_config');
  assert.deepEqual(calls[0].args, { gatewayUrl: 'https://gw', apiKey: null, model: 'auto/x' });
  assert.equal(view.configured, true);
});

test('set_vl_config clears the stored key with an empty string', async () => {
  const { calls, invoker } = recorder({ gatewayUrl: '', model: '', configured: false });
  await setVlConfig({ gatewayUrl: '', apiKey: '', model: '' }, invoker);
  assert.equal(calls[0].args.apiKey, '');
});

test('set_vl_config sends the new key when one is typed', async () => {
  const { calls, invoker } = recorder({ gatewayUrl: '', model: '', configured: true });
  await setVlConfig({ gatewayUrl: 'https://gw', apiKey: '  sk-test  ', model: 'm' }, invoker);
  assert.equal(calls[0].args.apiKey, 'sk-test');
});

test('the api key argument distinguishes keep, clear and set', () => {
  assert.equal(resolveApiKeyAction(undefined), 'keep');
  assert.equal(resolveApiKeyAction(null), 'keep');
  assert.equal(resolveApiKeyAction(''), 'clear');
  assert.equal(resolveApiKeyAction('sk-1'), 'set');
  assert.equal(toApiKeyArgument(undefined), null);
  assert.equal(toApiKeyArgument(''), '');
  assert.equal(toApiKeyArgument(' sk-1 '), 'sk-1');
  assert.deepEqual(buildVlConfigArgs({ gatewayUrl: ' gw ', apiKey: null, model: ' m ' }), {
    gatewayUrl: 'gw',
    apiKey: null,
    model: 'm',
  });
});

test('the gateway configuration never carries the api key back to the frontend', async () => {
  const { invoker } = recorder({ gatewayUrl: 'https://gw', model: 'auto/x', configured: true });
  const view = await getVlConfig(invoker);
  assert.deepEqual(Object.keys(view).sort(), ['configured', 'gatewayUrl', 'model']);
  assert.equal(Object.prototype.hasOwnProperty.call(view, 'apiKey'), false);
});

test('the template arguments only carry a system template when the backend supports one', async () => {
  const plain = buildTemplateArgs({ name: ' caption ', promptTemplate: 'p', responseSpec: '{}' });
  assert.deepEqual(plain, { name: 'caption', promptTemplate: 'p', responseSpec: '{}' });

  const withSystem = buildTemplateArgs({
    name: 'caption',
    promptTemplate: 'p',
    responseSpec: '{}',
    systemTemplate: 'be nice',
    supportsSystemTemplate: true,
  });
  assert.equal(withSystem.systemTemplate, 'be nice');
});

test('the template commands use the backend argument names', async () => {
  const { calls, invoker } = recorder({ id: 3, name: 'caption', version: 1 });
  await createVlTemplate({ name: 'caption', promptTemplate: 'p', responseSpec: '{}' }, invoker);
  await updateVlTemplate(3, { name: 'caption', promptTemplate: 'p2', responseSpec: '{}' }, invoker);
  await deleteVlTemplate(3, invoker);
  assert.equal(calls[0].command, 'create_vl_template');
  assert.deepEqual(calls[0].args, { name: 'caption', promptTemplate: 'p', responseSpec: '{}' });
  assert.equal(calls[1].command, 'update_vl_template');
  assert.equal(calls[1].args.id, 3);
  assert.equal(calls[1].args.promptTemplate, 'p2');
  assert.equal(calls[2].command, 'delete_vl_template');
  assert.deepEqual(calls[2].args, { id: 3 });
});

test('the system template is only used when the backend reports it', () => {
  assert.equal(detectSystemTemplateSupport([{ id: 1, name: 'a' }]), false);
  assert.equal(detectSystemTemplateSupport([{ id: 1, name: 'a', systemTemplate: '' }]), true);
  assert.equal(detectSystemTemplateSupport([]), false);
});

test('the annotation and call commands address one file', async () => {
  const { calls, invoker } = recorder([]);
  await getVlAnnotations(9, invoker);
  await getVlCalls(9, invoker);
  await cancelVlRun(5, invoker);
  assert.deepEqual(calls[0], { command: 'get_vl_annotations', args: { fileId: 9 } });
  assert.deepEqual(calls[1], { command: 'get_vl_calls', args: { fileId: 9 } });
  assert.deepEqual(calls[2], { command: 'cancel_vl_run', args: { runId: 5 } });
});

test('a failing backend call reports the error text of the backend', async () => {
  const invoker = async () => {
    throw '网关返回错误(401): invalid key';
  };
  await assert.rejects(() => runVlAnalysis([1], 2, false, invoker), (error) => {
    assert.ok(error instanceof Error);
    assert.equal(error.message, '网关返回错误(401): invalid key');
    return true;
  });
});

test('the error text of any rejection shape is readable', () => {
  assert.equal(describeVlError('boom'), 'boom');
  assert.equal(describeVlError(new Error('failed')), 'failed');
  assert.equal(describeVlError({ message: 'from object' }), 'from object');
  assert.equal(describeVlError({ error: 'nested' }), 'nested');
  assert.equal(describeVlError(null), '');
});

test('the result of an annotation is rendered field by field', () => {
  const parsed = parseResultJson('{"caption":"一只猫","tags":["猫","室内"]}');
  assert.equal(parsed.ok, true);
  const entries = buildResultEntries(parsed.value, { caption: '描述', tags: '标签' });
  assert.equal(entries.length, 2);
  assert.equal(entries[0].key, 'caption');
  assert.equal(entries[0].kind, 'text');
  assert.equal(entries[0].text, '一只猫');
  assert.equal(entries[0].labelKey, 'vl.field.caption');
  assert.equal(entries[1].key, 'tags');
  assert.equal(entries[1].kind, 'tags');
  assert.deepEqual(entries[1].items, ['猫', '室内']);
});

test('the declared fields of the response spec come first', () => {
  const parsed = parseResultJson('{"extra":"x","caption":"c"}');
  const entries = buildResultEntries(parsed.value, { caption: '描述' });
  assert.deepEqual(entries.map(entry => entry.key), ['caption', 'extra']);
  assert.equal(entries[1].labelKey, null);
});

test('a broken result json is reported instead of being rendered', () => {
  const parsed = parseResultJson('not json');
  assert.equal(parsed.ok, false);
  assert.deepEqual(buildResultEntries(null, {}), []);
});

test('a response spec must be a JSON object', () => {
  assert.equal(parseResponseSpec('{"caption":"c"}').ok, true);
  assert.equal(parseResponseSpec('').ok, true);
  assert.equal(parseResponseSpec('[1,2]').ok, false);
  assert.equal(parseResponseSpec('{oops').ok, false);
});

test('the templates are labelled with their version and grouped by name', () => {
  const templates = [
    { id: 1, name: 'caption', version: 1 },
    { id: 2, name: 'caption', version: 2 },
    { id: 3, name: 'tags', version: 1 },
  ];
  assert.equal(templateLabel(templates[0]), 'caption (v1)');
  assert.deepEqual(pickLatestVersions(templates).map(template => template.id), [2, 3]);
  const groups = groupTemplatesByName(templates);
  assert.equal(groups.length, 2);
  assert.equal(groups[0].name, 'caption');
  assert.deepEqual(groups[0].versions.map(version => version.version), [2, 1]);
});

test('the token usage of a call skips the values the gateway did not report', () => {
  const usage = formatUsage({ inputTokens: 71, outputTokens: 205, cacheReadTokens: null, thinkingTokens: 144 });
  assert.deepEqual(usage.map(entry => entry.key), ['input_tokens', 'output_tokens', 'thinking_tokens']);
  assert.equal(usage[0].value, 71);
});

test('a built-in template can be edited as a copy but never deleted', () => {
  const builtin = { id: 1, name: 'builtin caption', version: 1, isBuiltin: true };
  const builtinKeys = templateEditKeys(builtin);
  // Editing is allowed: the save creates a custom copy of the built-in row.
  assert.equal(builtinKeys.editable, true);
  assert.equal(builtinKeys.deletable, false);
  assert.equal(builtinKeys.editTitleKey, 'vl.builtin_edit_copy');
  assert.equal(builtinKeys.noticeKey, 'vl.builtin_edit_copy');

  const own = { id: 2, name: 'caption', version: 1, isBuiltin: false };
  const ownKeys = templateEditKeys(own);
  assert.equal(ownKeys.editable, true);
  assert.equal(ownKeys.deletable, true);
  assert.equal(ownKeys.editTitleKey, 'vl.edit_template');
  assert.equal(ownKeys.noticeKey, 'vl.new_version_notice');
});

test('a missing template has no edit and no delete action', () => {
  const keys = templateEditKeys(null);
  assert.equal(keys.editable, false);
  assert.equal(keys.deletable, false);
});

test('every locale carries the built-in copy wording', async () => {
  const directory = new URL('../src/locales/', import.meta.url);
  const files = (await readdir(directory)).filter(name => name.endsWith('.json'));
  const english = JSON.parse(await readFile(new URL('en.json', directory), 'utf8'));
  assert.ok(english.vl.builtin_edit_copy.includes('copy'));
  assert.ok(english.vl.builtin_edit_copy.includes('built-in'));
  for (const file of files) {
    const messages = JSON.parse(await readFile(new URL(file, directory), 'utf8'));
    const text = messages.vl?.builtin_edit_copy;
    assert.equal(typeof text, 'string', `${file} misses vl.builtin_edit_copy`);
    assert.ok(text.trim().length > 0, `${file} has an empty vl.builtin_edit_copy`);
  }
  // The Chinese locales carry a real translation, not the English placeholder.
  for (const file of ['zh-CN.json', 'zh-TW.json']) {
    const messages = JSON.parse(await readFile(new URL(file, directory), 'utf8'));
    assert.notEqual(messages.vl.builtin_edit_copy, english.vl.builtin_edit_copy, file);
  }
});

test('every locale carries the image understanding keys', async () => {
  const directory = new URL('../src/locales/', import.meta.url);
  const files = (await readdir(directory)).filter(name => name.endsWith('.json'));
  assert.equal(files.length, 17);
  const english = JSON.parse(await readFile(new URL('en.json', directory), 'utf8'));
  const expectedVl = Object.keys(english.vl).sort();
  const expectedSettings = Object.keys(english.settings.vl).sort();
  assert.ok(expectedVl.includes('panel_title'));
  assert.ok(expectedSettings.includes('api_key'));

  for (const file of files) {
    const messages = JSON.parse(await readFile(new URL(file, directory), 'utf8'));
    assert.deepEqual(Object.keys(messages.vl ?? {}).sort(), expectedVl, `${file} misses vl keys`);
    assert.deepEqual(Object.keys(messages.settings?.vl ?? {}).sort(), expectedSettings, `${file} misses settings.vl keys`);
    for (const key of expectedVl) {
      if (key === 'field') continue;
      assert.equal(typeof messages.vl[key], 'string', `${file} misses vl.${key}`);
    }
    for (const key of expectedSettings) {
      assert.equal(typeof messages.settings.vl[key], 'string', `${file} misses settings.vl.${key}`);
    }
    assert.deepEqual(Object.keys(messages.vl.field).sort(), Object.keys(english.vl.field).sort());
  }
});
