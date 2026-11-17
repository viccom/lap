/**
 * HTTP transport — the Web bundle built with `VITE_LAP_TRANSPORT=http`.
 *
 * The desktop (`tauri.js`) talks to the Rust shell over IPC; this transport
 * talks to `lapd` over the same-origin HTTP API (phase 3) so the browser (and
 * PWA) can run the very same Vue components. Phase 4 completes the skeleton:
 *   invoke(name, args)  -> POST /api/cmd/{name}   (JSON args -> JSON result)
 *   listen(name, cb)    -> GET  /api/events       (SSE, name-tagged payloads)
 *   emit(name, payload) -> local bus only; same-tab events never hit the net
 *   assetUrl(kind, ..)  -> /api/thumb|preview/{library_id}/{file_id}
 *
 * Authentication: `lapd` sets an HttpOnly `lapd_token` Cookie on login; same-
 * origin `fetch`/EventSource carry it automatically. A 401 from a command
 * means the session expired — callers can register a handler via
 * `onUnauthorized`, and `login`/`pair` are exposed for the Login view.
 *
 * Media signatures: every media URL needs `?exp=&sig=&dev=`. `signedAssetUrl`
 * mints them through `POST /api/media/sign`, batches up to 50, caches by raw
 * URL, and refreshes before the 10-minute TTL elapses. `assetUrl` returns the
 * raw path for backward compatibility; the media helper calls the signed one.
 */
