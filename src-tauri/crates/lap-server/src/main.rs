//! `lapd` — the standalone Lap server (serverization plan, phase 3).
//! Batch 3.3: authentication (Argon2id admin password, pairing-code device
//! tokens, rate limits) on top of the 3.2 command bridge and SSE. Media
//! endpoints land in the following 3.3 commits.

mod auth;
mod auth_store;
mod boot;
mod bridge;
mod bridge_gen;
mod flags;
mod lock;
mod media;
mod shutdown;
mod sse;
mod static_files;

use std::net::SocketAddr;
use std::process::ExitCode;

use axum::extract::Path;
use axum::http::Uri;
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
            "auth": true,
            "media": true,
            "web": static_files::web_enabled(),
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

/// Serve the Web client for every route not claimed by an API router. `/api/*`
/// paths are never served from the SPA shell — an unknown API route must 404,
/// not hand back index.html with a 200 (phase 4, batch 4.1).
async fn static_fallback(
    axum::extract::State(web): axum::extract::State<Option<static_files::WebRoot>>,
    uri: Uri,
) -> Response {
    let Some(web) = web else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    if uri.path().starts_with("/api/") {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    static_files::serve(&web, &uri).await
}

async fn serve(flags: flags::Flags) -> Result<(), String> {
    // boot::run installs the SSE sink as the process EventSink before any
    // boot stage can emit, and opens the auth store; the returned handles
    // back the /api/events and /api/auth routes.
    let boot = boot::run(&flags)?;

    // §3.7 graceful shutdown: the flag starts rejecting new commands and
    // tells axum to stop accepting connections.
    let shutdown = shutdown::ShutdownState::new();
    let signal_watcher = shutdown::spawn_signal_watcher(shutdown.clone());
    // The stdin control channel is how a Windows service wrapper (which has
    // no real SIGTERM) asks for a graceful stop.
    let control_watcher = shutdown::spawn_stdin_control(shutdown.clone());

    // /api/cmd and /api/events sit behind the §3.6 whitelist middleware;
    // server_info and /api/auth stay open (client discovery + first token).
    // Media routes authenticate via the §3.5 HMAC signature themselves.
    let protected = axum::Router::new()
        .route("/api/cmd/{name}", post(cmd_handler))
        .route("/api/events", get(sse::events_handler))
        .layer(axum::middleware::from_fn_with_state(
            boot.auth.clone(),
            auth::whitelist_middleware,
        ))
        .layer(axum::Extension(shutdown.clone()))
        .with_state(boot.bus.clone());
    let media_router = axum::Router::new()
        .route("/api/thumb/{library_id}/{file_id}", get(media::thumb_handler))
        .route("/api/preview/{library_id}/{file_id}", get(media::preview_handler))
        .route("/api/original/{file_id}", get(media::original_handler))
        .route("/api/video/{file_id}", get(media::video_handler))
        .with_state(boot.auth.clone());
    let auth_router = axum::Router::new()
        .route("/api/auth/status", get(auth::auth_status))
        .route("/api/auth/setup", post(auth::auth_setup))
        .route("/api/auth/login", post(auth::auth_login))
        .route("/api/auth/pair", post(auth::auth_pair))
        .route("/api/auth/pairing-code", post(auth::auth_pairing_code))
        .route("/api/auth/devices", get(auth::devices_list))
        .route("/api/auth/devices/{id}", axum::routing::delete(auth::device_revoke))
        .route("/api/media/sign", post(media::media_sign_handler))
        .with_state(boot.auth.clone());
    // Phase 4: serve the same Vue build the desktop shell embeds. `--web-dir`
    // wins; otherwise look for `resources/web` beside the executable. When
    // absent, /api keeps working and the browser simply has no shell (the
    // desktop client is unaffected).
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()));
    let web = static_files::WebRoot::resolve(flags.web_dir.as_deref(), exe_dir.as_deref());
    static_files::set_web_enabled(web.is_some());
    if let Some(root) = &web {
        println!("lapd: serving Web client from {}", root.path().display());
    }
    let app = axum::Router::new()
        .route("/api/server_info", get(server_info))
        .merge(protected)
        .merge(media_router)
        .merge(auth_router)
        .fallback(static_fallback)
        .with_state(web);
    let addr = format!("{}:{}", flags.host, flags.port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;

    println!(
        "lapd {} listening on http://{addr} (data: {})",
        env!("CARGO_PKG_VERSION"),
        boot.data_dir.display()
    );
    // `boot` (and its lock file guard) must outlive the server loop;
    // ConnectInfo feeds the loopback checks (setup, admin commands).
    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown.wait().await;
        println!("lapd: shutting down — no new commands accepted");
    })
    .await;

    // The server stopped accepting connections; now cancel the background
    // work and fold the WAL into the databases.
    let warnings = shutdown::run_shutdown_sequence(&boot.data_dir).await;
    signal_watcher.abort();
    control_watcher.abort();
    println!(
        "lapd: stopped{}",
        if warnings.is_empty() {
            String::new()
        } else {
            format!(" with {} checkpoint warning(s)", warnings.len())
        }
    );
    // The lock file guard is released here.
    drop(boot);
    serve_result.map_err(|e| format!("server error: {e}"))?;
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
    if let Some(password) = &flags.set_admin_password {
        return match set_admin_password_once(&flags, password) {
            Ok(message) => {
                println!("lapd: {message}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("lapd: {e}");
                ExitCode::from(1)
            }
        };
    }
    match serve(flags).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lapd: {e}");
            ExitCode::from(1)
        }
    }
}

/// `--set-admin-password`: resolve the data dir, hash once, exit. Refuses
/// to overwrite an existing password (rotation is deliberate, not a side
/// effect of service setup).
fn set_admin_password_once(flags: &flags::Flags, password: &str) -> Result<String, String> {
    let data_dir = boot::resolve_data_dir(flags)?;
    match auth::AuthService::set_admin_password(&data_dir, password) {
        Ok(()) => Ok(format!(
            "admin password set ({}), no server started",
            auth::admin_password_hash_path(&data_dir).display()
        )),
        Err(auth::AuthError::Conflict(m)) => Err(m.to_string()),
        Err(auth::AuthError::BadRequest(m)) => Err(m),
        Err(other) => Err(format!("{other:?}")),
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
        // The Web client adopts this separator for the library's stored paths
        // (the library uses the SERVER's separator, not the browser's).
        assert!(
            info["pathSeparator"] == "\\" || info["pathSeparator"] == "/",
            "pathSeparator must be the server separator"
        );
        assert!(info["capabilities"]["auth"].is_boolean());
        assert!(info["capabilities"]["media"].is_boolean());
        assert!(info["capabilities"]["web"].is_boolean());
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
