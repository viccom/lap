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

    /// Modules that phase 2 has made Tauri-free and that must stay that way.
    /// Each entry is a file beside this one; `include_str!` bakes its source
    /// into the test so the scan is over the real text, not a runtime read.
    const TAURI_FREE_MODULES: &[&str] = &[
        "t_ai.rs",
        "t_ai_png.rs",
        "t_batch_rename.rs",
        "t_cache_storage.rs",
        "t_cluster.rs",
        "t_common.rs",
        "t_config.rs",
        "t_embedded_jpeg.rs",
        "t_events.rs",
        "t_face.rs",
        "t_heif.rs",
        "t_http.rs",
        "t_jpeg.rs",
        "t_jxl.rs",
        "t_lens.rs",
        "t_libraw.rs",
        "t_migration.rs",
        "t_montage.rs",
        "t_motion_photo.rs",
        "t_paths.rs",
        "t_raw_display.rs",
        "t_rt.rs",
        "t_state.rs",
        "t_storage.rs",
        "t_tags.rs",
        "t_vl_protocol.rs",
        "t_vl_tags.rs",
        "t_write.rs",
    ];

    /// The exact sources, in the same order as `TAURI_FREE_MODULES`.
    /// `include_str!` needs a literal path, so the list is spelled out.
    const TAURI_FREE_SOURCES: &[&str] = &[
        include_str!("t_ai.rs"),
        include_str!("t_ai_png.rs"),
        include_str!("t_batch_rename.rs"),
        include_str!("t_cache_storage.rs"),
        include_str!("t_cluster.rs"),
        include_str!("t_common.rs"),
        include_str!("t_config.rs"),
        include_str!("t_embedded_jpeg.rs"),
        include_str!("t_events.rs"),
        include_str!("t_face.rs"),
        include_str!("t_heif.rs"),
        include_str!("t_http.rs"),
        include_str!("t_jpeg.rs"),
        include_str!("t_jxl.rs"),
        include_str!("t_lens.rs"),
        include_str!("t_libraw.rs"),
        include_str!("t_migration.rs"),
        include_str!("t_montage.rs"),
        include_str!("t_motion_photo.rs"),
        include_str!("t_paths.rs"),
        include_str!("t_raw_display.rs"),
        include_str!("t_rt.rs"),
        include_str!("t_state.rs"),
        include_str!("t_storage.rs"),
        include_str!("t_tags.rs"),
        include_str!("t_vl_protocol.rs"),
        include_str!("t_vl_tags.rs"),
        include_str!("t_write.rs"),
    ];

    /// The boundary guard from the serverization plan (§2): a module that has
    /// been de-Tauri'd must not reach back into `tauri::`. This is what keeps
    /// the migration one-directional — a new `tauri::` reference in any of
    /// these files fails the suite instead of silently re-coupling the core.
    ///
    /// Line comments are stripped first so the doc comments that *describe* the
    /// old `tauri::async_runtime` name (t_rt, t_paths) do not trip the check;
    /// only a real code reference counts. The needle is assembled from parts so
    /// this assertion's own text cannot match itself.
    #[test]
    fn migrated_core_modules_do_not_reference_tauri() {
        assert_eq!(
            TAURI_FREE_MODULES.len(),
            TAURI_FREE_SOURCES.len(),
            "module list and source list must stay in sync"
        );
        let needle = ["tauri", "::"].concat();
        let use_needle = ["use ", "tauri"].concat();
        for (name, source) in TAURI_FREE_MODULES.iter().zip(TAURI_FREE_SOURCES) {
            let code: String = source
                .lines()
                .map(|line| match line.find("//") {
                    Some(at) => &line[..at],
                    None => line,
                })
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                !code.contains(&needle) && !code.contains(&use_needle),
                "{name} references tauri again; a migrated core module must stay Tauri-free"
            );
        }
    }
}

