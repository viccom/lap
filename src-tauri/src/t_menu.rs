#[cfg(feature = "embedded")]
use tauri::menu::MenuEvent;
#[cfg(any(all(feature = "embedded", target_os = "macos"), feature = "client"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder, SubmenuBuilder};
#[cfg(all(feature = "embedded", target_os = "macos"))]
use tauri::Manager;
#[cfg(feature = "client")]
use tauri::Manager as _;
use tauri::{AppHandle, Runtime};
#[cfg(all(feature = "embedded", target_os = "macos"))]
use tauri::Emitter;

#[cfg(all(feature = "embedded", target_os = "macos"))]
pub fn install_app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let preferences_item = MenuItemBuilder::with_id("preferences", "Preferences…")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let about_item =
        MenuItemBuilder::with_id("about", format!("About {}", app.package_info().name))
            .build(app)?;
    let home_item = MenuItemBuilder::with_id("help_homepage", "lappp Home").build(app)?;
    let release_notes_item =
        MenuItemBuilder::with_id("help_release_notes", "Release Notes").build(app)?;
    let readme_item = MenuItemBuilder::with_id("help_readme", "README").build(app)?;
    let privacy_item = MenuItemBuilder::with_id("help_privacy", "Privacy").build(app)?;
    let report_issue_item =
        MenuItemBuilder::with_id("help_report_issue", "Report an Issue").build(app)?;
    // The predefined select-all item only reaches text-edit responders, so it
    // silently did nothing while photos were shown. Route a custom item to the
    // photo selection instead. The accelerator uses the character the active
    // layout produces, so it stays Cmd-A on AZERTY as well.
    let select_all_item = MenuItemBuilder::with_id("select_all", "Select All")
        .accelerator("CmdOrCtrl+A")
        .build(app)?;

    let app_submenu = SubmenuBuilder::new(app, app.package_info().name.clone())
        .item(&about_item)
        .separator()
        .item(&preferences_item)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let edit_submenu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .item(&select_all_item)
        .build()?;
    let window_submenu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize_with_text("Zoom")
        .separator()
        .close_window()
        .build()?;
    let help_submenu = SubmenuBuilder::new(app, "Help")
        .item(&home_item)
        .item(&release_notes_item)
        .item(&readme_item)
        .item(&privacy_item)
        .separator()
        .item(&report_issue_item)
        .build()?;
    let menu = MenuBuilder::new(app)
        .item(&app_submenu)
        .item(&edit_submenu)
        .item(&window_submenu)
        .item(&help_submenu)
        .build()?;

    app.set_menu(menu)?;
    Ok(())
}

#[cfg(all(feature = "embedded", not(target_os = "macos")))]
pub fn install_app_menu<R: Runtime>(_app: &AppHandle<R>) -> tauri::Result<()> {
    Ok(())
}

#[cfg(all(feature = "embedded", target_os = "macos"))]
pub fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().0.as_ref() {
        "select_all" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.emit("menu-select-all", ());
            }
        }
        "about" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.emit("app-open-about", ());
            }
        }
        "preferences" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.emit("app-open-preferences", ());
            }
        }
        "help_homepage" => {
            let _ = opener::open(
                option_env!("CARGO_PKG_HOMEPAGE").unwrap_or("https://julyx10.github.io/lap"),
            );
        }
        "help_release_notes" => {
            let _ = opener::open(format!(
                "{}/guide/release-notes/v{}.html",
                option_env!("CARGO_PKG_HOMEPAGE").unwrap_or("https://julyx10.github.io/lap"),
                env!("CARGO_PKG_VERSION")
            ));
        }
        "help_readme" => {
            let _ = opener::open(format!(
                "{}/blob/main/README.md",
                option_env!("CARGO_PKG_REPOSITORY").unwrap_or("https://github.com/julyx10/lap")
            ));
        }
        "help_privacy" => {
            let _ = opener::open(format!(
                "{}/blob/main/PRIVACY.md",
                option_env!("CARGO_PKG_REPOSITORY").unwrap_or("https://github.com/julyx10/lap")
            ));
        }
        "help_report_issue" => {
            let _ = opener::open(format!(
                "{}/issues",
                option_env!("CARGO_PKG_REPOSITORY").unwrap_or("https://github.com/julyx10/lap")
            ));
        }
        _ => {}
    }
}

#[cfg(all(feature = "embedded", not(target_os = "macos")))]
pub fn handle_menu_event<R: Runtime>(_app: &AppHandle<R>, _event: MenuEvent) {}

// -- thin client menu -------------------------------------------------------
//
// In the thin client the window usually shows a REMOTE lapd page, which has no
// Tauri API. The only native control surface that still works is a Rust-side
// menu, so switching/closing servers lives here. Menu item ids are
// `srv:<index>` (a server) or `picker` (open the local server picker).

#[cfg(feature = "client")]
pub const MENU_PICKER: &str = "picker";
#[cfg(feature = "client")]
pub const MENU_SERVER_PREFIX: &str = "srv:";

/// Build the thin client's menu from the current server list and install it.
#[cfg(feature = "client")]
pub fn install_client_menu<R: Runtime>(
    app: &AppHandle<R>,
    config: &crate::servers::ServersConfig,
) -> tauri::Result<()> {
    let mut submenu = SubmenuBuilder::new(app, "lappp");
    for (i, s) in config.servers.iter().enumerate() {
        let label = if config.current.as_deref() == Some(s.url.as_str()) {
            format!("● {}", s.name)
        } else {
            format!("   {}", s.name)
        };
        let item = MenuItemBuilder::with_id(format!("{MENU_SERVER_PREFIX}{i}"), label).build(app)?;
        submenu = submenu.item(&item);
    }
    if !config.servers.is_empty() {
        submenu = submenu.separator();
    }
    let picker_item = MenuItemBuilder::with_id(MENU_PICKER, "管理服务器…").build(app)?;
    let submenu = submenu.item(&picker_item).separator().quit().build()?;
    let menu = MenuBuilder::new(app).item(&submenu).build()?;
    app.set_menu(menu)?;
    Ok(())
}

/// Handle a thin-client menu click. `servers` is the live list (index → URL).
#[cfg(feature = "client")]
pub fn handle_client_menu_event(app: &AppHandle, id: &str, servers: &[crate::servers::Server]) {
    if id == MENU_PICKER {
        let _ = crate::client_cmds::client_show_picker(app.clone());
        return;
    }
    if let Some(idx) = id.strip_prefix(MENU_SERVER_PREFIX).and_then(|s| s.parse::<usize>().ok()) {
        if let Some(server) = servers.get(idx) {
            // Persist the choice (a menu switch is a real, persistent choice)
            // and refresh the menu so the ● marker follows.
            let _ = crate::client_cmds::connect_and_persist(app, &server.url);
            if let Some(state) = app.try_state::<crate::client_cmds::ClientState>() {
                let cfg = state.config.lock().unwrap().clone();
                let _ = install_client_menu(app, &cfg);
            }
        }
    }
}
