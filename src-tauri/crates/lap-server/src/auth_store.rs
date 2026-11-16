//! `<data>/lapd.db` — server-owned auth storage (batch 3.3 decision).
//!
//! Device tokens and the audit log are service-global state: they must
//! survive library switches and library deletion, so they cannot live in a
//! per-library `user_version` database. This is the concrete form of the
//! plan-doc's `device_tokens` table (originally sketched as migration v29).
//!
//! Single connection, WAL mode; the server is the only writer (same
//! single-writer discipline as the library store).

use std::path::Path;
use std::sync::Mutex;

use rusqlite::Connection;

pub const SCOPE_ADMIN: &str = "admin";
pub const SCOPE_STANDARD: &str = "standard";

#[derive(Debug, Clone)]
pub struct TokenRow {
    pub id: i64,
    pub scope: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenInfo {
    pub id: i64,
    pub device_name: String,
    pub scope: String,
    pub created_at: i64,
    pub last_seen_at: i64,
}

pub struct AuthStore {
    conn: Mutex<Connection>,
}

const SCHEMA_SQL: &str = "
    CREATE TABLE IF NOT EXISTS device_tokens (
        id INTEGER PRIMARY KEY,
        token_hash TEXT NOT NULL UNIQUE,
        device_name TEXT NOT NULL DEFAULT '',
        scope TEXT NOT NULL DEFAULT 'standard',
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE IF NOT EXISTS auth_audit (
        id INTEGER PRIMARY KEY,
        at INTEGER NOT NULL,
        kind TEXT NOT NULL,
        detail TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX IF NOT EXISTS idx_auth_audit_at ON auth_audit(at);
";

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

impl AuthStore {
    pub fn open(data_dir: &Path) -> Result<AuthStore, String> {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create data dir: {e}"))?;
        let path = data_dir.join("lapd.db");
        let conn = Connection::open(&path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| format!("setting WAL on {}: {e}", path.display()))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| format!("setting synchronous on {}: {e}", path.display()))?;

        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| format!("reading user_version: {e}"))?;
        if version < 1 {
            conn.execute_batch(SCHEMA_SQL)
                .map_err(|e| format!("creating lapd.db schema: {e}"))?;
            conn.pragma_update(None, "user_version", 1)
                .map_err(|e| format!("bumping lapd.db user_version: {e}"))?;
        }
        Ok(AuthStore { conn: Mutex::new(conn) })
    }

    /// Insert a token (hash only — the raw token is never stored) and return
    /// its row id.
    pub fn insert_token(
        &self,
        token_hash: &str,
        device_name: &str,
        scope: &str,
    ) -> Result<i64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO device_tokens (token_hash, device_name, scope, created_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![token_hash, device_name, scope, unix_now()],
        )
        .map_err(|e| format!("inserting device token: {e}"))?;
        Ok(conn.last_insert_rowid())
    }

    pub fn find_token(&self, token_hash: &str) -> Result<Option<TokenRow>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT id, scope FROM device_tokens WHERE token_hash = ?1")
            .map_err(|e| e.to_string())?;
        let mut rows = stmt
            .query(rusqlite::params![token_hash])
            .map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some(TokenRow {
                id: row.get(0).map_err(|e| e.to_string())?,
                scope: row.get(1).map_err(|e| e.to_string())?,
            })),
            None => Ok(None),
        }
    }

    pub fn list_tokens(&self) -> Result<Vec<TokenInfo>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, device_name, scope, created_at, last_seen_at
                 FROM device_tokens ORDER BY created_at",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok(TokenInfo {
                    id: row.get(0)?,
                    device_name: row.get(1)?,
                    scope: row.get(2)?,
                    created_at: row.get(3)?,
                    last_seen_at: row.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
    }

    pub fn touch_token(&self, id: i64) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "UPDATE device_tokens SET last_seen_at = ?1 WHERE id = ?2",
                rusqlite::params![unix_now(), id],
            );
        }
    }

    /// Revoke by id; returns whether a row was removed.
    pub fn delete_token(&self, id: i64) -> Result<bool, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute("DELETE FROM device_tokens WHERE id = ?1", rusqlite::params![id])
            .map_err(|e| format!("revoking device token: {e}"))?;
        Ok(removed > 0)
    }

    /// Append an audit row. Audit failures must never break the request
    /// path — callers log-and-continue.
    pub fn audit(&self, kind: &str, detail: &str) {
        if let Ok(conn) = self.conn.lock() {
            let _ = conn.execute(
                "INSERT INTO auth_audit (at, kind, detail) VALUES (?1, ?2, ?3)",
                rusqlite::params![unix_now(), kind, detail],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lapd-auth-store-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn schema_is_created_and_restartable() {
        let dir = scratch("schema");
        let store = AuthStore::open(&dir).unwrap();
        drop(store);
        // Reopening the same file must not fail or re-run the migration.
        let store = AuthStore::open(&dir).unwrap();
        assert!(store.list_tokens().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_round_trip_by_hash() {
        let dir = scratch("tokens");
        let store = AuthStore::open(&dir).unwrap();
        let id = store.insert_token("a".repeat(64).as_str(), "phone", SCOPE_ADMIN).unwrap();
        let found = store.find_token(&"a".repeat(64)).unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.scope, SCOPE_ADMIN);
        assert!(store.find_token(&"b".repeat(64)).unwrap().is_none());
        assert!(store.delete_token(id).unwrap());
        assert!(store.find_token(&"a".repeat(64)).unwrap().is_none());
        assert!(!store.delete_token(id).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_hashes_are_rejected() {
        let dir = scratch("dup");
        let store = AuthStore::open(&dir).unwrap();
        store.insert_token(&"c".repeat(64), "a", SCOPE_STANDARD).unwrap();
        assert!(store.insert_token(&"c".repeat(64), "b", SCOPE_ADMIN).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audit_rows_append_without_breaking_reads() {
        let dir = scratch("audit");
        let store = AuthStore::open(&dir).unwrap();
        store.audit("login_ok", "ip=127.0.0.1");
        store.audit("login_fail", "ip=10.0.0.2");
        // Audit has no reader endpoint yet; the write path must at least not
        // corrupt the token tables.
        assert!(store.list_tokens().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
