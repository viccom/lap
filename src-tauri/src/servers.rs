//! Persisted server list for the thin client (`--features client`).
//!
//! The thin client has no local backend, so it cannot use `lap-core`'s
//! `AppConfig`. It keeps its own tiny JSON file listing the `lapd` servers the
//! user has added, plus which one is current. The file lives in the same
//! per-user data directory as the full app but under its own name, so the two
//! builds never contend for `app-config.json`.
//!
//! This module is intentionally Tauri-free and dependency-light (`serde_json`
//! + `dirs`, both already used by the shell) so it can be unit-tested on its
//! own.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One known `lapd` server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    /// User-visible label (menu entry / list row).
    pub name: String,
    /// Base URL, e.g. `http://192.168.1.9:17860` (no trailing slash).
    pub url: String,
}

/// The whole persisted state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServersConfig {
    #[serde(default)]
    pub servers: Vec<Server>,
    /// URL of the server to connect to on startup, if any.
    #[serde(default)]
    pub current: Option<String>,
}

/// File name inside the app data directory.
pub const FILE_NAME: &str = "lapd-servers.json";

/// Normalize a user-entered URL: trim, add `http://` when no scheme is given,
/// drop any trailing slash so comparisons and joins stay predictable. Blank or
/// scheme-less-garbage input yields `None`.
pub fn normalize_url(raw: &str) -> Option<String> {
    let url = raw.trim();
    if url.is_empty() {
        return None;
    }
    let with_scheme = if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else {
        String::from("http://") + url
    };
    Some(with_scheme.trim_end_matches('/').to_string())
}

impl ServersConfig {
    /// Add a server (idempotent by URL: re-adding the same URL just renames it).
    pub fn add(&mut self, name: &str, url: &str) {
        let name = name.trim();
        let name = if name.is_empty() { url } else { name };
        if let Some(existing) = self.servers.iter_mut().find(|s| s.url == url) {
            existing.name = name.to_string();
        } else {
            self.servers.push(Server { name: name.to_string(), url: url.to_string() });
        }
    }

    /// Remove a server by URL; clears `current` if it pointed there.
    pub fn remove(&mut self, url: &str) {
        self.servers.retain(|s| s.url != url);
        if self.current.as_deref() == Some(url) {
            self.current = None;
        }
    }

    /// Mark a server current. A URL not in the list is ignored (no dangling
    /// pointer), matching "connect only to a known server".
    pub fn set_current(&mut self, url: &str) {
        if self.servers.iter().any(|s| s.url == url) {
            self.current = Some(url.to_string());
        }
    }
}

/// The app data directory for the thin client: mirrors the full app's naming
/// (`com.julyx10.lap`, `.debug` suffix on debug builds) so both builds share a
/// location, but the server list is its own file. `LAP_DATA_DIR` overrides it
/// (mirroring `lapd --data-dir`), so tests and portable runs stay isolated.
pub fn default_config_path() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("LAP_DATA_DIR") {
        if !dir.as_os_str().is_empty() {
            return Ok(PathBuf::from(dir).join(FILE_NAME));
        }
    }
    let dir = dirs::data_local_dir()
        .ok_or_else(|| "Failed to get the local AppData directory".to_string())?
        .join(if cfg!(debug_assertions) { "com.julyx10.lap.debug" } else { "com.julyx10.lap" });
    Ok(dir.join(FILE_NAME))
}

/// Load the config, falling back to an empty one when the file is missing or
/// unreadable. A corrupt file is treated as "no servers" rather than an error:
/// the user can always re-add them, and a hard failure would strand startup.
pub fn load_from(path: &Path) -> ServersConfig {
    let Ok(body) = std::fs::read_to_string(path) else {
        return ServersConfig::default();
    };
    serde_json::from_str(&body).unwrap_or_default()
}

/// Persist the config. Writes to a temp file then renames, so a crash mid-write
/// never leaves a half-written list.
pub fn save_to(path: &Path, config: &ServersConfig) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cannot create config dir: {e}"))?;
    }
    let body = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body).map_err(|e| format!("cannot write config: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("cannot replace config: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "lap-servers-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let _ = tag;
        dir
    }

    #[test]
    fn url_normalization_adds_scheme_and_drops_trailing_slash() {
        assert_eq!(normalize_url("192.168.1.9:17860").as_deref(), Some("http://192.168.1.9:17860"));
        assert_eq!(normalize_url("  http://host:1234/  ").as_deref(), Some("http://host:1234"));
        assert_eq!(normalize_url("https://nas.local").as_deref(), Some("https://nas.local"));
        assert_eq!(normalize_url("   "), None);
        assert_eq!(normalize_url(""), None);
    }

    #[test]
    fn add_is_idempotent_by_url_and_renames() {
        let mut cfg = ServersConfig::default();
        cfg.add("NAS", "http://a:1");
        cfg.add("NAS again", "http://a:1");
        assert_eq!(cfg.servers.len(), 1);
        assert_eq!(cfg.servers[0].name, "NAS again");
        cfg.add("", "http://b:2"); // blank name falls back to the URL
        assert_eq!(cfg.servers.len(), 2);
        assert_eq!(cfg.servers[1].name, "http://b:2");
    }

    #[test]
    fn remove_clears_current_when_it_pointed_there() {
        let mut cfg = ServersConfig::default();
        cfg.add("A", "http://a:1");
        cfg.add("B", "http://b:2");
        cfg.set_current("http://a:1");
        cfg.remove("http://a:1");
        assert_eq!(cfg.current, None);
        assert_eq!(cfg.servers.len(), 1);
    }

    #[test]
    fn set_current_ignores_unknown_url() {
        let mut cfg = ServersConfig::default();
        cfg.add("A", "http://a:1");
        cfg.set_current("http://nope:9");
        assert_eq!(cfg.current, None);
        cfg.set_current("http://a:1");
        assert_eq!(cfg.current.as_deref(), Some("http://a:1"));
    }

    #[test]
    fn round_trips_through_disk_and_recovers_from_corruption() {
        let dir = scratch("round");
        let path = dir.join(FILE_NAME);
        let mut cfg = ServersConfig::default();
        cfg.add("A", "http://a:1");
        cfg.set_current("http://a:1");
        save_to(&path, &cfg).unwrap();
        assert_eq!(load_from(&path), cfg);
        // Missing file → empty default.
        assert_eq!(load_from(&dir.join("nope.json")), ServersConfig::default());
        // Corrupt file → empty default, no panic.
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), ServersConfig::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
