import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import ts from 'typescript';
import { ref, computed } from 'vue';

// Upstream #555 reworked the folder-deletion flow: the failed-trash dialog no
// longer offers "permanent delete" or "retry" (its Ok/Cancel both just close
// it), and whether the initial request is a permanent delete is decided by
// config.settings.fileDeleteBehavior at startDeleteFolder time. These tests
// pin the CURRENT contract:
//   startDeleteFolder -> (guard, capture, identity, dialog)
//   clickTrashFolder  -> deleteFolder | deleteFolderPermanently by flag,
//                        failure dialog on error, splice on success.

const source = fs.readFileSync(new URL('../src/components/AlbumFolder.vue', import.meta.url), 'utf8');
const body = ts.transpile(
  source.slice(source.indexOf('async function startRenameFolder('), source.indexOf('// more menuitems')) +
  source.slice(source.indexOf('const clickTrashFolder ='), source.indexOf('/// toggle whether folder')),
  { target: ts.ScriptTarget.ES2022 },
);

function fixture() {
  const calls = [];
  const deps = {
    pendingFolderDelete: ref(null), folderTrashFailure: ref(null), isDeletingFolder: ref(false), isRenamingFolder: ref(false),
    isCreatingFolder: ref(false), disposed: false, props: { rootPath: '/album', albumId: 1 },
    libConfig: { _libraryId: 'one' }, deletePermanently: ref(false),
    selection: { folderPath: ref(''), folderId: ref(0), expandAndSelectFolder: async () => {} },
    showTrashFolderMsgbox: ref(false), showTrashFailedFolderMsgbox: ref(false),
    isDirectoryAccessible: async () => true, prepareFolderDelete: async () => 'identity-one',
    clickFolder: async (_, folder) => { deps.selection.folderPath.value = folder.path; },
    deleteFolder: async (...args) => { calls.push(['trash', ...args]); return { deletedCount: 0, error: 'Recycle Bin unavailable' }; },
    deleteFolderPermanently: async (...args) => { calls.push(['permanent', ...args]); return 1; },
    toast: { error() {}, success() {} }, t: key => key, console: { error() {} },
    originalFolderName: ref(''), uiStore: { pushInputHandler() {} },
    nextTick: async () => {}, folderInputRef: ref(null),
    config: { settings: { fileDeleteBehavior: 'trash' } },
    tauriEmit: async () => {}, getAllAlbums: async () => [], recountAlbum: async () => {},
    getFolderByPath: () => null,
  };
  const run = () => new Function(...Object.keys(deps), body +
    '; return { startDeleteFolder, startRenameFolder, clickTrashFolder };')(...Object.values(deps));
  return { deps, calls, run };
}
const folder = { path: '/album/child', name: 'child' };

test('opening deletion captures its own target without any prior rename', async () => {
  const f = fixture(); const actions = f.run();
  await actions.startDeleteFolder(folder);
  assert.equal(f.deps.showTrashFolderMsgbox.value, true);
  assert.deepEqual(f.deps.pendingFolderDelete.value, {
    path: folder.path, name: folder.name, libraryId: 'one', identity: 'identity-one',
  });
  f.deps.selection.folderPath.value = '/album';
  await actions.clickTrashFolder();
  assert.deepEqual(f.calls, [['trash', folder.path, 'one', 'identity-one']]);
  assert.equal(f.deps.showTrashFailedFolderMsgbox.value, true);
});

test('a prior rename cannot supply the deletion target', async () => {
  const f = fixture(); const actions = f.run();
  await actions.startRenameFolder({ path: '/album/other', name: 'other' });
  assert.equal(f.deps.pendingFolderDelete.value, null);
  f.deps.isRenamingFolder.value = false;
  await actions.startDeleteFolder(folder);
  await actions.clickTrashFolder();
  assert.deepEqual(f.calls, [['trash', folder.path, 'one', 'identity-one']]);
});

test('cancelling the confirmation makes no filesystem request', async () => {
  const f = fixture();
  await f.run().startDeleteFolder(folder);
  f.deps.showTrashFolderMsgbox.value = false;
  assert.deepEqual(f.calls, []);
});

test('duplicate confirmation cannot submit a second request', async () => {
  const f = fixture(); let finish;
  f.deps.deleteFolder = async (...args) => {
    f.calls.push(args);
    return new Promise(resolve => { finish = resolve; });
  };
  const actions = f.run(); await actions.startDeleteFolder(folder);
  const first = actions.clickTrashFolder();
  await actions.clickTrashFolder();
  assert.equal(f.calls.length, 1);
  finish({ deletedCount: 0, error: 'Unknown failure' }); await first;
  assert.equal(f.deps.isDeletingFolder.value, false);
});

