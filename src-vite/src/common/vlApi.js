/**
 * Image understanding (vision-language) - backend calls.
 *
 * The invoke layer is deliberately thin: the argument shapes and the result
 * handling live in ./vlLogic.js so they can be tested without a Tauri runtime.
 *
 * Every function takes an optional `invoker` as its last argument; it defaults
 * to the real Tauri invoke and exists so the tests can observe the payloads.
 */
import { invoke } from './transport/index.js';
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
export async function setVlConfig(
  { gatewayUrl, apiKey, model, rerankModel, embedSource, embedModel, protocol, maxConcurrency } = {},
  invoker = invoke,
) {
  const args = buildVlConfigArgs({ gatewayUrl, apiKey, model, rerankModel, embedSource, embedModel, protocol, maxConcurrency });
  try {
    return await invoker('set_vl_config', args);
  } catch (error) {
    fail('save the image understanding configuration', error);
  }
}

/// Ask the configured model to describe a two-colour probe image.
///
/// Gateways may advertise `vision: "supported"` while serving a text-only
/// backend; such a model "describes" photos from EXIF alone. The probe cannot
/// be answered without seeing the pixels, so this is the honest capability
/// check to run before a whole-library understanding pass.
/// `error` is present when the check could not run (gateway unreachable, …);
/// `ok` is false in that case as well.
/** @returns {Promise<{ ok: boolean, answer: string, error?: string } | undefined>} */
export async function checkVlVision(invoker = invoke) {
  try {
    const answer = await invoker('check_vl_vision');
    return {
      ok: Boolean(answer?.ok),
      answer: String(answer?.answer ?? ''),
      error: answer?.error ? String(answer.error) : '',
    };
  } catch (error) {
    fail('check the gateway vision capability', error);
  }
}

/// The models the gateway offers. `authRequired` marks a 401 answer: the
/// list is unusable but the chat surface may still work.
export async function listVlModels(invoker = invoke) {
  try {
    const answer = await invoker('list_vl_models');
    return {
      models: Array.isArray(answer?.models) ? answer.models : [],
      authRequired: Boolean(answer?.authRequired),
    };
  } catch (error) {
    fail('list the gateway models', error);
  }
}

/// Every persisted run across the configured libraries, newest first.
/** @returns {Promise<any[]>} */
export async function listVlRuns(invoker = invoke) {
  try {
    const runs = await invoker('list_vl_runs');
    return Array.isArray(runs) ? runs : [];
  } catch (error) {
    fail('list the image understanding runs', error);
  }
}

/// The call evidence of one run, oldest first.
export async function listVlRunCalls(runId, invoker = invoke) {
  try {
    const calls = await invoker('list_vl_run_calls', { runId: String(runId ?? '') });
    return Array.isArray(calls) ? calls : [];
  } catch (error) {
    fail('list the calls of the image understanding run', error);
  }
}

// templates

/// Every template version, newest version of a name first.
/** @returns {Promise<any[]>} */
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

/// Cancel a running analysis. Run ids are UUID strings and are passed
/// through verbatim — never through Number().
export async function cancelVlRun(runId, invoker = invoke) {
  try {
    return await invoker('cancel_vl_run', { runId: String(runId ?? '') });
  } catch (error) {
    fail('cancel the image understanding run', error);
  }
}

/// Retry the failed files of one run as a fresh run; returns the new run id.
export async function retryVlRun(runId, invoker = invoke) {
  try {
    return await invoker('retry_vl_run', { runId: String(runId ?? '') });
  } catch (error) {
    fail('retry the image understanding run', error);
  }
}

/// Delete one terminal run together with its call evidence.
export async function deleteVlRun(runId, invoker = invoke) {
  try {
    await invoker('delete_vl_run', { runId: String(runId ?? '') });
    return true;
  } catch (error) {
    fail('delete the image understanding run', error);
  }
}

/// Delete every terminal run across all libraries; returns the removed count.
export async function clearVlRuns(invoker = invoke) {
  try {
    const count = await invoker('clear_vl_runs', {});
    return Number(count) || 0;
  } catch (error) {
    fail('clear the finished image understanding runs', error);
  }
}

// scheduled tasks (queued file sets that fire at a fixed time)

/// Queue a task: the picked files plus the template, firing at `date time`.
export async function queueVlTask(fileIds, templateId, force, time, date, invoker = invoke) {
  const ids = (Array.isArray(fileIds) ? fileIds : [])
    .map(id => Number(id))
    .filter(id => Number.isFinite(id) && id > 0);
  try {
    return await invoker('queue_vl_task', {
      fileIds: ids,
      templateId: Number(templateId) || 0,
      force: force === true,
      time: String(time ?? ''),
      date: String(date ?? ''),
    });
  } catch (error) {
    fail('queue the image understanding task', error);
  }
}

/// The queued tasks of every library.
/** @returns {Promise<any[]>} */
export async function listVlTasks(invoker = invoke) {
  try {
    const tasks = await invoker('list_vl_tasks', {});
    return Array.isArray(tasks) ? tasks : [];
  } catch (error) {
    fail('list the image understanding tasks', error);
  }
}

/// Move a queued task to a new date and time (revives expired/failed ones).
export async function updateVlTaskTime(id, time, date, invoker = invoke) {
  try {
    await invoker('update_vl_task_time', {
      id: String(id ?? ''),
      time: String(time ?? ''),
      date: String(date ?? ''),
    });
    return true;
  } catch (error) {
    fail('reschedule the image understanding task', error);
  }
}

/// Remove a queued task.
export async function deleteVlTask(id, invoker = invoke) {
  try {
    await invoker('delete_vl_task', { id: String(id ?? '') });
    return true;
  } catch (error) {
    fail('delete the image understanding task', error);
  }
}

/// The annotations of one file, newest first.
/** @param {number | string} fileId @returns {Promise<any[]>} */
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
