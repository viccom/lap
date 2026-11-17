/**
 * Transport seam for the frontend/back-end split (serverization plan §3.3).
 *
 * The rest of the app imports `invoke`/`Channel`/`listen`/`emit` and the media
 * `assetUrl` from here instead of from `@tauri-apps/*`. Which backend those
 * calls reach is chosen once, at build time, from `VITE_LAP_TRANSPORT`:
 *
 *   default (unset) -> Tauri desktop shell   (unchanged behaviour)
 *   VITE_LAP_TRANSPORT=http -> HTTP to lapd  (Web client; endpoints land in phase 3)
 *
 * Exports keep the `@tauri-apps` signatures verbatim, so migrating a file is a
 * one-line import change and no call site moves.
 */
import { resolveTransportMode } from './mode.js';
import { resolveCapabilities } from './capabilities.js';
import { createTauriTransport } from './tauri.js';
import { createHttpTransport } from './http.js';

// Vite replaces `import.meta.env.VITE_LAP_TRANSPORT` with a literal at build
// time, so the mode is fixed per bundle and the branch is decided once at load.
// Node (tests that import this module transitively) has no `import.meta.env`,
// so read it defensively and fall back to the Tauri default.
//
// Note: both factories are imported statically, so the unused one stays in the
// bundle as a few KB of dead code. Correctness does not depend on eliminating
// it — the chosen `impl` is the only one ever called — and a lazy
// `import()` swap is not worth the async plumbing at this stage.
const env = import.meta.env;
const mode = resolveTransportMode(env ? env.VITE_LAP_TRANSPORT : undefined);
const impl = mode === 'http' ? createHttpTransport() : createTauriTransport();

export const transportMode = mode;
export const invoke = impl.invoke;
export const Channel = impl.Channel;
export const listen = impl.listen;
export const emit = impl.emit;
export const assetUrl = impl.assetUrl;

// Auth/signing are HTTP-only concerns (phase 4). The desktop transport has no
// equivalent, so export inert stubs there — callers never invoke them when
// `native` is true, but the names exist so views import them unconditionally.
const noopAuth = async () => { throw new Error('auth is not available in the desktop transport'); };
export const onUnauthorized = impl.onUnauthorized ?? (() => () => {});
export const authStatus = impl.authStatus ?? noopAuth;
export const login = impl.login ?? noopAuth;
export const pair = impl.pair ?? noopAuth;
export const setup = impl.setup ?? noopAuth;
export const pairingCode = impl.pairingCode ?? noopAuth;
export const signedAssetUrl = impl.signedAssetUrl ?? (async (url) => url);
// Server descriptor + device management are HTTP-only concerns (phase 4); the
// desktop transport has no equivalent.
export const serverInfo = impl.serverInfo ?? noopAuth;
export const devicesList = impl.devicesList ?? noopAuth;
export const deviceRevoke = impl.deviceRevoke ?? noopAuth;

/// Desktop-only (C-class) entry points are gated on this (§3.3).
export function capabilities() {
  return resolveCapabilities(mode);
}
