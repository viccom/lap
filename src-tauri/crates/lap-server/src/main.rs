//! `lapd` — the standalone Lap server (serverization plan, phase 3).
//! Batch 3.2 skeleton: flags, lock file, shared boot chain, server_info.
//! The command bridge and SSE land in the following 3.2 commits.

mod boot;
mod bridge;
mod bridge_gen;
mod flags;
mod lock;
mod sse;

use std::process::ExitCode;

use axum::extract::Path;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};

/// The read-only capability descriptor §3.3 pins for the web transport.
/// Field set is deliberately minimal (plan §3.3, open question 6) and will
/// grow when the web client consumes it.
fn server_info_value() -> serde_json::Value {
    serde_json::json!({
        "name": "lapd",
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "pathSeparator": std::path::MAIN_SEPARATOR,
        "capabilities": {
            "auth": false,
            "media": false,
        },
    })
}

async fn server_info() -> Json<serde_json::Value> {
    Json(server_info_value())
}

/// `POST /api/cmd/{name}` — the command bridge. Raw JSON args in, raw JSON
/// result out (the phase-1 http.js contract): a command's `Ok` serializes
/// as the response body, `Err(String)` becomes 500 `{"error": …}`.
async fn cmd_handler(Path(name): Path<String>, body: String) -> Response {
    let parsed = if body.trim().is_empty() {
        serde_json::Value::Object(Default::default())
    } else {
        match serde_json::from_str::<serde_json::Value>(&body) {
            Ok(v) if v.is_object() => v,
            Ok(_) => {
                return bridge::BridgeError::BadRequest("request body must be a JSON object".to_string())
                    .into_response()
            }
            Err(e) => return bridge::BridgeError::BadRequest(format!("invalid JSON body: {e}")).into_response(),
        }
    };
    bridge::handle_command(&name, &parsed).await.map(Json).into_response()
}

async fn serve(flags: flags::Flags) -> Result<(), String> {
    // boot::run installs the SSE sink as the process EventSink before any
    // boot stage can emit; the returned bus backs the /api/events route.
    let boot = boot::run(&flags)?;

    let app = axum::Router::new()
        .route("/api/server_info", get(server_info))
        .route("/api/cmd/{name}", post(cmd_handler))
        .route("/api/events", get(sse::events_handler))
        .with_state(boot.bus.clone());
    let addr = format!("{}:{}", flags.host, flags.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;

    println!(
        "lapd {} listening on http://{addr} (data: {})",
        env!("CARGO_PKG_VERSION"),
        boot.data_dir.display()
    );
    // `boot` (and its lock file guard) must outlive the server loop.
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("server error: {e}"))?;
    drop(boot);
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", flags::USAGE);
        return ExitCode::SUCCESS;
    }
    let flags = match flags::parse(argv) {
        Ok(flags) => flags,
        Err(e) => {
            eprintln!("lapd: {e}\n\n{}", flags::USAGE);
            return ExitCode::from(2);
        }
    };
    match serve(flags).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lapd: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_info_shape_is_minimal_and_honest() {
        let info = server_info_value();
        assert_eq!(info["name"], "lapd");
        assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(info["platform"], std::env::consts::OS);
        assert!(info["capabilities"]["auth"].is_boolean());
        assert!(info["capabilities"]["media"].is_boolean());
    }

    /// Commands deliberately NOT on the HTTP bridge, hand-pinned so a newly
    /// registered command stays red here until the generator is re-run AND
    /// someone reviews which side of the whitelist it belongs on.
    /// B 需适配 (4): AppHandle/Channel desktop glue.
    /// C 原生专属 (16): window, dialog, clipboard, drag, wallpaper.
    /// A 不暴露 (2): startup telemetry that only the desktop frontend calls.
    const SKIP: &[&str] = &[
        // B
        "add_album",
        "change_thumbnail_cache_location",
        "copy_images",
        "switch_library",
        // C
        "cancel_file_drag",
        "copy_edited_image",
        "copy_file_paths",
        "get_drag_payload",
        "get_external_app_display_name",
        "has_importable_clipboard",
        "import_clipboard",
        "import_from_drag",
        "open_external_url",
        "open_file_with_app",
        "open_files_with_app",
        "record_startup_stage",
        "reveal_path",
        "select_folder",
        "set_desktop_wallpaper",
        "start_file_drag",
        // A 不暴露
        "finish_startup",
        "get_ai_initialization_status",
    ];

    #[test]
    fn bridge_whitelist_covers_the_registration_exactly() {
        let main_rs = include_str!("../../../src/main.rs");
        let mut registered: Vec<String> = Vec::new();
        for line in main_rs.lines() {
            let line = line.trim();
            if line.starts_with("//") || line.is_empty() {
                continue;
            }
            if let Some(name) = registered_name(line) {
                registered.push(name);
            }
        }
        assert_eq!(registered.len(), 253, "registration count drifted");

        let bridged: std::collections::HashSet<_> = bridge_gen::COMMANDS.iter().copied().collect();
        let skip: std::collections::HashSet<_> = SKIP.iter().copied().collect();
        assert_eq!(
            bridged.len() + skip.len(),
            registered.len(),
            "whitelist + skip must partition the registry"
        );
        // The §3.6 exposure axis must cover every bridged command exactly.
        assert_eq!(
            bridge_gen::EXPOSURES.len(),
            bridge_gen::COMMANDS.len(),
            "EXPOSURES must align with COMMANDS by index"
        );
        for (name, exposure) in bridge_gen::EXPOSURES {
            assert!(
                matches!(*exposure, "read" | "write" | "admin"),
                "`{name}`: exposure `{exposure}` is not a network level"
            );
            assert!(
                bridged.contains(name),
                "`{name}` has an exposure but is not in COMMANDS"
            );
        }
        for name in &registered {
            assert!(
                bridged.contains(name.as_str()) || skip.contains(name.as_str()),
                "`{name}` is registered but neither bridged nor hand-reviewed as skipped — rerun `python -I docs/dev/gen-lapd-bridge.py` and review the skip list"
            );
        }
        for name in bridge_gen::COMMANDS {
            assert!(
                registered.iter().any(|r| r == name),
                "`{name}` is bridged but no longer registered"
            );
        }
        for name in SKIP {
            assert!(
                registered.iter().any(|r| r == name),
                "`{name}` is in the skip list but no longer registered"
            );
        }
    }

    /// The bare command name from a `t_mod::command,` registration line.
    /// Strict: both sides must be plain lowercase identifiers, so ordinary
    /// code lines that happen to end in `,` never match.
    fn registered_name(line: &str) -> Option<String> {
        let plain = |s: &str| {
            !s.is_empty()
                && s.chars().all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
        };
        let (module, name) = line.split_once("::")?;
        if !module.starts_with("t_") || !plain(module) {
            return None;
        }
        let name = name.strip_suffix(',')?;
        if !plain(name) {
            return None;
        }
        Some(name.to_string())
    }
}
