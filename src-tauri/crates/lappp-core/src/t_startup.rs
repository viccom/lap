//! Startup timing and work that is not needed to render the first window.
//!
//! The state itself (`StartupState`) and the AI warm-up (`t_ai::ensure_ai`)
//! live in the process registry / `t_ai` so the core can reach them without a
//! Tauri handle; this module keeps the desktop commands and the one-shot
//! background bootstrap they trigger.
use crate::{t_ai, t_sqlite, t_state, t_utils};

/// Record a one-time startup stage in the process registry.
pub fn mark(stage: &str) {
    t_state::startup().mark(stage);
}

pub fn record_startup_stage(stage: String) {
    if matches!(
        stage.as_str(),
        "app-mounted"
            | "first-content-ready"
            | "language-ready"
            | "library-state-ready"
            | "window-shown"
    ) {
        mark(&stage);
    }
}

pub fn get_ai_initialization_status() -> String {
    t_state::startup()
        .ai_status
        .lock()
        .map(|status| status.clone())
        .unwrap_or_else(|e| format!("failed: {}", e))
}

pub fn finish_startup(model: i64) {
    mark("first-content-ready");
    // Pin the bundled models dir for the optional-model fallback; before
    // this runs the executable directory serves (see t_config).
    if let Ok(resource) = crate::t_paths::resolve_resource("models") {
        crate::t_config::set_resource_models_dir(resource);
    }
    if t_state::startup()
        .background_started
        .swap(true, std::sync::atomic::Ordering::SeqCst)
    {
        return;
    }
    let library_id = crate::t_config::current_library_id().ok();
    crate::t_rt::spawn_blocking(move || {
        // A missing optional multilingual pack should not prevent vision indexing.
        let model = if model == 1 && !t_ai::AiEngine::is_multilingual_model_available() {
            0
        } else {
            model
        };
        if let Err(error) = t_ai::ensure_ai(Some(model)) {
            eprintln!("Failed to start AI Engine: {}", error);
            report_ai_dependency_error();
        }
        if let Some(library_id) = library_id {
            let _ = crate::t_config::with_current_library(&library_id, || {
                if t_sqlite::is_database_corrupted() {
                    return Ok(());
                }
                let generation = t_utils::album_accessibility_generation();
                let mut albums = t_sqlite::Album::get_all_albums()?;
                t_utils::refresh_all_album_accessibility_if_current(&mut albums, generation);
                if generation == t_utils::album_accessibility_generation() {
                    mark("album-accessibility-ready");
                    crate::t_events::emit(
                        crate::t_events::names::ALBUMS_REFRESHED,
                        serde_json::json!({
                            "albums": albums, "refreshFolders": false, "libraryId": library_id,
                        }),
                    );
                }
                t_utils::start_folder_mtime_sync();
                Ok(())
            });
        }
    });
}

fn report_ai_dependency_error() {
    #[cfg(target_os = "windows")]
    {
        let arch_key = if cfg!(target_arch = "aarch64") {
            r"HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\ARM64"
        } else {
            r"HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\X64"
        };
        let result = std::process::Command::new("reg")
            .args(["query", arch_key, "/v", "Installed"])
            .stdout(std::process::Stdio::null())
            .status();
        let installed = result.is_ok() && result.unwrap().success();
        if !installed {
            let arch = if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x64"
            };
            let url = format!("https://aka.ms/vs/17/release/vc_redist.{}.exe", arch);
            let _ = std::process::Command::new("powershell")
                                .args(["-NoProfile", "-Command", &format!(
                                    r#"$wsh = New-Object -ComObject Wscript.Shell; $wsh.Popup('Lap requires the Microsoft Visual C++ Redistributable.`n`nA download page will open in your browser.`nPlease install it, then restart Lap.', 0, 'Lap - Missing Dependency', 0x30); Start-Process '{}'"#,
                                    url
                                )])
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status();
            // Browsing remains usable when optional AI dependencies are missing.
        }
    }
}
