/**
 * Web authentication state (phase 4). The desktop bundle never uses this —
 * `login()`/`pair()` are inert there. In the HTTP bundle `lapd` issues an
 * HttpOnly `lapd_token` Cookie on login, so "is logged in" is decided once at
 * startup by `GET /api/auth/status` + a probe command, then tracked here.
 *
 * The router guard reads `isAuthenticated`; `http.js` calls `markExpired` on
 * any command 401 so the next navigation bounces back to /login.
 */
import {
  transportMode,
  authStatus,
  login as transportLogin,
  pair as transportPair,
  setup as transportSetup,
  onUnauthorized,
} from '@/common/transport';

const isWeb = transportMode === 'http';

let authenticated = false;
let scope = 'none';
let passwordSet = false;
const listeners = new Set();

function notify() {
  for (const fn of listeners) {
    try { fn(authenticated, scope); } catch { /* ignore a broken listener */ }
  }
}

export function isAuthenticated() {
  return !isWeb || authenticated; // desktop is always "authenticated"
}

export function currentScope() {
  return scope;
}

export function subscribeAuth(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** Probe the session: status says whether a password is configured; a live
 *  token is confirmed by a cheap read command. Only a 401 from the probe means
 *  the Cookie is absent/expired — every other failure (429 rate limit, 5xx, a
 *  transient network error) says nothing about the session and must NOT log the
 *  user out. Conflating the two used to bounce valid sessions to /login. */
export async function checkSession() {
  if (!isWeb) { authenticated = true; scope = 'desktop'; return true; }
  let status;
  try {
    status = await authStatus();
  } catch {
    // /api/auth/status is unauthenticated and cheap; a failure here is a server
    // problem, not proof of a missing session.
    notify();
    return authenticated;
  }
  passwordSet = status?.passwordSet === true;
  if (!passwordSet) {
    // No password configured yet → the setup screen owns the flow.
    authenticated = false; scope = 'none'; notify(); return false;
  }
  // We cannot read the HttpOnly Cookie from JS; probe with a token-protected
  // command. `/api/auth/devices` is admin-only, so use a benign read command
  // that every scope can call via the existing invoke transport.
  const { invoke } = await import('@/common/transport');
  try {
    await invoke('get_app_config', {});
    authenticated = true; notify(); return true;
  } catch (error) {
    if (error?.status === 401) {
      authenticated = false; notify(); return false;
    }
    // Keep the last known state: the app stays usable and a real 401 on a later
    // command still redirects through `markExpired`.
    notify();
    return authenticated;
  }
}

export async function loginWithPassword(password, deviceName = 'web') {
  const cred = await transportLogin(password, deviceName);
  authenticated = true;
  scope = cred?.scope ?? 'standard';
  notify();
  return cred;
}

export async function pairWithCode(code, deviceName = 'web') {
  const cred = await transportPair(code, deviceName);
  authenticated = true;
  scope = cred?.scope ?? 'admin';
  notify();
  return cred;
}

export async function setupPassword(password) {
  const result = await transportSetup(password);
  passwordSet = true;
  return result;
}

export async function markExpired() {
  authenticated = false;
  scope = 'none';
  notify();
}

export function passwordConfigured() {
  return passwordSet;
}

// A command 401 invalidates the session.
onUnauthorized(() => { void markExpired(); });

export const webMode = isWeb;