test('a switched library rejects the pending target', async () => {
  const f = fixture(); const actions = f.run();
  await actions.startDeleteFolder(folder);
  f.deps.libConfig._libraryId = 'two';
  await actions.clickTrashFolder();
  assert.deepEqual(f.calls, []);
});

test('backend errors release the busy state and show the failure dialog', async () => {
  const f = fixture(); f.deps.deleteFolder = async () => { throw Error('Rejected'); };
  const actions = f.run(); await actions.startDeleteFolder(folder);
  await actions.clickTrashFolder();
  assert.equal(f.deps.isDeletingFolder.value, false);
  assert.equal(f.deps.showTrashFailedFolderMsgbox.value, true);
  // The failure dialog carries the error (String(error) - "Error: Rejected");
  // permanent delete is decided at startDeleteFolder time, not offered as a
  // failure fallback.
  assert.equal(f.deps.folderTrashFailure.value.error, 'Error: Rejected');
});

test('successful deletion updates the captured folder and leaves siblings intact', async () => {
  const f = fixture();
  const target = { ...folder };
  const sibling = { path: '/album/sibling', name: 'sibling' };
  f.deps.props.children = [target, sibling];
  f.deps.selection.folderId = ref(2);
  f.deps.getFolderByPath = () => ({ id: 1 });
  f.deps.deleteFolder = async (...args) => { f.calls.push(args); return { deletedCount: 1, error: null }; };
  const actions = f.run();
  await actions.startDeleteFolder(target);
  f.deps.selection.folderPath.value = sibling.path;
  await actions.clickTrashFolder();
  assert.deepEqual(f.calls, [[folder.path, 'one', 'identity-one']]);
  assert.deepEqual(f.deps.props.children, [sibling]);
  assert.equal(f.deps.selection.folderPath.value, '/album');
  assert.equal(f.deps.showTrashFolderMsgbox.value, false);
  assert.equal(f.deps.isDeletingFolder.value, false);
});

for (const error of ['Permission denied', 'File is in use', 'Network disconnected', 'Path not found', 'Uncertain partial result']) {
  test(`${error} is reported in the failure dialog`, async () => {
    const f = fixture();
    f.deps.deleteFolder = async () => ({ deletedCount: 0, error });
    const actions = f.run();
    await actions.startDeleteFolder(folder);
    await actions.clickTrashFolder();
    assert.equal(f.deps.showTrashFailedFolderMsgbox.value, true);
    assert.equal(f.deps.folderTrashFailure.value.error, error);
  });
}

test('the delete behavior config routes the confirmation to the permanent pipeline', async () => {
  const f = fixture();
  f.deps.config.settings.fileDeleteBehavior = 'permanent';
  const actions = f.run();
  await actions.startDeleteFolder(folder);
  assert.equal(f.deps.deletePermanently.value, true);
  await actions.clickTrashFolder();
  assert.deepEqual(f.calls, [['permanent', folder.path, 'one', 'identity-one']]);
  assert.equal(f.deps.isDeletingFolder.value, false);
});

test('failure to identify the directory prevents opening a destructive confirmation', async () => {
  const f = fixture();
  f.deps.prepareFolderDelete = async () => { throw Error('Cannot identify target'); };
  await f.run().startDeleteFolder(folder);
  assert.equal(f.deps.pendingFolderDelete.value, null);
  assert.equal(f.deps.showTrashFolderMsgbox.value, false);
  assert.deepEqual(f.calls, []);
});

test('library switching during identity preparation discards the prepared target', async () => {
  const f = fixture();
  f.deps.prepareFolderDelete = async () => { f.deps.libConfig._libraryId = 'two'; return 'identity-one'; };
  await f.run().startDeleteFolder(folder);
  assert.equal(f.deps.pendingFolderDelete.value, null);
  assert.equal(f.deps.showTrashFolderMsgbox.value, false);
});

test('partial-operation warning is shown only when a trash attempt may have partially completed', () => {
  const f = fixture();
  f.deps.pendingFolderDelete.value = { ...folder, libraryId: 'one', identity: 'identity-one' };
  f.deps.folderTrashFailure.value = { error: 'Unknown', mayHavePartiallyCompleted: true };
  const messageSource = source.slice(source.indexOf('const folderTrashFailureMessage ='), source.indexOf('const folderContextMenus ='));
  const deps = { ...f.deps, computed, t: key => key };
  const message = new Function(...Object.keys(deps), ts.transpile(messageSource) + ';return folderTrashFailureMessage;')(...Object.values(deps));
  assert.match(message.value, /folder_partial_warning/);
  f.deps.folderTrashFailure.value.mayHavePartiallyCompleted = false;
  assert.doesNotMatch(message.value, /folder_partial_warning/);
});
