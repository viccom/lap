/**
 * Image understanding (vision-language) - backend calls.
 *
 * The invoke layer is deliberately thin: the argument shapes and the result
 * handling live in ./vlLogic.js so they can be tested without a Tauri runtime.
 *
 * Every function takes an optional `invoker` as its last argument; it defaults
 * to the real Tauri invoke and exists so the tests can observe the payloads.
 */
import { invoke } from '@tauri-apps/api/core';
import {
  buildRunArgs,
  buildTemplateArgs,
  buildVlConfigArgs,
  describeVlError,
} from './vlLogic.js';

function fail(action, error) {
  console.error(`Failed to ${action}:`, error);
  const message = describeVlError(error);
  throw new Error(message || `${action} failed`);
}

// gateway configuration

/// The gateway settings. The API key is never part of the answer.
export async function getVlConfig(invoker = invoke) {
  try {
    return await invoker('get_vl_config');
  } catch (error) {
    fail('get the image understanding configuration', error);
  }
}

/// Store the gateway settings. `apiKey` of null keeps the stored key, '' clears it.
export async function setVlConfig({ gatewayUrl, apiKey, model } = {}, invoker = invoke) {
  const args = buildVlConfigArgs({ gatewayUrl, apiKey, model });
  try {
    return await invoker('set_vl_config', args);
  } catch (error) {
    fail('save the image understanding configuration', error);
  }
}

// templates

/// Every template version, newest version of a name first.
export async function listVlTemplates(invoker = invoke) {
  try {
    const templates = await invoker('list_vl_templates');
    return Array.isArray(templates) ? templates : [];
  } catch (error) {
    fail('list the image understanding templates', error);
  }
}

/// Create a template. The first version of a name is version 1.
export async function createVlTemplate(template, invoker = invoke) {
  const args = buildTemplateArgs(template);
  try {
    return await invoker('create_vl_template', args);
  } catch (error) {
    fail('create the image understanding template', error);
  }
}

/// Edit a template. The backend creates the next version and keeps the old row.
export async function updateVlTemplate(id, template, invoker = invoke) {
  const args = buildTemplateArgs(template);
  try {
    return await invoker('update_vl_template', { id: Number(id), ...args });
  } catch (error) {
    fail('update the image understanding template', error);
  }
}

/// Delete a template. Built-in templates are protected by the backend.
export async function deleteVlTemplate(id, invoker = invoke) {
  try {
    return await invoker('delete_vl_template', { id: Number(id) });
  } catch (error) {
    fail('delete the image understanding template', error);
  }
}

// analysis

/// Queue an analysis run and return its id.
export async function runVlAnalysis(fileIds, templateId, force = false, invoker = invoke) {
  const args = buildRunArgs(fileIds, templateId, force);
  try {
    return await invoker('run_vl_analysis', args);
  } catch (error) {
    fail('start the image understanding run', error);
  }
}

/// Cancel a running analysis.
export async function cancelVlRun(runId, invoker = invoke) {
  try {
    return await invoker('cancel_vl_run', { runId: Number(runId) });
  } catch (error) {
    fail('cancel the image understanding run', error);
  }
}

/// The annotations of one file, newest first.
export async function getVlAnnotations(fileId, invoker = invoke) {
  try {
    const annotations = await invoker('get_vl_annotations', { fileId: Number(fileId) });
    return Array.isArray(annotations) ? annotations : [];
  } catch (error) {
    fail('get the image understanding annotations', error);
  }
}

/// The call evidence of one file, newest first.
export async function getVlCalls(fileId, invoker = invoke) {
  try {
    const calls = await invoker('get_vl_calls', { fileId: Number(fileId) });
    return Array.isArray(calls) ? calls : [];
  } catch (error) {
    fail('get the image understanding calls', error);
  }
}
