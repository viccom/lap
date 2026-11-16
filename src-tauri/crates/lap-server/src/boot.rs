//! lapd bootstrap — the shared startup chain from the plan §3.7 sequence
//! table (lapd column). Desktop-only steps are skipped: window/menu/URI
//! protocol setup, Tauri fs scopes, the updater. The SSE sink registration
//! lands with the SSE module later in batch 3.2; until then core emissions
//! go to the EventSink no-op.

use std::path::{Path, PathBuf};

use lap_core::t_paths::PathsConfig;
use lap_core::t_sqlite::Album;
use lap_core::{t_config, t_paths, t_rt, t_sqlite, t_startup, t_state, t_utils, t_video, t_vl_schedule};

use crate::flags::Flags;
use crate::lock::{self, LockGuard};

/// Everything `main` must keep alive while serving.
pub struct Boot {
    /// Held for its Drop: removing the lock file when the process exits.
    #[allow(dead_code)]
    pub lock: LockGuard,
    pub data_dir: PathBuf,
}

pub fn run(flags: &Flags) -> Result<Boot, String> {
    // First thing, same as the desktop shell, so StartupState::started
    // anchors at process start.
    t_state::init_process_states();

    if let Some(dir) = &flags.data_dir {
        t_config::set_data_dir_override(dir.clone());
    }
    t_config::set_app_identifier("com.julyx10.lap");

    // Paths: lapd resolves resources next to the executable (installed and
    // portable layouts both keep `resources/` beside the binary); when
    // running from a dev target directory, the repo layout serves. The cache
    // dir follows the data root so debug builds stay isolated from the
    // installed app's caches.
    let data_dir = t_config::get_app_data_dir()?;
    t_paths::set_paths(PathsConfig {
        resource_dir: resource_dir_candidate(),
        app_cache_dir: Some(data_dir.clone()),
    });

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
    lap_core::t_http::init_video_http_server();
    t_video::init_video_cache();

    t_startup::mark("setup-ready");
    t_vl_schedule::spawn_scheduler();

    // The desktop runs this from the frontend's finish_startup call; §3.7
    // pins lapd to self-execute it at boot. A fresh desktop client always
    // passes imageSearch.model = 0 (configStore default, localStorage-only),
    // so 0 is the faithful eager equivalent.
    t_startup::finish_startup(0);

    Ok(Boot { lock: guard, data_dir })
}

fn resource_dir_candidate() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()));
    if let Some(dir) = exe_dir {
        let candidate = dir.join("resources");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    // Dev runs from a target/ directory — the repo layout serves.
    let dev = Path::new(&std::env::current_dir().ok()?).join("resources");
    dev.is_dir().then_some(dev)
}

fn switch_to_library(id: &str) -> Result<(), String> {
    let _guard = t_config::library_refresh_lock()
        .write()
        .map_err(|e| e.to_string())?;
    t_config::switch_library(id)?;
    t_utils::clear_album_accessibility();
    t_sqlite::clear_conn_pool();
    // A corrupt target library surfaces here as DB_CORRUPTED_MSG; the
    // desktop turns that into a frontend banner. For lapd the corruption
    // flag stays set and is_database_corrupted() reports it to callers.
    t_sqlite::create_db()?;

    // The desktop refreshes album accessibility in the background after the
    // switch; lapd does the same on the shared runtime.
    t_rt::spawn_blocking(|| -> Result<(), String> {
        let mut albums = Album::get_all_albums()
            .map_err(|e| format!("Error while checking albums after switching library: {e}"))?;
        t_utils::refresh_all_album_accessibility(&mut albums);
        Ok(())
    });
    t_utils::start_folder_mtime_sync();
    Ok(())
}
