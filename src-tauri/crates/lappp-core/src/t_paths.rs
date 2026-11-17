//! Process-level path configuration (serverization plan §3.2).
//!
//! Nine core call sites resolve paths through Tauri's `app.path()`
//! (`resource_dir` for models/ffmpeg, `app_cache_dir` for the video cache).
//! To let the core run without a Tauri handle, the desktop shell resolves those
//! directories once at startup and pins them here; the call sites read from
//! this module instead of reaching for an `AppHandle`.
//!
//! Deliberately seeded from the shell rather than re-derived: Tauri's
//! `app_cache_dir()` is `cache_dir()/{identifier}` with no debug suffix, while
//! `t_config::get_app_cache_dir()` appends `.debug` in debug builds. Deriving
//! here would silently move the video cache between builds. `lapd` will seed
//! the same struct from `--data-dir` in phase 3.
//!
//! Same set-once pattern as `t_config::APP_IDENTIFIER` / `t_video::SIDE_CAR_DIR`.

use std::path::PathBuf;
use std::sync::Mutex;

/// Directories the core needs that only the host can resolve.
#[derive(Debug, Clone, Default)]
pub struct PathsConfig {
    /// Tauri resource directory (contains `models/`, `ffmpeg/`).
    pub resource_dir: Option<PathBuf>,
    /// Tauri app cache directory (contains `video_cache/`).
    pub app_cache_dir: Option<PathBuf>,
}

static PATHS: Mutex<Option<PathsConfig>> = Mutex::new(None);

/// Pin the resolved directories. Called once from the desktop `setup()`.
pub fn set_paths(config: PathsConfig) {
    if let Ok(mut guard) = PATHS.lock() {
        *guard = Some(config);
    }
}

fn get_paths() -> PathsConfig {
    PATHS
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
        .unwrap_or_default()
}

/// The resource directory, or an error if the host never pinned it (tests,
/// headless runs that did not configure paths).
pub fn resource_dir() -> Result<PathBuf, String> {
    get_paths()
        .resource_dir
        .ok_or_else(|| "resource directory is not configured".to_string())
}

/// Resolve a path under the resource directory, mirroring
/// `app.path().resolve(sub, BaseDirectory::Resource)`.
pub fn resolve_resource(sub: &str) -> Result<PathBuf, String> {
    Ok(resource_dir()?.join(sub))
}

/// The app cache directory, or an error if the host never pinned it. Falls back
/// to the `dirs`-based location so non-shell callers (tests) still resolve.
pub fn app_cache_dir() -> Result<PathBuf, String> {
    match get_paths().app_cache_dir {
        Some(dir) => Ok(dir),
        None => crate::t_config::get_app_cache_dir(),
    }
}

/// The video cache directory (`app_cache_dir/video_cache`).
pub fn video_cache_dir() -> Result<PathBuf, String> {
    Ok(app_cache_dir()?.join("video_cache"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconfigured_cache_dir_falls_back_to_config() {
        // The desktop shell has not run in this test binary, so the pin is
        // empty and the fallback must still produce a path.
        assert!(app_cache_dir().is_ok());
        assert!(video_cache_dir().unwrap().ends_with("video_cache"));
    }

    #[test]
    fn unconfigured_resource_dir_is_an_error() {
        // No shell seeded it here; callers treat this as "not available".
        // (If a test elsewhere pinned paths, this would succeed instead.)
        if get_paths().resource_dir.is_none() {
            assert!(resource_dir().is_err());
        }
    }

    // The boundary guard moved to lappp-core's guard.rs in phase 3 (it scans
    // every core source file; this module list is no longer maintained here).
}

