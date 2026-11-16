/**
 * HTTP transport — the Web bundle built with `VITE_LAP_TRANSPORT=http`.
 *
 * SKELETON (phase 1). It fixes the request/response shapes the rest of the app
 * will use so both transports are drop-in identical, but the `lapd` endpoints
 * behind them are implemented in phase 3. Until then a `=http` bundle builds
 * and unit-tests against these shapes; it is not yet a working Web app.
 *
 * Endpoint contract (serverization plan §3.3/§3.5):
 *   invoke(name, args)  -> POST /api/cmd/{name}   (JSON args -> JSON result)
 *   listen(name, cb)    -> GET  /api/events       (SSE, name-tagged payloads)
 *   emit(name, payload) -> local bus only; §3.3 keeps same-tab events off the net
 *   assetUrl(kind, ..)  -> /api/thumb|preview/{library_id}/{file_id}
 */
export function createHttpTransport({
  baseUrl = '',
  fetchImpl = globalThis.fetch?.bind(globalThis),
  EventSourceImpl = globalThis.EventSource,
} = {}) {
  // Same-tab events (emit + listen for names nobody served over SSE) run on a
  // local bus, mirroring the Tauri event semantics without a round trip.
  const bus = new EventTarget();

  function invoke(name, args = {}) {
    const url = `${baseUrl}/api/cmd/${name}`;
    return fetchImpl(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(args ?? {}),
    }).then(async (res) => {
      if (!res.ok) {
        throw new Error(`command ${name} failed: HTTP ${res.status}`);
      }
      // Commands may legitimately return no body.
      const text = await res.text();
      return text ? JSON.parse(text) : null;
    });
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

  return {
    mode: 'http',
    invoke,
    Channel,
    listen: (name, callback) => listen(name, callback),
    listenLocal,
    emit,
    capabilities: () => ({ native: false }),
    assetUrl(kind, { libraryId = '', fileId = 0, filePath = '' } = {}) {
      // `asset` (arbitrary file path) has no file_id and no Web endpoint until
      // phase 3; return empty so callers degrade instead of requesting a Tauri
      // asset URL that cannot resolve in a browser.
      if (kind === 'asset') return filePath ? '' : '';
      return `${baseUrl}/api/${kind}/${libraryId}/${fileId}`;
    },
  };
}
