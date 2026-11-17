/**
 * Host-difference seam for window/webview APIs (phase 4, batch 4.7).
 *
 * The desktop shell opens secondary windows through Tauri's `WebviewWindow`;
 * a browser tab has no such thing. This module is the ONE place that decides
 * between the two hosts, so call sites never import `@tauri-apps/*window*`
 * directly and never crash when the Web build runs in a browser.
 *
 * Two rules keep this honest:
 *   1. Importing the Tauri modules is harmless (plain JS); only *calling* the
 *      native bridge throws. So the real APIs are statically imported and used
 *      in desktop mode, while the HTTP bundle gets browser-safe equivalents.
 *   2. Web equivalents are real where a browser can do the job (fullscreen,
 *      focus) and honest `null`/no-ops where it cannot (secondary windows).
 *      Callers are expected to branch on `capabilities().native` for flows that
 *      have no Web meaning; the stub exists so a stray call degrades instead of
 *      exploding.
 *
 * `webWindowTargets` maps a desktop window label to the in-app route the Web
 * client should navigate to, so "open the settings window" becomes "go to
 * /settings" without each caller inventing its own mapping.
 */
import { getCurrentWebviewWindow as tauriWindow, WebviewWindow as TauriWebviewWindow } from '@tauri-apps/api/webviewWindow';
import { resolveTransportMode } from './mode.js';

const mode = resolveTransportMode(
  typeof import.meta !== 'undefined' && import.meta.env
    ? import.meta.env.VITE_LAP_TRANSPORT
    : undefined,
);

/// Desktop window label -> Web route. Labels absent here have no Web analogue
/// (e.g. the transient image viewer, which the caller opens as a route itself).
const webWindowTargets = {
  settings: '/settings',
  imageeditor: '/image-editor',
  montage: '/montage',
  'vl-lab': '/vl-lab',
};

/** A window label that the Web client can reach as a route, or null. */
export function webRouteForWindowLabel(label) {
  return webWindowTargets[label] || null;
}

class BrowserWindowStub {
  label = 'main';
  async show() {}
  async hide() {}
  async close() { window.close(); } // browsers ignore scripted close on non-opener tabs
  async setFocus() { window.focus(); }
  async listen() { return () => {}; }
  async onCloseRequested() { return () => {}; }
  async setSize() {}
  async setMinSize() {}
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
  async isMinimized() { return false; }
  async isFocused() { return Boolean(document.hasFocus()); }
  async setResizable() {}
  async maximize() {}
  async unmaximize() {}
  async isMinimized() { return false; }
  async innerSize() { return { width: window.innerWidth, height: window.innerHeight, toLogical: () => ({ width: window.innerWidth, height: window.innerHeight }) }; }
  async outerPosition() { return { x: 0, y: 0, toLogical: () => ({ x: 0, y: 0 }) }; }
  async scaleFactor() { return window.devicePixelRatio || 1; }
  // Resize/move listeners are desktop window events; a browser tab has none.
  async onResized() { return () => {}; }
  async onMoved() { return () => {}; }
  async onFocusChanged() { return () => {}; }
}

/**
 * Browser stand-in for `WebviewWindow`. `getByLabel` reports "no such window"
 * (the Web client keeps a single tab), and constructing one is a programming
 * error the caller must prevent via `capabilities().native` — it throws loudly
 * rather than silently pretending a window opened.
 */
function createWebWebviewWindow() {
  return {
    getByLabel: async () => null,
    getAll: async () => [],
    // The instance constructor is only reachable from a native-only branch.
    create() {
      throw new Error('WebviewWindow is not available in the Web client; navigate a route instead');
    },
  };
}

// The native module keeps `getByLabel` as a static; expose the same shape so
// call sites do `Window.getByLabel(...)` identically on both hosts.
function nativeWebviewWindow() {
  return {
    getByLabel: (label) => TauriWebviewWindow.getByLabel(label),
    getAll: () => TauriWebviewWindow.getAll(),
    create(label, options) { return new TauriWebviewWindow(label, options); },
  };
}

const webWindowAdapter = createWebWebviewWindow();
const nativeWindowAdapter = nativeWebviewWindow();

let cachedWindow = null;

/** The current window, with a browser-safe stand-in in the Web client. */
export function getCurrentWebviewWindow() {
  if (cachedWindow) return cachedWindow;
  cachedWindow = mode === 'http' ? new BrowserWindowStub() : tauriWindow();
  return cachedWindow;
}

/**
 * The `WebviewWindow` façade. `getByLabel` is async on both hosts; `create`
 * builds a secondary window on desktop and throws on Web (callers gate it).
 */
export function webviewWindow() {
  return mode === 'http' ? webWindowAdapter : nativeWindowAdapter;
}

/** True when the app is running as a Web (non-Tauri) client. */
export function isWebClient() {
  return mode === 'http';
}

/**
 * Leave a secondary page. On the desktop shell "leaving" means closing the
 * window and letting the OS focus the main window again. A browser tab has no
 * scriptable close (`window.close()` is ignored for tabs the script didn't
 * open), so the Web client navigates back to where the user came from, falling
 * back to the grid when there is no history to return to. Every secondary page
 * exits through this one verb so the two hosts stay honest about the difference
 * instead of one of them being a dead end.
 */
export function closeOrBack() {
  if (mode !== 'http') {
    getCurrentWebviewWindow().close();
    return;
  }
  // Dynamic import avoids a static import cycle (router -> auth, not this seam).
  void import('@/common/router').then((mod) => {
    const router = mod.default;
    // vue-router records the previous full path in history state; with nothing
    // behind us (a deep link opened fresh) `back` is absent, so go home instead
    // of silently doing nothing.
    if (router.options.history.state?.back) router.back();
    else router.push('/');
  });
}
