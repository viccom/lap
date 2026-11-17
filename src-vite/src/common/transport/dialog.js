/**
 * Host-difference seam for native dialogs (phase 4, batch 4.7).
 *
 * The desktop shell uses Tauri's dialog plugin; a browser has its own, weaker
 * equivalents. This module picks the right one so call sites never import
 * `@tauri-apps/plugin-dialog` directly — importing is harmless (plain JS), but
 * *calling* it in a browser throws and would break the feature that used it.
 *
 * Degradation per dialog, chosen to keep the Web client usable:
 *   ask(...)     -> window.confirm (real behaviour; confirm/cancel both work)
 *   message(...) -> window.alert   (real behaviour)
 *   open/save    -> null           (no browser equivalent for picking a
 *                                   *server-side* path; callers already treat
 *                                   a falsy result as "user cancelled")
 *
 * `open`/`save` returning null is deliberate: the Web client chooses server
 * paths through settings, not through a local file picker (§3.9). Callers must
 * keep their existing "cancelled" handling; none of them may assume a path.
 */
import { open as tauriOpen, save as tauriSave, ask as tauriAsk, message as tauriMessage } from '@tauri-apps/plugin-dialog';
import { resolveTransportMode } from './mode.js';

const mode = resolveTransportMode(
  typeof import.meta !== 'undefined' && import.meta.env
    ? import.meta.env.VITE_LAP_TRANSPORT
    : undefined,
);

const isWeb = mode === 'http';

const webDialog = {
  async open() {
    return null; // no browser equivalent for a server-side file/folder picker
  },
  async save() {
    return null;
  },
  async ask(message, options = {}) {
    const title = options.title ? `${options.title}\n\n` : '';
    return window.confirm(`${title}${message}`);
  },
  async message(message, options = {}) {
    const title = options.title ? `${options.title}\n\n` : '';
    window.alert(`${title}${message}`);
  },
};

const nativeDialog = {
  open: (options) => tauriOpen(options),
  save: (options) => tauriSave(options),
  ask: (message, options) => tauriAsk(message, options),
  message: (message, options) => tauriMessage(message, options),
};

const impl = isWeb ? webDialog : nativeDialog;

/** Open a file/folder picker (native only; null on Web). */
export const open = (options) => impl.open(options);
/** Save-file picker (native only; null on Web). */
export const save = (options) => impl.save(options);
/** Yes/no confirmation — native dialog or window.confirm. */
export const ask = (message, options) => impl.ask(message, options);
/** Informational message — native dialog or window.alert. */
export const message = (message, options) => impl.message(message, options);

/** True when the host has a native file/folder picker. */
export function hasNativeFilePicker() {
  return !isWeb;
}
