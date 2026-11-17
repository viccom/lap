//! Desktop "connect to server" client mode (plan §3.7/§5).
//!
//! The desktop normally embeds the backend: it opens the library SQLite and
//! runs the whole core in-process. When a `lapd` server already owns the data
//! directory — its `<data>/lapd.lock` names a live pid that is not us — starting
//! the embedded backend would make two processes race for one library (risk
//! R2, plan §6). Client mode instead points the main window at the server's own
//! Web UI and opens no local database.
//!
//! This module is desktop-only glue (argument parsing + lock inspection); it
//! lives in the shell, not lap-core, because the core must stay Tauri-free and
//! has no notion of "client vs embedded".
//!
//! Detection order (explicit beats implicit):
//!   1. `--connect <url>` / `--connect=<url>` on the command line
//!   2. `LAP_CONNECT` environment variable
//!   3. a live foreign owner in `<data>/lapd.lock` → `http://127.0.0.1:<port>`
//!   4. otherwise: embedded mode (unchanged default)

use std::path::Path;

/// Read a `--name value` / `--name=value` flag from the argument list. Used by
/// both `--connect` and `--data-dir` so the desktop and `lapd` share the same
/// flag spelling. `args` includes argv[0].
fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let prefix = String::from(name) + "=";
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(rest) = arg.strip_prefix(&prefix) {
            return Some(rest);
        }
        if arg == name {
            return iter.next().map(|s| s.as_str());
        }
    }
    None
}

/// Decide the server URL for client mode from explicit configuration only.
///
/// `args` is the full process argument list (argv[0] included); `env_connect`
/// is the value of `LAP_CONNECT` if set. A command-line flag always wins over
/// the environment variable. Returns `None` when neither is given, leaving the
/// embedded default in place.
pub fn client_target_from_args(args: &[String], env_connect: Option<&str>) -> Option<String> {
    if let Some(url) = flag_value(args, "--connect").and_then(normalize) {
        return Some(url);
    }
    env_connect.and_then(normalize)
}

/// Optional `--data-dir` for the desktop shell, mirroring `lapd`. This exists
/// so client/embedded mode can be exercised against an isolated directory
/// (tests, portable runs) without ever touching the real per-user data dir.
pub fn data_dir_from_args(args: &[String]) -> Option<std::path::PathBuf> {
    flag_value(args, "--data-dir")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
}

/// Trim and ensure an `http(s)://` scheme, so `--connect 127.0.0.1:17860`
/// works as well as a full URL. Blank input yields `None`.
fn normalize(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        Some(String::from("http://") + url)
    }
}

/// If `<data_dir>/lapd.lock` names a live pid other than `own_pid`, return the
/// URL of that server (`http://127.0.0.1:<port>`). A missing, unparsable, stale
/// (dead pid) or self-owned lock yields `None`, so the embedded backend may
/// start normally (plan §3.7: stale locks are taken over).
pub fn running_server_target(data_dir: &Path, own_pid: u32) -> Option<String> {
    let body = std::fs::read_to_string(data_dir.join("lapd.lock")).ok()?;
    let (pid, port) = parse_lock_owner(&body)?;
    if pid == own_pid || !pid_alive(pid) {
        return None;
    }
    Some(format!("http://127.0.0.1:{port}"))
}

/// Read `{pid, port, …}` from a lock body. The lock file is written by
/// `lap-server::lock` with those exact keys (see `crates/lap-server/src/lock.rs`).
fn parse_lock_owner(body: &str) -> Option<(u32, u16)> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
    let port = u16::try_from(value.get("port")?.as_u64()?).ok()?;
    Some((pid, port))
}

/// Cross-platform pid liveness without new dependencies. Mirrors
/// `lap-server::lock::pid_alive` (the shell does not depend on lap-server).
/// When the check cannot decide (another user's process) it reports "alive":
/// wrongly taking over a live writer is the dangerous direction, so an
/// undecidable pid must not be reclaimed.
#[cfg(target_os = "windows")]
fn pid_alive(pid: u32) -> bool {
    use std::os::raw::c_void;

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: u32 = 5;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
        fn GetExitCodeProcess(handle: *mut c_void, exit_code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && exit_code == STILL_ACTIVE
    }
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(true)
}

