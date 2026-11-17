//! lappp-server bootstrap — the shared startup chain from the plan §3.7 sequence
//! table (lappp-server column). Desktop-only steps are skipped: window/menu/URI
//! protocol setup, Tauri fs scopes, the updater. The SSE sink registration
//! lands with the SSE module later in batch 3.2; until then core emissions
//! go to the EventSink no-op.

use std::path::{Path, PathBuf};

use lappp_core::t_paths::PathsConfig;
use lappp_core::t_sqlite::Album;
use lappp_core::{t_config, t_paths, t_rt, t_sqlite, t_startup, t_state, t_utils, t_video, t_vl_schedule};

use crate::flags::Flags;
use crate::lock::{self, LockGuard};

/// Everything `main` must keep alive while serving.
pub struct Boot {
    /// Held for its Drop: removing the lock file when the process exits.
    #[allow(dead_code)]
    pub lock: LockGuard,
    pub data_dir: PathBuf,
    /// The SSE fan-out the process EventSink forwards into.
    pub bus: std::sync::Arc<crate::sse::EventBus>,
    /// Authentication state (device tokens, pairing, MAC key).
    pub auth: std::sync::Arc<crate::auth::AuthService>,
}

/// Data-dir resolution shared by the boot chain and the
/// `--set-admin-password` one-shot path.
pub fn resolve_data_dir(flags: &Flags) -> Result<PathBuf, String> {
    if let Some(dir) = &flags.data_dir {
        t_config::set_data_dir_override(dir.clone());
    }
    t_config::set_app_identifier("com.julyx10.lap");
    t_config::get_app_data_dir()
}

pub fn run(flags: &Flags) -> Result<Boot, String> {
    // First thing, same as the desktop shell, so StartupState::started
    // anchors at process start.
    t_state::init_process_states();

    // The SSE sink must be installed before the boot chain runs: the
    // finish_startup tail below emits `albums-refreshed`, and core
    // background tasks may start emitting at any point after this.
    let bus = crate::sse::EventBus::new();
    lappp_core::t_events::set_sink(crate::sse::SseSink::new(bus.clone()));

    let data_dir = resolve_data_dir(flags)?;

    // Paths: resolve the resource root exactly like the desktop shell (see
    // `pick_resource_dir`) so lappp-server serves the same `models/` and `ffmpeg/`
    // from the same folder. `init_ffmpeg_path` then points the video sidecar
    // dir at that root, mirroring the desktop's setup order.
    t_paths::set_paths(PathsConfig {
        resource_dir: resource_dir_candidate(),
        app_cache_dir: Some(data_dir.clone()),
    });
    t_video::init_ffmpeg_path();

    // Creates the data dir + a default config on first run.
    t_config::load_app_config()?;

    let guard = lock::acquire(&data_dir, flags.port, std::process::id())?;

    t_startup::mark("database-started");
    match &flags.library {
        // Same flow as the desktop switch_library command minus the fs
        // scopes step (Tauri scopes do not exist here).
        Some(id) => switch_to_library(id)?,
        None => t_sqlite::create_db()?,
    }
    t_startup::mark("database-ready");

    #[cfg(target_os = "linux")]
    lappp_core::t_http::init_video_http_server();
    t_video::init_video_cache();

    t_startup::mark("setup-ready");
    t_vl_schedule::spawn_scheduler();

    // The desktop runs this from the frontend's finish_startup call; §3.7
    // pins lappp-server to self-execute it at boot. A fresh desktop client always
    // passes imageSearch.model = 0 (configStore default, localStorage-only),
    // so 0 is the faithful eager equivalent.
    t_startup::finish_startup(0);

    let auth = std::sync::Arc::new(crate::auth::AuthService::open(&data_dir)?);
    if auth.password_set() {
        // §3.6: the pairing code travels over the console — the operator
        // reads it where the server runs.
        let code = auth.mint_pairing_code();
        println!(
            "lappp-server: pairing code {code} (valid 5 minutes) — POST /api/auth/pair {{\"code\": \"{code}\", \"deviceName\": \"…\"}}"
        );
    } else {
        println!(
            "lappp-server: no admin password set — POST /api/auth/setup {{\"password\": \"…\"}} from localhost, or run `lappp-server --set-admin-password <pw>`"
        );
    }

    Ok(Boot { lock: guard, data_dir, bus, auth })
}

