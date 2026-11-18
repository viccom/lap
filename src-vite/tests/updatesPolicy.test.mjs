// Fork update-policy tests.
//
// lappp deliberately keeps upstream's Tauri identifier (`com.julyx10.lap`) so
// existing libraries stay readable, but the installer keys upgrades on that
// same identifier. An update served by upstream would therefore be installed
// *over* lappp and silently discard every fork-only change. Until this fork
// owns both the feed endpoint and the signing key, in-app updates must be off.
import test from 'node:test';
import assert from 'node:assert/strict';

import { UPDATER_FEED_IS_FORK_OWNED, resolveUpdaterEnabled } from '../src/common/updatesPolicy.js';

test('the shipped fork does not own an update feed yet', () => {
  assert.equal(UPDATER_FEED_IS_FORK_OWNED, false);
});

test('updates stay disabled on the desktop shell while the feed is upstream-owned', () => {
  // native desktop, but feed is upstream -> must not check/install
  assert.equal(resolveUpdaterEnabled({ native: true, feedIsForkOwned: false }), false);
});

test('the Web client never runs the desktop updater', () => {
  assert.equal(resolveUpdaterEnabled({ native: false, feedIsForkOwned: true }), false);
});

test('updates become available only once the feed is fork-owned', () => {
  assert.equal(resolveUpdaterEnabled({ native: true, feedIsForkOwned: true }), true);
});

test('a missing native flag is treated as not-native (Web-safe default)', () => {
  assert.equal(resolveUpdaterEnabled({ feedIsForkOwned: true }), false);
});
