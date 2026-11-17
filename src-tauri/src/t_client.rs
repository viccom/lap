//! Data-directory lock inspection for the desktop shell (plan §3.7).
//!
//! Two processes must never write one photo library: the desktop embeds the
//! backend and opens the library SQLite, while `lapd` may own the same data
//! directory. `lapd` records ownership in `<data>/lapd.lock`; before the
//! desktop opens the library it must check that lock and refuse to start when
//! a live `lapd` already owns the directory (risk R2, plan §6).
//!
//! This module is desktop-only glue and stays Tauri-free so the shell can use
//! it on every build. The thin client (a separate build target) has no local
//! library and does not consult this.

use std::path::Path;

/// Read a `--name value` / `--name=value` flag from the argument list. Shared
/// spelling with `lapd` flags. `args` includes argv[0].
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

/// Optional `--data-dir` for the desktop shell, mirroring `lapd`. This exists
/// so the embedded boot can be exercised against an isolated directory
/// (tests, portable runs) without ever touching the real per-user data dir.
pub fn data_dir_from_args(args: &[String]) -> Option<std::path::PathBuf> {
    flag_value(args, "--data-dir")
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
}

/// If `<data_dir>/lapd.lock` names a live pid other than `own_pid`, return the
/// address of that server (`127.0.0.1:<port>`). A missing, unparsable, stale
/// (dead pid) or self-owned lock yields `None`, so the desktop may open the
/// library normally (plan §3.7: stale locks are taken over).
pub fn conflicting_server(data_dir: &Path, own_pid: u32) -> Option<String> {
    let body = std::fs::read_to_string(data_dir.join("lapd.lock")).ok()?;
    let (pid, port) = parse_lock_owner(&body)?;
    if pid == own_pid || !pid_alive(pid) {
        return None;
    }
    Some(format!("127.0.0.1:{port}"))
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
    fn conflict_query_needs_a_live_foreign_pid() {
        let dir = scratch("lock");
        // No lock file → no conflict.
        assert_eq!(conflicting_server(&dir, 111), None);
        // A live foreign owner (use this very process as the "server").
        std::fs::write(
            dir.join("lapd.lock"),
            serde_json::json!({ "pid": std::process::id(), "port": 17860 }).to_string(),
        )
        .unwrap();
        assert_eq!(
            conflicting_server(&dir, std::process::id() + 1).as_deref(),
            Some("127.0.0.1:17860")
        );
        // Our own pid is never a conflict.
        assert_eq!(conflicting_server(&dir, std::process::id()), None);
        // A stale pid (unreachable) is not a conflict.
        std::fs::write(
            dir.join("lapd.lock"),
            serde_json::json!({ "pid": 4_000_000_000u64, "port": 1 }).to_string(),
        )
        .unwrap();
        assert_eq!(conflicting_server(&dir, 111), None);
        // Unparsable / missing keys → no conflict.
        std::fs::write(dir.join("lapd.lock"), "not json").unwrap();
        assert_eq!(conflicting_server(&dir, 111), None);
        std::fs::write(dir.join("lapd.lock"), serde_json::json!({ "pid": 1 }).to_string()).unwrap();
        assert_eq!(conflicting_server(&dir, 111), None);
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
