//! Desktop-shell filesystem scope authorization (serverization plan §3.2).
//!
//! Granting an album directory access through Tauri's platform scope store
//! only matters to the desktop shell (the web server validates paths itself),
//! so these two helpers live here rather than in `t_utils`.

pub fn authorize_directory_scope(
    app_handle: &tauri::AppHandle,
    dir_path: &str,
) -> Result<(), String> {
    use tauri::Manager;

    app_handle
        .state::<tauri::Scopes>()
        .allow_directory(dir_path, true)
        .map_err(|e| format!("Failed to authorize directory '{}': {}", dir_path, e))
}

pub fn restore_album_scopes(app_handle: &tauri::AppHandle) -> Result<(), String> {
    let albums = crate::t_sqlite::Album::get_all_albums()?;

    for album in albums {
        authorize_directory_scope(app_handle, &album.path)?;
    }

    Ok(())
}
