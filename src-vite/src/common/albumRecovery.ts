import { emit } from '@/common/transport';
import { invoke } from '@/common/transport';
import { libConfig } from '@/common/config';
import { getAlbumAccessibility, getUnavailableAlbumIds, resetFileAccessibility, setAlbumAccessibility } from '@/common/availability';

type AlbumAccess = { id: number; path: string; is_accessible?: boolean };

// Startup, explicit selection and focus recovery must update the same state.
export async function applyAlbumAccess(
  album: AlbumAccess, available: boolean, libraryId: string,
  { previousAvailable = album.is_accessible, selectionChanged = false } = {},
) {
  if (libraryId !== libConfig._libraryId) return;
  const changed = previousAvailable !== available || (getAlbumAccessibility(album.id) ?? true) !== available;
  setAlbumAccessibility(album.id, available);
  album.is_accessible = available;
  const cleared = available && resetFileAccessibility(album.path);
  if (changed || cleared) {
    await emit('album-accessibility-changed', {
      albumId: album.id, rootPath: album.path, available, libraryId, selectionChanged,
    });
  }
}

// A slow disconnected share must not accumulate probes on repeated focus events.
const pending = new Map<string, Promise<void>>();
export function recheckOfflineAlbums(): Promise<void> {
  const libraryId = libConfig._libraryId;
  const existing = pending.get(libraryId);
  if (existing) return existing;
  const unavailableIds = getUnavailableAlbumIds();
  if (unavailableIds.length === 0) return Promise.resolve();
  const task = (async () => {
    for (const albumId of unavailableIds) {
      if (libraryId !== libConfig._libraryId) return;
      if (getAlbumAccessibility(albumId) !== false) continue;
      try {
        // Publish availability only after the album is ready for UI recovery.
        const available = await invoke<boolean>('check_album_accessibility', { albumId });
        if (libraryId !== libConfig._libraryId) return;
        if (!available) continue;
        const album = await invoke<AlbumAccess>('get_album', { albumId });
        if (libraryId !== libConfig._libraryId) return;
        await applyAlbumAccess(album, true, libraryId, { previousAvailable: false });
        if (libraryId !== libConfig._libraryId) return;
        await emit('albums-refreshed', { albums: [album], refreshFolders: false, libraryId });
      } catch (error) {
        console.error('Failed to recheck offline album:', albumId, error);
      }
    }
  })().finally(() => { pending.delete(libraryId); });
  pending.set(libraryId, task);
  return task;
}
