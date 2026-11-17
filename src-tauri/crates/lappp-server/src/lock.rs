//! `<data>/lapd.lock` — one writer per data directory (plan §3.7).
//! Pre-phase-5 behavior: a live lock refuses startup with a message and the
//! process exits; a stale lock (dead pid, unparsable content) is taken over.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Held for the process lifetime; dropping removes the lock file.
#[derive(Debug)]
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Create/verify `<data_dir>/lapd.lock`. `pid` is a parameter so tests can
/// simulate a live or dead owner; production passes `std::process::id()`.
pub fn acquire(data_dir: &Path, port: u16, pid: u32) -> Result<LockGuard, String> {
    fs::create_dir_all(data_dir).map_err(|e| format!("cannot create data dir: {e}"))?;
    let path = data_dir.join("lapd.lock");

    if let Ok(existing) = fs::read_to_string(&path) {
        match parse_lock(&existing) {
            Some(owner) if owner != pid && pid_alive(owner) => {
                return Err(format!(
                    "another instance owns this data directory (pid {owner}, lock {})",
                    path.display()
                ));
            }
            _ => {}
        }
    }

    let started_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let body = serde_json::json!({ "pid": pid, "port": port, "startedAt": started_at });
    fs::write(&path, serde_json::to_string(&body).unwrap_or_default())
        .map_err(|e| format!("cannot write lock file {}: {e}", path.display()))?;
    Ok(LockGuard { path })
}

fn parse_lock(body: &str) -> Option<u32> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("pid")?
        .as_u64()
        .map(|p| u32::try_from(p).ok())?
}

/// Cross-platform pid liveness check without new dependencies. When the
/// check itself cannot decide (process of another user, etc.) we assume
/// "alive": refusing a legitimate start is recoverable (delete the lock),
/// clobbering a live writer is not.
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
        // Localized `tasklist` output is not parseable; query the process
        // directly instead.
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // A pid we may not inspect is treated as alive; a pid that does
            // not exist fails with ERROR_INVALID_PARAMETER (87).
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

    fn scratch_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("lapd-lock-{tag}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn acquiring_creates_and_drop_removes_the_lock() {
        let dir = scratch_dir("create");
        let guard = acquire(&dir, 17860, std::process::id()).unwrap();
        let body = fs::read_to_string(dir.join("lapd.lock")).unwrap();
        assert!(body.contains(&std::process::id().to_string()));
        drop(guard);
        assert!(!dir.join("lapd.lock").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_live_owner_refuses_a_second_acquire() {
        // A genuinely live foreign pid: a short-lived child process.
        let mut child = std::process::Command::new(if cfg!(windows) { "ping" } else { "sleep" })
            .args(if cfg!(windows) { &["-n", "3", "127.0.0.1"][..] } else { &["2"][..] })
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn sleep child");
        let foreign_pid = child.id();
        let dir = scratch_dir("live");
        let _owner = acquire(&dir, 17860, foreign_pid).unwrap();
        let err = acquire(&dir, 17860, foreign_pid + 1).unwrap_err();
        assert!(err.contains("another instance owns"), "got: {err}");
        drop(_owner);
        let _ = child.kill();
        let _ = child.wait();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn our_own_pid_is_taken_over_without_liveness_check() {
        // A leftover lock naming ourselves (crashed restart) is reclaimed.
        let dir = scratch_dir("self");
        let _guard = acquire(&dir, 17860, std::process::id()).unwrap();
        let _guard = acquire(&dir, 17860, std::process::id()).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_lock_is_taken_over() {
        let dir = scratch_dir("stale");
        fs::write(
            dir.join("lapd.lock"),
            serde_json::json!({ "pid": 4_000_000_000u64, "port": 1 }).to_string(),
        )
        .unwrap();
        let _guard = acquire(&dir, 17860, std::process::id()).unwrap();
        let body = fs::read_to_string(dir.join("lapd.lock")).unwrap();
        assert!(body.contains(&std::process::id().to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unparsable_lock_is_taken_over() {
        let dir = scratch_dir("garbage");
        fs::write(dir.join("lapd.lock"), "not json at all").unwrap();
        let _guard = acquire(&dir, 17860, std::process::id()).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