/// Pick the resource root the way the desktop shell does.
///
/// Tauri's `resource_dir()` on Windows is the executable's own directory
/// (the portable and installed layouts both keep `models/` and `ffmpeg/`
/// beside the binary), so the exe directory must be the first candidate — NOT
/// an `exe/resources` subfolder. Getting this wrong silently hides the bundled
/// models: the local embedding model then reads as "not installed" and
/// `embed_status` fails. Dev runs (`target/debug`, no payload beside the exe)
/// still fall back to the repo `resources/` layout.
fn resource_dir_candidate() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = &exe_dir {
        // Desktop / portable / installed: the payload sits beside the binary.
        candidates.push(dir.clone());
        // A layout that nests the payload under `resources/` beside the binary.
        candidates.push(dir.join("resources"));
    }
    // Dev run from the repo: `resources/` under the cwd or the src-tauri dir.
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("resources"));
        candidates.push(cwd.join("src-tauri").join("resources"));
    }

    let has_payload = |dir: &Path| dir.join("models").is_dir() || dir.join("ffmpeg").is_dir();
    pick_resource_dir(&candidates, exe_dir.as_deref(), &has_payload)
}

/// Choose the resource root: the first candidate that carries the payload,
/// else `fallback` (the exe directory) so callers report a missing file
/// against a real location instead of `None`.
///
/// Pure (env injected) so the layout precedence is unit-tested directly.
fn pick_resource_dir(
    candidates: &[PathBuf],
    fallback: Option<&Path>,
    has_payload: &dyn Fn(&Path) -> bool,
) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|dir| has_payload(dir))
        .cloned()
        .or_else(|| fallback.map(|dir| dir.to_path_buf()))
}

fn switch_to_library(id: &str) -> Result<(), String> {
    let _guard = t_config::library_refresh_lock()
        .write()
        .map_err(|e| e.to_string())?;
    t_config::switch_library(id)?;
    t_utils::clear_album_accessibility();
    t_sqlite::clear_conn_pool();
    // A corrupt target library surfaces here as DB_CORRUPTED_MSG; the
    // desktop turns that into a frontend banner. For lappp-server the corruption
    // flag stays set and is_database_corrupted() reports it to callers.
    t_sqlite::create_db()?;

    // The desktop refreshes album accessibility in the background after the
    // switch; lappp-server does the same on the shared runtime.
    t_rt::spawn_blocking(|| -> Result<(), String> {
        let mut albums = Album::get_all_albums()
            .map_err(|e| format!("Error while checking albums after switching library: {e}"))?;
        t_utils::refresh_all_album_accessibility(&mut albums);
        Ok(())
    });
    t_utils::start_folder_mtime_sync();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presence check over a fixed set of "populated" directories, so the
    /// precedence test needs no real filesystem.
    fn payload_in(populated: &[PathBuf]) -> impl Fn(&Path) -> bool + '_ {
        move |dir: &Path| populated.iter().any(|p| p == dir)
    }

    #[test]
    fn the_exe_directory_wins_when_it_carries_the_payload() {
        // Portable / installed layout: `models/` and `ffmpeg/` sit beside the
        // binary. The exe dir (first candidate) is chosen — never the
        // `resources/` subfolder that follows it, which would hide the models.
        let exe = PathBuf::from("D:/LapPortable");
        let candidates = vec![exe.clone(), exe.join("resources")];
        let populated = vec![exe.clone()];
        let picked = pick_resource_dir(&candidates, Some(&exe), &payload_in(&populated));
        assert_eq!(picked.as_deref(), Some(exe.as_path()));
    }

    #[test]
    fn a_nested_resources_root_is_used_when_the_exe_dir_is_bare() {
        // A layout that nests the payload under `resources/` beside the exe:
        // the second candidate carries it and must be chosen.
        let exe = PathBuf::from("D:/Lap");
        let nested = exe.join("resources");
        let candidates = vec![exe.clone(), nested.clone()];
        let populated = vec![nested.clone()];
        let picked = pick_resource_dir(&candidates, Some(&exe), &payload_in(&populated));
        assert_eq!(picked.as_deref(), Some(nested.as_path()));
    }

    #[test]
    fn a_payload_less_dev_root_is_used_for_a_bare_exe() {
        // Dev run: the binary lives in `target/debug` (bare); the repo
        // `resources/` candidate carries the payload and must be chosen.
        let exe = PathBuf::from("D:/repo/src-tauri/target/debug");
        let dev = PathBuf::from("D:/repo/src-tauri/resources");
        let candidates = vec![exe.clone(), exe.join("resources"), dev.clone()];
        let populated = vec![dev.clone()];
        let picked = pick_resource_dir(&candidates, Some(&exe), &payload_in(&populated));
        assert_eq!(picked.as_deref(), Some(dev.as_path()));
    }

    #[test]
    fn a_payload_less_everything_still_returns_the_exe_dir() {
        // Last resort: return the exe dir so callers report a missing file
        // against a real location instead of `None`.
        let exe = PathBuf::from("D:/somewhere");
        let candidates = vec![exe.clone(), exe.join("resources")];
        let populated: Vec<PathBuf> = vec![];
        let picked = pick_resource_dir(&candidates, Some(&exe), &payload_in(&populated));
        assert_eq!(picked.as_deref(), Some(exe.as_path()));
    }
}
