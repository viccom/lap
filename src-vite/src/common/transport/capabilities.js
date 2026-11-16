/**
 * Transport capabilities (§3.3 of the serverization plan).
 *
 * `native: false` means the code is running as a Web client against `lapd`,
 * so the desktop-only (C-class) entry points — directory pickers, reveal in
 * file manager, native drag-out, updater, wallpaper, window controls — must be
 * hidden instead of invoked.
 *
 * The answer is primarily the build-time mode, with a runtime fallback: a
 * default (Tauri-mode) bundle opened in a plain browser has no
 * `window.__TAURI_INTERNALS__`, so we report `native: false` rather than let
 * the user hit an entry point that would throw. Under Node (tests, SSR) there
 * is no `window`, so the compile-time answer stands.
 */
export function resolveCapabilities(mode, env = globalThis) {
  if (mode === 'http') return { native: false };
  const win = env?.window;
  if (win && !win.__TAURI_INTERNALS__) return { native: false };
  return { native: true };
}
