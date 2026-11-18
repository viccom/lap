/**
 * Whether the desktop in-app updater may run at all (fork update policy).
 *
 * lappp is a fork that deliberately keeps upstream's Tauri identifier
 * (`com.julyx10.lap`) so existing libraries stay readable. The installer keys
 * upgrades on that identifier, which means an update artifact served by
 * upstream julyx10/lap would be installed *over* lappp — replacing the whole
 * application and silently dropping every fork-only change (VL understanding,
 * AI search, the client/server split).
 *
 * Until this fork publishes its own signed feed (own endpoint + own minisign
 * pubkey), the updater must stay off. Flip `UPDATER_FEED_IS_FORK_OWNED` to
 * `true` only together with `plugins.updater.endpoints` / `pubkey` in
 * `src-tauri/tauri.conf.json` pointing at the fork.
 *
 * Pure and dependency-free so it is unit-testable under Node.
 */
export const UPDATER_FEED_IS_FORK_OWNED = false;

/**
 * True when an update check may be performed.
 *
 * `native` false = Web client (lapd-served bundle): it updates by refreshing
 * the page, so it must never reach the desktop updater.
 */
export function resolveUpdaterEnabled({
  native = false,
  feedIsForkOwned = UPDATER_FEED_IS_FORK_OWNED,
} = {}) {
  return Boolean(native) && Boolean(feedIsForkOwned);
}