#[cfg(not(any(target_os = "windows", unix)))]
fn pid_alive(pid: u32) -> bool {
    let _ = pid;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn connect_flag_wins_over_env_and_accepts_both_spellings() {
        let argv = args(&["Lap.exe", "--connect", "192.168.1.5:17860"]);
        assert_eq!(
            client_target_from_args(&argv, Some("http://ignored")).as_deref(),
            Some("http://192.168.1.5:17860")
        );
        let argv = args(&["Lap.exe", "--connect=http://host:1234"]);
        assert_eq!(
            client_target_from_args(&argv, None).as_deref(),
            Some("http://host:1234")
        );
        // https is preserved verbatim.
        let argv = args(&["Lap.exe", "--connect", "https://nas.local"]);
        assert_eq!(
            client_target_from_args(&argv, None).as_deref(),
            Some("https://nas.local")
        );
    }

    #[test]
    fn env_var_is_used_only_without_a_flag() {
        let argv = args(&["Lap.exe"]);
        assert_eq!(
            client_target_from_args(&argv, Some("127.0.0.1:9")).as_deref(),
            Some("http://127.0.0.1:9")
        );
        // Blank values are ignored (no accidental client mode).
        assert_eq!(client_target_from_args(&argv, Some("   ")), None);
        assert_eq!(client_target_from_args(&argv, None), None);
        // A flag with a missing value is ignored, not consumed as the URL.
        let argv = args(&["Lap.exe", "--connect"]);
        assert_eq!(client_target_from_args(&argv, None), None);
    }

    #[test]
    fn data_dir_flag_accepts_both_spellings_and_ignores_blank() {
        assert_eq!(
            data_dir_from_args(&args(&["Lap.exe", "--data-dir", "C:/tmp/x"])),
            Some(std::path::PathBuf::from("C:/tmp/x"))
        );
        assert_eq!(
            data_dir_from_args(&args(&["Lap.exe", "--data-dir=C:/tmp/y"])),
            Some(std::path::PathBuf::from("C:/tmp/y"))
        );
        assert_eq!(data_dir_from_args(&args(&["Lap.exe"])), None);
        assert_eq!(data_dir_from_args(&args(&["Lap.exe", "--data-dir", "  "])), None);
    }

    #[test]
    fn lock_owner_query_needs_live_foreign_pid() {
        let dir = scratch("lock");
        // No lock file → embedded mode.
        assert_eq!(running_server_target(&dir, 111), None);
        // A live foreign owner (use this very process as the "server") → its URL.
        std::fs::write(
            dir.join("lapd.lock"),
            serde_json::json!({ "pid": std::process::id(), "port": 17860 }).to_string(),
        )
        .unwrap();
        assert_eq!(
            running_server_target(&dir, std::process::id() + 1).as_deref(),
            Some("http://127.0.0.1:17860")
        );
        // Our own pid is never treated as a foreign server.
        assert_eq!(running_server_target(&dir, std::process::id()), None);
        // A stale pid (unreachable) is taken over → embedded mode.
        std::fs::write(
            dir.join("lapd.lock"),
            serde_json::json!({ "pid": 4_000_000_000u64, "port": 1 }).to_string(),
        )
        .unwrap();
        assert_eq!(running_server_target(&dir, 111), None);
        // Unparsable / missing keys → embedded mode.
        std::fs::write(dir.join("lapd.lock"), "not json").unwrap();
        assert_eq!(running_server_target(&dir, 111), None);
        std::fs::write(dir.join("lapd.lock"), serde_json::json!({ "pid": 1 }).to_string()).unwrap();
        assert_eq!(running_server_target(&dir, 111), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "lap-client-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
