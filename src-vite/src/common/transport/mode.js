/**
 * Which backend transport the app talks to.
 *
 * The mode is a build-time choice (see §3.3 of the serverization plan): the
 * desktop bundle keeps the default Tauri transport, and a Web bundle opts into
 * the HTTP transport with `VITE_LAP_TRANSPORT=http`. Anything other than the
 * explicit `http` opt-in stays on Tauri, so an unset or misspelled flag never
 * silently drops the app off the desktop transport.
 *
 * Pure and dependency-free so it can be unit-tested under Node.
 */
export function resolveTransportMode(flag) {
  return flag === 'http' ? 'http' : 'tauri';
}
