/**
 * Web-safe facade for the `@tauri-apps/api/window` calls the app makes at
 * startup. Importing the Tauri module in a browser is harmless (it is plain
 * JS); only *calling* the native bridge would throw. So the real function is
 * statically imported and used in desktop mode, while the HTTP bundle gets a
 * browser-safe stub with the same tiny surface (phase 4).
 *
 * Covered surface (what main.js/App.vue actually use):
 *   `.label`, `.show()`, `.hide()`, `.close()`, `.listen()`,
 *   `.onCloseRequested()`, `.setSize()`, `.center()`.
 */
import { getCurrentWebviewWindow as tauriWindow } from '@tauri-apps/api/webviewWindow';
import { resolveTransportMode } from './mode.js';

const mode = resolveTransportMode(
  typeof import.meta !== 'undefined' && import.meta.env
    ? import.meta.env.VITE_LAP_TRANSPORT
    : undefined,
);

class BrowserWindowStub {
  label = 'main';
  async show() {}
  async hide() {}
  async close() { window.close(); } // browsers ignore scripted close on non-opener tabs
  async setFocus() { window.focus(); }
  async listen() { return () => {}; }
  async onCloseRequested() { return () => {}; }
  async setSize() {}
  async center() {}
  async minimize() {}
  async toggleMaximize() {}
  async startDragging() {}
  // Fullscreen maps to the browser Fullscreen API (phase 4).
  async isFullscreen() { return Boolean(document.fullscreenElement); }
  async setFullscreen(on) {
    try {
      if (on && !document.fullscreenElement) await document.documentElement.requestFullscreen();
      else if (!on && document.fullscreenElement) await document.exitFullscreen();
    } catch { /* user gesture / policy may refuse */ }
  }
  async isMaximized() { return false; }
  async setResizable() {}
  async maximize() {}
  async unmaximize() {}
  // Resize/move listeners are desktop window events; a browser tab has none.
  async onResized() { return () => {}; }
  async onMoved() { return () => {}; }
  async onFocusChanged() { return () => {}; }
}

let cached = null;

/** Synchronous, like the real Tauri API — works in top-level code. */
export function getCurrentWebviewWindow() {
  if (cached) return cached;
  cached = mode === 'http' ? new BrowserWindowStub() : tauriWindow();
  return cached;
}

/** True when the app is running as a Web (non-Tauri) client. */
export function isWebClient() {
  return mode === 'http';
}
