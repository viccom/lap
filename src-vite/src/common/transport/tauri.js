/**
 * Tauri transport — the default, used by the desktop bundle.
 *
 * A thin re-export of the `@tauri-apps` primitives the rest of the app already
 * used, so migrating a file is a one-line import change and call sites stay put.
 * The primitives are injected (defaulting to the real ones) for the same reason
 * `common/vlApi.js` injects `invoker`: tests can observe the calls under Node.
 */
import { invoke, Channel, convertFileSrc } from '@tauri-apps/api/core';
import { listen, emit } from '@tauri-apps/api/event';

export function createTauriTransport({
  invoker = invoke,
  ChannelImpl = Channel,
  listenImpl = listen,
  emitImpl = emit,
  convertFileSrcImpl = convertFileSrc,
} = {}) {
  return {
    mode: 'tauri',
    invoke: invoker,
    Channel: ChannelImpl,
    listen: listenImpl,
    emit: emitImpl,
    capabilities: () => ({ native: true }),
    /// Media base URL without a query string; callers append params.
    /// `win` decides the custom scheme (`http://*.localhost` only works on
    /// Windows' webview) — passed in rather than re-deriving the platform here
    /// so `utils.ts` stays the single source of truth.
    assetUrl(kind, { libraryId = '', fileId = 0, filePath = '', win = false } = {}) {
      if (kind === 'asset') {
        return filePath ? convertFileSrcImpl(filePath) : '';
      }
      const scheme = win ? `http://${kind}.localhost` : `${kind}://localhost`;
      return `${scheme}/${libraryId}/${fileId}`;
    },
  };
}
