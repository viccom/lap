/**
 * Host-difference seam for OS-level queries (phase 4, batch 4.7).
 *
 * `plugin-os` only exists in the Tauri shell. The Web client answers from
 * browser APIs. Keeping this in one place means `api.js` never imports the
 * plugin directly, so a browser build cannot break on it.
 */
import { locale as tauriLocale } from '@tauri-apps/plugin-os';
import { resolveTransportMode } from './mode.js';

const mode = resolveTransportMode(
  typeof import.meta !== 'undefined' && import.meta.env
    ? import.meta.env.VITE_LAP_TRANSPORT
    : undefined,
);

/** The display locale: the OS preference on desktop, the browser's on Web. */
export function osLocale() {
  if (mode === 'http') {
    return Promise.resolve(
      (typeof navigator !== 'undefined' && navigator.language) || 'en',
    );
  }
  return tauriLocale();
}