export function createHttpTransport({
  baseUrl = '',
  fetchImpl = globalThis.fetch?.bind(globalThis),
  EventSourceImpl = globalThis.EventSource,
} = {}) {
  // Same-tab events (emit + listen for names nobody serves over SSE) run on a
  // local bus, mirroring the Tauri event semantics without a round trip.
  const bus = new EventTarget();
  const unauthorizedHandlers = new Set();

  function onUnauthorized(handler) {
    unauthorizedHandlers.add(handler);
    return () => unauthorizedHandlers.delete(handler);
  }

  function signalUnauthorized() {
    for (const handler of unauthorizedHandlers) {
      try { handler(); } catch { /* a broken handler must not break the command */ }
    }
  }

  async function invoke(name, args = {}) {
    const url = `${baseUrl}/api/cmd/${name}`;
    const res = await fetchImpl(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(args ?? {}),
    });
    if (res.status === 401) {
      signalUnauthorized();
      throw commandError(name, res.status);
    }
    if (!res.ok) {
      throw commandError(name, res.status);
    }
    // Commands may legitimately return no body.
    const text = await res.text();
    return text ? JSON.parse(text) : null;
  }

  // The HTTP status travels with the error: callers must be able to tell "the
  // session is gone (401)" from "the request failed for another reason (429,
  // 5xx, network)" — treating the latter as a logout bounced valid sessions to
  // /login whenever the rate limiter kicked in.
  function commandError(name, status) {
    const error = new Error(`command ${name} failed: HTTP ${status}`);
    error.status = status;
    return error;
  }

  // One shared EventSource fanned out to per-name subscribers, opened lazily.
  const subscribers = new Map(); // name -> Set<callback>
  let source = null;

  function ensureSource() {
    if (source || !EventSourceImpl) return;
    source = new EventSourceImpl(`${baseUrl}/api/events`);
    source.onmessage = (msg) => {
      let envelope;
      try {
        envelope = JSON.parse(msg.data);
      } catch {
        return;
      }
      const handlers = subscribers.get(envelope?.event);
      if (!handlers) return;
      const event = { event: envelope.event, id: envelope.id, payload: envelope.payload };
      for (const cb of handlers) cb(event);
    };
  }

  function listen(name, callback) {
    const handlers = subscribers.get(name) ?? new Set();
    handlers.add(callback);
    subscribers.set(name, handlers);
    ensureSource();
    // Also observe the local bus so a same-tab emit reaches this listener.
    const unlistenLocal = listenLocal(name, callback);
    return unlistenLocal.then((offLocal) => () => {
      offLocal();
      const set = subscribers.get(name);
      if (!set) return;
      set.delete(callback);
      if (set.size === 0) subscribers.delete(name);
    });
  }

  function emit(name, payload) {
    // Local fan-out so a component that listens for its own emit still works.
    const event = new Event(name);
    event.payload = payload;
    bus.dispatchEvent(event);
  }

  function listenLocal(name, callback) {
    const handler = (event) => callback({ event: name, payload: event.payload });
    bus.addEventListener(name, handler);
    return Promise.resolve(() => bus.removeEventListener(name, handler));
  }

  /// Placeholder Channel: the two Channel users (thumbnail cache migration,
  /// native drag-out) are desktop paths a Web client never takes.
  class Channel {
    onmessage = null;
    send() {
      throw new Error('Channel is not available over the HTTP transport');
    }
  }

  // -- Authentication (phase 4) -------------------------------------------
  async function authRequest(path, body) {
    const res = await fetchImpl(`${baseUrl}/api/auth/${path}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body ?? {}),
    });
    const text = await res.text();
    let parsed = null;
    try { parsed = text ? JSON.parse(text) : null; } catch { parsed = text; }
    if (!res.ok) throw Object.assign(new Error(typeof parsed === 'string' ? parsed : `HTTP ${res.status}`), { status: res.status, body: parsed });
    return parsed;
  }

  async function authStatus() {
    const res = await fetchImpl(`${baseUrl}/api/auth/status`);
    // A failed status read is a server-side problem, not "no password set":
    // reporting `passwordSet: false` here would send the app to the setup
    // screen (and, before the fix, log a valid session out).
    if (!res.ok) {
      const error = new Error(`auth status failed: HTTP ${res.status}`);
      error.status = res.status;
      throw error;
    }
    const text = await res.text();
    return text ? JSON.parse(text) : { auth: false, passwordSet: false };
  }

  const login = (password, deviceName = 'web') =>
    authRequest('login', { password, deviceName });
  const pair = (code, deviceName = 'web') =>
    authRequest('pair', { code, deviceName });
  const setup = (password) =>
    authRequest('setup', { password });
  const pairingCode = () =>
    authRequest('pairing-code', {});

  // -- Signed media URLs (phase 4) ----------------------------------------
  // The signature binds path+query+device; mint in batches of ≤50 and refresh
  // before the 600s TTL. Raw->signed map keeps callers' URLs unchanged.
  const SIGN_TTL_MS = 9 * 60 * 1000; // mint at 9min, before the 10min expiry
  const SIGN_BATCH = 50; // server cap per /api/media/sign request
  const signedCache = new Map(); // rawUrl -> { url, expiresAt }
  // Microtask-batched queue: every signedAssetUrl call in the same tick is
  // collected and minted together (a grid mounts dozens of thumbnails at once).
  let pendingRaw = new Set();
  let flushPromise = null;

  async function signBatch(rawUrls) {
    const res = await fetchImpl(`${baseUrl}/api/media/sign`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ urls: rawUrls }),
    });
    if (res.status === 401) {
      signalUnauthorized();
      throw new Error('media signing failed: HTTP 401');
    }
    if (!res.ok) throw new Error(`media signing failed: HTTP ${res.status}`);
    const body = await res.json();
    const urls = body?.urls;
    if (!Array.isArray(urls) || urls.length !== rawUrls.length) {
      throw new Error('media signing returned an unexpected response');
    }
    const now = Date.now();
    rawUrls.forEach((raw, i) => {
      signedCache.set(raw, { url: urls[i], expiresAt: now + SIGN_TTL_MS });
    });
  }

  async function flushSignQueue() {
    while (pendingRaw.size) {
      const batch = [...pendingRaw].slice(0, SIGN_BATCH);
      batch.forEach((u) => pendingRaw.delete(u));
      try {
        await signBatch(batch);
      } catch {
        // Failed URLs stay uncached; a later call retries them.
      }
    }
  }

  function scheduleFlush() {
    if (!flushPromise) {
      // Defer one microtask so all same-tick callers join the first batch.
      flushPromise = Promise.resolve().then(flushSignQueue).finally(() => {
        flushPromise = null;
        // URLs added while the last batch was in flight need another pass.
        if (pendingRaw.size) scheduleFlush();
      });
    }
    return flushPromise;
  }

  async function signedAssetUrl(raw) {
    if (!raw) return '';
    const cached = signedCache.get(raw);
    if (cached && cached.expiresAt > Date.now()) return cached.url;
    pendingRaw.add(raw);
    // scheduleFlush returns a promise covering all batches queued up to and
    // including this URL; once it resolves the cache has our entry (or the
    // request failed, in which case we return '' and the img error path shows).
    await scheduleFlush();
    return signedCache.get(raw)?.url ?? '';
  }

  return {
    mode: 'http',
    invoke,
    Channel,
    listen: (name, callback) => listen(name, callback),
    listenLocal,
    emit,
    onUnauthorized,
    authStatus,
    login,
    pair,
    setup,
    pairingCode,
    signedAssetUrl,
    capabilities: () => ({ native: false }),
    assetUrl(kind, { libraryId = '', fileId = 0, filePath = '' } = {}) {
      // `asset` (arbitrary file path) has no Web endpoint; the media helper
      // maps original/video by file_id. Keep the raw path shape here.
      if (kind === 'asset') return filePath ? '' : '';
      return `${baseUrl}/api/${kind}/${libraryId}/${fileId}`;
    },
  };
}
