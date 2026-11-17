//! Commands and menu for the thin client (`--features client`).
//!
//! These are app-level commands (not `plugin:` commands), so they bypass the
//! ACL entirely (Tauri only enforces the ACL for plugin commands unless the app
//! ships an ACL manifest, which this crate does not). They are only meant to be
//! called from the LOCAL picker page, which is the only page with the Tauri API
//! available — the remote `lapd` page has none.

use std::sync::Mutex;

use tauri::Manager;

use crate::servers::{self, ServersConfig};

/// Process-wide handle to the loaded server list so menu clicks and commands
/// share one source of truth. `PathBuf` is captured once at startup.
pub struct ClientState {
    pub path: std::path::PathBuf,
    pub config: Mutex<ServersConfig>,
}

/// Point the main window at a server URL. Absolute remote URLs load fine in a
/// webview (verified against Tauri 2.10); the window keeps its decorations
/// because the remote page has no self-drawn title bar.
pub fn navigate_main(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    let parsed = tauri::Url::parse(url).map_err(|e| format!("invalid url {url}: {e}"))?;
    window.navigate(parsed).map_err(|e| e.to_string())
}

/// Persist `url` as current (if it is a known server) and navigate the main
/// window to it. Shared by the picker command and the native menu so a menu
/// switch is remembered across restarts.
pub fn connect_and_persist(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    let Some(url) = servers::normalize_url(url) else {
        return Err("服务器地址不能为空".to_string());
    };
    if let Some(state) = app.try_state::<ClientState>() {
        let mut cfg = state.config.lock().unwrap();
        cfg.set_current(&url);
        servers::save_to(&state.path, &cfg)?;
    }
    navigate_main(app, &url)
}

// -- commands (called from the local picker page) ---------------------------

#[tauri::command]
pub fn client_list_servers(state: tauri::State<ClientState>) -> ServersConfig {
    state.config.lock().unwrap().clone()
}

#[tauri::command]
pub fn client_add_server(
    state: tauri::State<ClientState>,
    name: String,
    url: String,
) -> Result<ServersConfig, String> {
    let Some(url) = servers::normalize_url(&url) else {
        return Err("服务器地址不能为空".to_string());
    };
    let mut cfg = state.config.lock().unwrap();
    cfg.add(&name, &url);
    servers::save_to(&state.path, &cfg)?;
    Ok(cfg.clone())
}

#[tauri::command]
pub fn client_remove_server(
    state: tauri::State<ClientState>,
    url: String,
) -> Result<ServersConfig, String> {
    let mut cfg = state.config.lock().unwrap();
    cfg.remove(&url);
    servers::save_to(&state.path, &cfg)?;
    Ok(cfg.clone())
}

/// Persist the choice and navigate to it. The window reloads the remote UI.
#[tauri::command]
pub fn client_connect(app: tauri::AppHandle, url: String) -> Result<(), String> {
    connect_and_persist(&app, &url)
}

/// Leave the current server and go back to the local picker page.
#[tauri::command]
pub fn client_show_picker(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "no main window".to_string())?;
    let parsed = tauri::Url::parse(picker_url()).map_err(|e| e.to_string())?;
    window.navigate(parsed).map_err(|e| e.to_string())
}

/// The picker page is served by our own `picker` URI scheme (registered in
/// `main.rs`), not the app's bundle protocol: a debug build with `devUrl` set
/// embeds no assets, and the picker must work in both debug and release.
pub const PICKER_SCHEME: &str = "picker";

/// Address of the picker page for the current platform. Tauri addresses custom
/// schemes as `http://<scheme>.localhost/...` on Windows/Android and
/// `<scheme>://localhost/...` elsewhere (same rule `t_protocol`'s thumb/preview
/// use).
pub fn picker_url() -> &'static str {
    if cfg!(any(target_os = "windows", target_os = "android")) {
        "http://picker.localhost/picker.html"
    } else {
        "picker://localhost/picker.html"
    }
}

/// The picker HTML, embedded into the binary so the client ships as one file.
/// Single source of truth: the same file the web build copies into `dist`.
pub const PICKER_HTML: &str = include_str!("../../src-vite/public/picker.html");

/// Register the `picker` protocol. Any path returns the picker page (a
/// single-page form), so a deep link cannot 404.
pub fn register_picker_protocol(builder: tauri::Builder<tauri::Wry>) -> tauri::Builder<tauri::Wry> {
    builder.register_uri_scheme_protocol(PICKER_SCHEME, |_ctx, _req| {
        http::Response::builder()
            .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(PICKER_HTML.as_bytes().to_vec())
            .unwrap_or_else(|_| http::Response::new(Vec::new()))
    })
}
