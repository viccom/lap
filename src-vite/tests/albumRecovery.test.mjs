import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { reactive } from 'vue';

const read = path => fs.readFileSync(new URL(path, import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const compile = source => ts.transpile(source.replace(/^import .*;\n/gm, '').replace(/export /g, ''), { target: ts.ScriptTarget.ES2022 });
function fixture() {
  const libConfig = { _libraryId: 'smb-test' };
  const access = new Function('reactive', 'libConfig', compile(read('../src/common/availability.ts')) +
    ';return {getAlbumAccessibility,getUnavailableAlbumIds,setAlbumAccessibility,setFileAccessibility,setFolderAccessibility,resetFileAccessibility,isOriginalUnavailable};')(reactive, libConfig);
  const calls = { reads: 0, checks: [], events: [], emptyChecks: 0 };
  const albums = [{ id: 1, path: '/Volumes/photos', is_accessible: false }, { id: 2, path: '/local', is_accessible: true }];
  const options = { read: async id => albums.find(album => album.id === id), probe: async () => true };
  let onRefreshed;
  const deps = { libConfig, ...access,
    invoke: async (command, args) => {
      if (command === 'check_album_accessibility') {
        calls.checks.push(args.albumId);
        return options.probe(args.albumId);
      }
      assert.equal(command, 'get_album');
      assert.ok(calls.checks.includes(args.albumId));
      calls.reads++;
      return options.read(args.albumId);
    },
    emit: async (name, payload) => {
      calls.events.push({ name, payload });
      if (name === 'albums-refreshed') await onRefreshed({ payload });
    },
    console: { error() {} },
  };
  const recovery = new Function(...Object.keys(deps), compile(read('../src/common/albumRecovery.ts')) +
    '; return {applyAlbumAccess,recheckOfflineAlbums};')(...Object.values(deps));
  const home = read('../src/views/Home.vue');
  const start = home.indexOf('async (event: any) => {', home.indexOf("unlistenAlbumsRefreshed = await listen('albums-refreshed'"));
  const body = home.slice(start, home.indexOf('\n  });', start));
  onRefreshed = new Function('libConfig', 'applyAlbumAccess', 'checkLibraryEmpty',
    ts.transpile('const handler = ' + body, { target: ts.ScriptTarget.ES2022 }) + ';return handler;')(
    libConfig, recovery.applyAlbumAccess, async () => calls.emptyChecks++);
  return { ...access, ...recovery, libConfig, calls, albums, options, onRefreshed };
}
const file = { id: 42, album_id: 1, album_accessible: false, file_path: '/Volumes/photos/sub/image.jpg' };

test('startup recovery clears stale album, folder and file state and notifies preview windows', async () => {
  const f = fixture();
  f.setAlbumAccessibility(1, false);
  f.setFolderAccessibility('/Volumes/photos/sub', false);
  f.setFileAccessibility(file.file_path, false);
  f.setAlbumAccessibility(2, false);
  await f.onRefreshed({ payload: { libraryId: f.libConfig._libraryId, refreshFolders: false,
    albums: [{ ...f.albums[0], is_accessible: true }] } });
  assert.equal(f.isOriginalUnavailable(file), false);
  assert.equal(f.getAlbumAccessibility(2), false);
  assert.equal(f.calls.events.length, 1);
  assert.equal(f.calls.events[0].name, 'album-accessibility-changed');
  assert.equal(f.calls.events[0].payload.rootPath, '/Volumes/photos');
  assert.equal(f.calls.events[0].payload.selectionChanged, false);
  await f.onRefreshed({ payload: { libraryId: 'different-library', albums: [f.albums[0]] } });
  assert.equal(f.calls.events.length, 1);
});

test('normal online focus does no catalog query or filesystem probe', async () => {
  const f = fixture();
  f.setAlbumAccessibility(1, true);
  await f.recheckOfflineAlbums();
  assert.equal(f.calls.reads, 0);
  assert.deepEqual(f.calls.checks, []);
  assert.deepEqual(f.calls.events, []);
});

test('focus recovery only probes offline roots and emits one content recovery event', async () => {
  const f = fixture();
  f.setAlbumAccessibility(1, false); f.setAlbumAccessibility(2, true);
  f.setFolderAccessibility('/Volumes/photos/sub', false);
  await f.recheckOfflineAlbums();
  assert.deepEqual(f.calls.checks, [1]);
  assert.equal(f.isOriginalUnavailable(file), false);
  assert.deepEqual(f.calls.events.map(event => event.name), ['album-accessibility-changed', 'albums-refreshed']);
  await f.recheckOfflineAlbums();
  assert.equal(f.calls.reads, 1);
});

test('repeated focus shares one slow SMB probe and a failed probe can be retried', async () => {
  const f = fixture(); f.setAlbumAccessibility(1, false);
  let resolveProbe;
  f.options.probe = () => new Promise(resolve => { resolveProbe = resolve; });
  const first = f.recheckOfflineAlbums();
  assert.equal(f.recheckOfflineAlbums(), first);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(f.calls.checks, [1]);
  resolveProbe(false); await first;
  assert.equal(f.calls.reads, 0, 'unavailable roots must not trigger catalog reads or cover repair');
  assert.equal(f.getAlbumAccessibility(1), false);
  assert.deepEqual(f.calls.events, []);
  f.options.probe = async () => true;
  await f.recheckOfflineAlbums();
  assert.deepEqual(f.calls.checks, [1, 1]);
  assert.equal(f.getAlbumAccessibility(1), true);
});

test('switching libraries while a probe is pending discards its recovery result', async () => {
  const f = fixture(); f.setAlbumAccessibility(1, false);
  let resolveProbe;
  f.options.probe = () => new Promise(resolve => { resolveProbe = resolve; });
  const pending = f.recheckOfflineAlbums();
  await new Promise(resolve => setImmediate(resolve));
  f.libConfig._libraryId = 'new-library'; f.setAlbumAccessibility(1, false);
  resolveProbe(true); await pending;
  assert.equal(f.getAlbumAccessibility(1), false);
  assert.deepEqual(f.calls.events, []);
});

test('explicit selection uses the same recovery state without a duplicate content refresh', async () => {
  const f = fixture(); f.setAlbumAccessibility(1, true);
  await f.applyAlbumAccess(f.albums[0], true, f.libConfig._libraryId,
    { previousAvailable: false, selectionChanged: true });
  assert.equal(f.calls.events.length, 1);
  assert.equal(f.calls.events[0].payload.selectionChanged, true);
  await f.applyAlbumAccess(f.albums[0], true, f.libConfig._libraryId);
  assert.equal(f.calls.events.length, 1);
});


test('a slow probe from the previous library does not block recovery in the new library', async () => {
  const f = fixture(); f.setAlbumAccessibility(1, false);
  const resolvers = new Map();
  f.options.probe = () => new Promise(resolve => resolvers.set(f.libConfig._libraryId, resolve));
  const old = f.recheckOfflineAlbums();
  await new Promise(resolve => setImmediate(resolve));
  f.libConfig._libraryId = 'new-library'; f.setAlbumAccessibility(1, false);
  const current = f.recheckOfflineAlbums();
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(f.calls.checks, [1, 1]);
  resolvers.get('new-library')(true); await current;
  assert.equal(f.getAlbumAccessibility(1), true);
  resolvers.get('smb-test')(true); await old;
  assert.ok(f.calls.events.every(event => event.payload.libraryId === 'new-library'));
});


test('healthy startup registers access state without repeatedly refreshing content', async () => {
  const f = fixture();
  await f.onRefreshed({ payload: { libraryId: f.libConfig._libraryId, refreshFolders: false,
    albums: f.albums.map(album => ({ ...album, is_accessible: true })) } });
  assert.equal(f.getAlbumAccessibility(1), true);
  assert.equal(f.getAlbumAccessibility(2), true);
  assert.deepEqual(f.calls.events, []);
});

test('a failed album read remains retryable without blocking other albums', async () => {
  const f = fixture();
  f.setAlbumAccessibility(1, false); f.setAlbumAccessibility(2, false);
  f.setFileAccessibility(file.file_path, false);
  f.options.read = async id => {
    if (id === 1) throw new Error('temporary database error');
    return f.albums.find(album => album.id === id);
  };
  await f.recheckOfflineAlbums();
  assert.equal(f.getAlbumAccessibility(1), false);
  assert.equal(f.isOriginalUnavailable(file), true);
  assert.equal(f.getAlbumAccessibility(2), true);
  f.options.read = async id => f.albums.find(album => album.id === id);
  await f.recheckOfflineAlbums();
  assert.deepEqual(f.calls.checks, [1, 2, 1]);
  assert.equal(f.isOriginalUnavailable(file), false);
});
