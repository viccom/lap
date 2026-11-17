/**
 * Desktop command wrappers (serverization phase 3): thin
 * #[tauri::command] layers over lap_core::t_cmds. Signatures mirror the
 * core functions verbatim; the contract guards pin them. Drag/clipboard
 * and fs-scope commands live here (desktop-only glue).
 */

use lap_core::t_config::{self, AppConfig, Library, LibraryInfo, LibraryState};
use lap_core::t_face;
use lap_core::t_image;
use lap_core::t_montage;
use lap_core::t_raw_display::RawDisplayOptions;
use lap_core::t_similar;
use lap_core::t_sqlite::{
    ACamera, ACollection, ACollectionOrder, ACollectionSelectionCount, AFile, AFileCollection,
    AFolder, ALens, ALocation, ATag, ATagFileState, ATagSelectionCount, AThumb, ATimeLine, Album,
    AlbumDisplayOrder, GroupedQueryResult, ImageSearchParams, Person, PersonPage,
    PersonPageRequest, QueryParams, SmartQueryParams,
};
use lap_core::t_ai;
use lap_core::t_sqlite;
use lap_core::t_storage;
use lap_core::t_utils;
use lap_core::t_write;

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use crate::t_pasteboard;
use crate::t_scopes;

#[allow(unused_imports)]
use lap_core::t_cmds::*;


#[tauri::command]
pub async fn switch_library(app_handle: tauri::AppHandle, id: String) -> Result<(), String> {
    // The blocking task reports whether the target library turned out to be corrupt, deciding from
    // create_db's returned error itself rather than re-reading the process-global flag afterwards
    // (which a concurrent switch could resolve against a different library).
    let corrupted = lap_core::t_rt::spawn_blocking(move || -> Result<bool, String> {
        let _guard = t_config::library_refresh_lock()
            .write()
            .map_err(|e| e.to_string())?;
        t_config::switch_library(&id)?;
        t_utils::clear_album_accessibility();
        t_sqlite::clear_conn_pool();
        match t_sqlite::create_db() {
            Ok(()) => Ok(false),
            Err(e) if e == t_sqlite::DB_CORRUPTED_MSG => Ok(true),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| format!("Failed to join switch library task: {}", e))??;

    if corrupted {
        return Ok(());
    }
    t_scopes::restore_album_scopes(&app_handle)?;
    lap_core::t_rt::spawn_blocking(|| -> Result<(), String> {
        let mut albums = Album::get_all_albums()
            .map_err(|e| format!("Error while checking albums after switching library: {}", e))?;
        t_utils::refresh_all_album_accessibility(&mut albums);
        Ok(())
    })
    .await
    .map_err(|e| format!("Failed to join album accessibility check: {}", e))??;
    t_utils::start_folder_mtime_sync();
    Ok(())
}


#[tauri::command]
pub fn add_album(
    app_handle: tauri::AppHandle,
    folder_path: &str,
    name: String,
    description: String,
    file_types: i64,
    small_image_filter: i64,
    excluded_folders: Vec<String>,
) -> Result<Album, String> {
    // Authorizing the directory with the platform's filesystem scope is a
    // desktop-shell concern; the album itself is written by the core.
    t_scopes::authorize_directory_scope(&app_handle, folder_path).map_err(|e| {
        format!(
            "Error while authorizing album folder '{}': {}",
            folder_path, e
        )
    })?;

    t_write::add_album(
        folder_path,
        &name,
        &description,
        file_types,
        small_image_filter,
        &excluded_folders,
    )
}


#[tauri::command]
pub fn add_album_by_path(
    folder_path: &str,
    name: String,
    description: String,
    file_types: i64,
    small_image_filter: i64,
    excluded_folders: Vec<String>,
) -> Result<Album, String> {
    // No fs-scope authorization: the Web server reads the folder itself (see
    // the core doc). The desktop keeps using `add_album`, which authorizes.
    lap_core::t_cmds::add_album_by_path(
        folder_path,
        name,
        description,
        file_types,
        small_image_filter,
        excluded_folders,
    )
}


#[tauri::command]
pub fn select_folder(
    app_handle: tauri::AppHandle,
    album_id: i64,
    folder_path: &str,
) -> Result<AFolder, String> {
    if !t_utils::directory_accessible(folder_path) {
        if let Some(renamed_path) = find_renamed_sibling_folder(album_id, folder_path)? {
            t_scopes::authorize_directory_scope(&app_handle, &renamed_path)
                .map_err(|e| format!("Error while authorizing folder '{}': {}", renamed_path, e))?;
            let inode = t_utils::FileInfo::new(&renamed_path)
                .ok()
                .map(|info| info.inode as i64)
                .filter(|inode| *inode != 0);
            AFolder::migrate_path_by_inode(album_id, &renamed_path, inode)?;
            return AFolder::fetch(&renamed_path)?
                .ok_or_else(|| format!("Renamed folder missing from DB: {}", renamed_path));
        }

        if let Some(folder) = AFolder::fetch(folder_path)? {
            if folder.album_id == album_id {
                return Ok(folder);
            }
        }
    }

    t_scopes::authorize_directory_scope(&app_handle, folder_path)
        .map_err(|e| format!("Error while authorizing folder '{}': {}", folder_path, e))?;

    AFolder::add_to_db(album_id, folder_path)
        .map_err(|e| format!("Error while adding folder to DB: {}", e))
}


#[tauri::command]
pub async fn copy_images(
    app_handle: tauri::AppHandle,
    file_paths: Vec<String>,
) -> Result<usize, String> {
    t_pasteboard::copy_files_to_clipboard(&app_handle, file_paths).await
}


#[tauri::command]
pub async fn change_thumbnail_cache_location(
    new_dir: Option<String>,
    operation_id: String,
    allow_missing_source: Option<bool>,
    on_progress: tauri::ipc::Channel<lap_core::t_cache_storage::CacheProgress>,
) -> Result<lap_core::t_cache_storage::CacheChangeResult, lap_core::t_cache_storage::CacheChangeError> {
    ensure_db_storage_change_allowed()?;
    let operation = lap_core::t_cache_storage::MigrationOperation::begin(operation_id)?;
    lap_core::t_rt::spawn_blocking(move || {
        let _guard = cache_migration_library_guard(&operation.cancellation)?;
        lap_core::t_cache_storage::change(
            new_dir,
            allow_missing_source.unwrap_or(false),
            &operation.cancellation,
            |progress| {
                let _ = on_progress.send(progress);
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}


#[tauri::command]
pub async fn set_desktop_wallpaper(
    // macOS needs an AppHandle to hop to the main thread (see the cfg block
    // below); other platforms never touch it. Injected by Tauri, never sent
    // by the frontend — restoring the pre-3.1 signature verbatim.
    _app_handle: tauri::AppHandle,
    file_path: &str,
    companion_path: Option<String>,
) -> Result<(), String> {
    let source_path = companion_path
        .filter(|path| !path.trim().is_empty() && Path::new(path).is_file())
        .unwrap_or_else(|| file_path.to_string());
    if !Path::new(&source_path).is_file() {
        return Err("Wallpaper source file does not exist".to_string());
    }

    #[cfg(target_os = "macos")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        _app_handle
            .run_on_main_thread(move || {
                use objc2::MainThreadMarker;
                use objc2::runtime::AnyObject;
                use objc2_app_kit::{NSScreen, NSWorkspace, NSWorkspaceDesktopImageOptionKey};
                use objc2_foundation::{NSDictionary, NSString, NSURL};

                let result = (|| {
                    let main_thread = MainThreadMarker::new().ok_or_else(|| {
                        "macOS wallpaper updates must run on the main thread".to_string()
                    })?;
                    let screen = NSScreen::mainScreen(main_thread)
                        .ok_or_else(|| "macOS could not find the primary display".to_string())?;
                    let path = NSString::from_str(&source_path);
                    let url = NSURL::fileURLWithPath(&path);
                    let options: objc2::rc::Retained<
                        NSDictionary<NSWorkspaceDesktopImageOptionKey, AnyObject>,
                    > = NSDictionary::new();
                    unsafe {
                        NSWorkspace::sharedWorkspace()
                            .setDesktopImageURL_forScreen_options_error(&url, &screen, &options)
                    }
                    .map_err(|error| {
                        format!(
                            "macOS could not set the desktop wallpaper: {}",
                            error.localizedDescription()
                        )
                    })
                })();
                let _ = sender.send(result);
            })
            .map_err(|error| format!("Failed to schedule macOS wallpaper update: {error}"))?;
        receiver
            .await
            .map_err(|_| "macOS wallpaper update was cancelled".to_string())??;
    }

    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::Com::{
            CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        };
        use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper};
        use windows::core::HSTRING;

        let (sender, receiver) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| unsafe {
                // `CoInitializeEx` returns an HRESULT (rather than a Result) in
                // windows 0.61, so convert it before attaching application context.
                CoInitializeEx(None, COINIT_APARTMENTTHREADED)
                    .ok()
                    .map_err(|error| format!("Failed to initialize Windows COM: {error}"))?;
                let update = (|| {
                    let wallpaper: IDesktopWallpaper =
                        CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL).map_err(|error| {
                            format!("Windows desktop wallpaper service is unavailable: {error}")
                        })?;
                    let monitor_count = wallpaper.GetMonitorDevicePathCount().map_err(|error| {
                        format!("Failed to enumerate Windows displays: {error}")
                    })?;
                    let primary_monitor = (0..monitor_count)
                        .find_map(|index| {
                            let monitor = wallpaper.GetMonitorDevicePathAt(index).ok()?;
                            let monitor_id = monitor.to_hstring();
                            CoTaskMemFree(Some(monitor.0 as *const std::ffi::c_void));
                            let rect = wallpaper.GetMonitorRECT(&monitor_id).ok()?;
                            (rect.left <= 0 && rect.right > 0 && rect.top <= 0 && rect.bottom > 0)
                                .then_some(monitor_id)
                        })
                        .ok_or_else(|| "Windows could not find the primary display".to_string())?;
                    let path = HSTRING::from(source_path);
                    wallpaper
                        .SetWallpaper(&primary_monitor, &path)
                        .map_err(|error| {
                            format!("Windows could not set the desktop wallpaper: {error}")
                        })
                })();
                CoUninitialize();
                update
            })();
            let _ = sender.send(result);
        });
        receiver
            .await
            .map_err(|_| "Windows wallpaper update was cancelled".to_string())??;
    }

    #[cfg(target_os = "linux")]
    {
        use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

        const URI_PATH_ENCODE_SET: &AsciiSet = &CONTROLS
            .add(b' ')
            .add(b'"')
            .add(b'#')
            .add(b'%')
            .add(b'<')
            .add(b'>')
            .add(b'?')
            .add(b'[')
            .add(b'\\')
            .add(b']')
            .add(b'^')
            .add(b'`')
            .add(b'{')
            .add(b'|')
            .add(b'}');

        let file_uri = format!(
            "file://{}",
            utf8_percent_encode(&source_path, URI_PATH_ENCODE_SET)
        );
        let set_key = |key: &str| {
            Command::new("gsettings")
                .args(["set", "org.gnome.desktop.background", key, &file_uri])
                .output()
        };
        let output = set_key("picture-uri")
            .map_err(|error| format!("Failed to start GNOME wallpaper service: {error}"))?;
        if !output.status.success() {
            return Err(
                "This Linux desktop environment does not support setting wallpapers from Lap"
                    .to_string(),
            );
        }
        // GNOME 42+ can use a separate dark wallpaper. Older schemas simply
        // reject this key, which is safe to ignore after picture-uri succeeds.
        let _ = set_key("picture-uri-dark");
    }

    Ok(())
}


#[tauri::command]
pub async fn has_importable_clipboard(_app_handle: tauri::AppHandle) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(clipboard_data) = crate::t_pasteboard::get_clipboard_import_data(&_app_handle).await
        else {
            return false;
        };

        if clipboard_data.file_paths.iter().any(|path| {
            std::path::Path::new(path).is_file() && t_utils::get_file_type(path).is_some()
        }) {
            return true;
        }

        if clipboard_data.png.is_some() {
            return true;
        }
    }

    let Ok(mut clipboard) = arboard::Clipboard::new() else {
        return false;
    };

    if let Ok(file_paths) = clipboard.get().file_list() {
        if file_paths
            .iter()
            .any(|path| path.is_file() && path.to_str().and_then(t_utils::get_file_type).is_some())
        {
            return true;
        }
    }

    clipboard.get_image().is_ok()
}


#[tauri::command]
pub async fn import_clipboard(
    _app_handle: tauri::AppHandle,
    folder_id: i64,
    folder_path: &str,
) -> Result<Vec<AFile>, String> {
    #[cfg(target_os = "linux")]
    {
        let clipboard_data = crate::t_pasteboard::get_clipboard_import_data(&_app_handle).await?;
        let mut file_error = None;
        if !clipboard_data.file_paths.is_empty() {
            let mut imported = Vec::new();
            for path in clipboard_data
                .file_paths
                .iter()
                .map(std::path::Path::new)
                .filter(|path| path.is_file())
            {
                let supported = path.to_str().and_then(t_utils::get_file_type).is_some();
                if !supported {
                    continue;
                }
                match import_clipboard_file(path, folder_id, folder_path) {
                    Ok(file) => imported.push(file),
                    Err(error) => {
                        eprintln!(
                            "Failed to import clipboard file {}: {}",
                            path.display(),
                            error
                        );
                        file_error.get_or_insert(error);
                    }
                }
            }
            if !imported.is_empty() {
                return Ok(imported);
            }
        }

        if let Some(png) = clipboard_data.png {
            let new_path = t_utils::save_bytes_to_folder(&png, "image/png", folder_path)
                .ok_or_else(|| "Failed to save clipboard image".to_string())?;
            let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
                let _ = std::fs::remove_file(&new_path);
                format!("Unsupported file type: {}", new_path)
            })?;
            let now = chrono::Utc::now().timestamp_millis();
            return match AFile::add_to_db(folder_id, &new_path, file_type, now) {
                Ok((file, _)) => Ok(vec![file]),
                Err(error) => {
                    let _ = std::fs::remove_file(&new_path);
                    Err(error)
                }
            };
        }

        if !clipboard_data.file_paths.is_empty() {
            return file_error.map_or_else(|| Ok(Vec::new()), Err);
        }
    }

    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| format!("Failed to open clipboard: {}", e))?;
    if let Ok(file_paths) = clipboard.get().file_list() {
        if !file_paths.is_empty() {
            let mut file_error = None;
            let mut imported = Vec::new();
            for path in file_paths.iter().filter(|path| path.is_file()) {
                let supported = path.to_str().and_then(t_utils::get_file_type).is_some();
                if !supported {
                    continue;
                }
                match import_clipboard_file(path, folder_id, folder_path) {
                    Ok(file) => imported.push(file),
                    Err(error) => {
                        eprintln!(
                            "Failed to import clipboard file {}: {}",
                            path.display(),
                            error
                        );
                        file_error.get_or_insert(error);
                    }
                }
            }
            if !imported.is_empty() {
                return Ok(imported);
            }
            return file_error.map_or_else(|| Ok(Vec::new()), Err);
        }
    }

    let clipboard_image = match clipboard.get_image() {
        Ok(image) => image,
        Err(arboard::Error::ContentNotAvailable) => return Ok(Vec::new()),
        Err(error) => return Err(format!("Failed to read clipboard image: {}", error)),
    };
    let width = u32::try_from(clipboard_image.width)
        .map_err(|_| "Clipboard image width is too large".to_string())?;
    let height = u32::try_from(clipboard_image.height)
        .map_err(|_| "Clipboard image height is too large".to_string())?;
    let rgba = image::RgbaImage::from_raw(width, height, clipboard_image.bytes.into_owned())
        .ok_or_else(|| "Clipboard image data is invalid".to_string())?;

    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .map_err(|e| format!("Failed to encode clipboard image: {}", e))?;
    let new_path = t_utils::save_bytes_to_folder(bytes.get_ref(), "image/png", folder_path)
        .ok_or_else(|| "Failed to save clipboard image".to_string())?;
    let file_type = t_utils::get_file_type(&new_path).ok_or_else(|| {
        let _ = std::fs::remove_file(&new_path);
        format!("Unsupported file type: {}", new_path)
    })?;
    let now = chrono::Utc::now().timestamp_millis();
    match AFile::add_to_db(folder_id, &new_path, file_type, now) {
        Ok((file, _)) => Ok(vec![file]),
        Err(error) => {
            let _ = std::fs::remove_file(&new_path);
            Err(error)
        }
    }
}


/// Import an image from the macOS drag pasteboard (for browser-sourced drags
/// where Tauri cannot provide file paths).
#[tauri::command]
pub async fn import_from_drag(
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String> {
    let url = crate::t_pasteboard::get_drag_image_url()
        .ok_or_else(|| "No image URL found in drag pasteboard".to_string())?;
    lap_core::t_cmds::import_url_inner(&url, folder_id, folder_path).await
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragPayload {
    pub file_paths: Vec<String>,
    pub url: Option<String>,
}

#[tauri::command]
pub fn get_drag_payload() -> DragPayload {
    DragPayload {
        file_paths: crate::t_pasteboard::get_drag_file_paths(),
        url: crate::t_pasteboard::get_drag_image_url(),
    }
}


#[tauri::command]
pub fn get_system_region_locale() -> Result<Option<String>, String>
{
    lap_core::t_cmds::get_system_region_locale()
}


#[tauri::command]
pub fn get_app_config() -> Result<AppConfig, String>
{
    lap_core::t_cmds::get_app_config()
}


#[tauri::command]
pub fn get_supported_format_extensions() -> SupportedFormatExtensions
{
    lap_core::t_cmds::get_supported_format_extensions()
}


#[tauri::command]
pub fn get_ffmpeg_backed_image_extensions() -> Vec<String>
{
    lap_core::t_cmds::get_ffmpeg_backed_image_extensions()
}


#[tauri::command]
pub fn check_gstreamer_available() -> bool
{
    lap_core::t_cmds::check_gstreamer_available()
}


#[tauri::command]
pub fn set_last_selected_item_index(index: i64) -> Result<(), String>
{
    lap_core::t_cmds::set_last_selected_item_index(index)
}


#[tauri::command]
pub fn get_thumbnail_cache_location() -> Result<lap_core::t_cache_storage::CacheLocation, String>
{
    lap_core::t_cmds::get_thumbnail_cache_location()
}


#[tauri::command]
pub async fn get_thumbnail_cache_migration_count() -> Result<u64, lap_core::t_cache_storage::CacheChangeError>
{
    lap_core::t_cmds::get_thumbnail_cache_migration_count().await
}


#[tauri::command]
pub fn cancel_thumbnail_cache_migration(operation_id: String) -> Result<bool, String>
{
    lap_core::t_cmds::cancel_thumbnail_cache_migration(operation_id)
}


#[tauri::command]
pub fn get_db_storage_dir() -> Result<String, String>
{
    lap_core::t_cmds::get_db_storage_dir()
}


#[tauri::command]
pub fn is_using_custom_db_storage() -> Result<bool, String>
{
    lap_core::t_cmds::is_using_custom_db_storage()
}


#[tauri::command]
pub async fn change_db_storage_dir(
    new_dir: String,
) -> Result<t_storage::DbStorageChangeResult, String>
{
    lap_core::t_cmds::change_db_storage_dir(new_dir).await
}


#[tauri::command]
pub async fn reset_db_storage_dir() -> Result<t_storage::DbStorageChangeResult, String>
{
    lap_core::t_cmds::reset_db_storage_dir().await
}


#[tauri::command]
pub fn add_library(name: &str) -> Result<Library, String>
{
    lap_core::t_cmds::add_library(name)
}


#[tauri::command]
pub fn hide_library(id: &str, hidden: bool) -> Result<(), String>
{
    lap_core::t_cmds::hide_library(id, hidden)
}


#[tauri::command]
pub fn reorder_libraries(ids: Vec<String>) -> Result<(), String>
{
    lap_core::t_cmds::reorder_libraries(ids)
}


#[tauri::command]
pub fn edit_library(id: &str, name: &str) -> Result<(), String>
{
    lap_core::t_cmds::edit_library(id, name)
}


#[tauri::command]
pub async fn remove_library(id: String) -> Result<(), String>
{
    lap_core::t_cmds::remove_library(id).await
}


#[tauri::command]
pub fn is_database_corrupted() -> bool
{
    lap_core::t_cmds::is_database_corrupted()
}


#[tauri::command]
pub async fn get_library_info(id: String) -> Result<LibraryInfo, String>
{
    lap_core::t_cmds::get_library_info(id).await
}


#[tauri::command]
pub fn save_library_state(id: &str, state: LibraryState) -> Result<(), String>
{
    lap_core::t_cmds::save_library_state(id, state)
}


#[tauri::command]
pub fn get_library_state(id: &str) -> Result<LibraryState, String>
{
    lap_core::t_cmds::get_library_state(id)
}


#[tauri::command]
pub fn get_current_library_state() -> Result<LibraryState, String>
{
    lap_core::t_cmds::get_current_library_state()
}


#[tauri::command]
pub fn get_all_albums(refresh_accessibility: bool) -> Result<Vec<Album>, String>
{
    lap_core::t_cmds::get_all_albums(refresh_accessibility)
}


#[tauri::command]
pub fn get_all_album_folders(include_excluded: Option<bool>) -> Result<Vec<AFolder>, String>
{
    lap_core::t_cmds::get_all_album_folders(include_excluded)
}


#[tauri::command]
pub fn generate_directory_thumbnails(
    dir_path: &str,
    output_dir: &str,
    thumbnail_size: u32,
) -> Result<t_image::BatchThumbnailStats, String>
{
    lap_core::t_cmds::generate_directory_thumbnails(dir_path, output_dir, thumbnail_size)
}


#[tauri::command]
pub fn get_album(album_id: i64) -> Result<Album, String>
{
    lap_core::t_cmds::get_album(album_id)
}


#[tauri::command]
pub async fn check_album_accessibility(album_id: i64) -> Result<bool, String>
{
    lap_core::t_cmds::check_album_accessibility(album_id).await
}


#[tauri::command]
pub fn recount_album(album_id: i64) -> Result<Album, String>
{
    lap_core::t_cmds::recount_album(album_id)
}


#[tauri::command]
pub fn get_album_visible_counts() -> Result<HashMap<i64, i64>, String>
{
    lap_core::t_cmds::get_album_visible_counts()
}


#[tauri::command]
pub async fn edit_album(
    id: i64,
    name: String,
    description: String,
    file_types: i64,
    small_image_filter: i64,
    excluded_folders: Vec<String>,
) -> Result<usize, String>
{
    lap_core::t_cmds::edit_album(id, name, description, file_types, small_image_filter, excluded_folders).await
}


#[tauri::command]
pub async fn list_album_subfolders(path: String) -> Result<Vec<String>, String>
{
    lap_core::t_cmds::list_album_subfolders(path).await
}


#[tauri::command]
pub async fn remove_album(id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::remove_album(id).await
}


#[tauri::command]
pub fn reorder_albums(items: Vec<AlbumDisplayOrder>) -> Result<usize, String>
{
    lap_core::t_cmds::reorder_albums(items)
}


#[tauri::command]
pub fn set_album_cover(id: i64, file_id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::set_album_cover(id, file_id)
}


#[tauri::command]
pub fn index_album(
    album_id: i64,
    thumbnail_size: u32,
    raw_display_options: RawDisplayOptions,
    skip_file_path: Option<String>,
    group_raw_jpeg_pairs: bool,
) -> Result<(), String>
{
    lap_core::t_cmds::index_album(album_id, thumbnail_size, raw_display_options, skip_file_path, group_raw_jpeg_pairs)
}


#[tauri::command]
pub fn cancel_indexing(album_id: i64) -> Result<(), String>
{
    lap_core::t_cmds::cancel_indexing(album_id)
}


#[tauri::command]
pub fn get_index_recovery_info() -> Option<lap_core::t_utils::IndexRecoveryInfo>
{
    lap_core::t_cmds::get_index_recovery_info()
}


#[tauri::command]
pub fn clear_index_recovery_info() -> Result<(), String>
{
    lap_core::t_cmds::clear_index_recovery_info()
}


#[tauri::command]
pub fn fetch_folder(
    path: &str,
    is_recursive: bool,
    sort: i64,
) -> Result<t_utils::FileNode, String>
{
    lap_core::t_cmds::fetch_folder(path, is_recursive, sort)
}


#[tauri::command]
pub async fn count_folder(
    path: String,
    file_types: Option<i64>,
    excluded_folders: Option<Vec<String>>,
    small_image_filter: Option<i64>,
) -> Result<(u64, u64, u64, u64, u64, u64, u64), String>
{
    lap_core::t_cmds::count_folder(path, file_types, excluded_folders, small_image_filter).await
}


#[tauri::command]
pub fn create_folder(path: &str, folder_name: &str) -> Option<String>
{
    lap_core::t_cmds::create_folder(path, folder_name)
}


#[tauri::command]
pub fn rename_folder(folder_path: &str, new_folder_name: &str) -> Option<String>
{
    lap_core::t_cmds::rename_folder(folder_path, new_folder_name)
}


#[tauri::command]
pub fn move_folder(
    folder_path: &str,
    new_album_id: i64,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String>
{
    lap_core::t_cmds::move_folder(folder_path, new_album_id, new_folder_path, conflict_policy)
}


#[tauri::command]
pub fn move_folder_outside_library(
    folder_path: &str,
    new_folder_path: &str,
    conflict_policy: &str,
) -> Result<String, String>
{
    lap_core::t_cmds::move_folder_outside_library(folder_path, new_folder_path, conflict_policy)
}


#[tauri::command]
pub fn copy_folder(
    folder_path: &str,
    new_folder_path: &str,
    new_album_id: i64,
    conflict_policy: &str,
) -> Result<String, String>
{
    lap_core::t_cmds::copy_folder(folder_path, new_folder_path, new_album_id, conflict_policy)
}


#[tauri::command]
pub fn prepare_folder_delete(folder_path: &str, library_id: &str) -> Result<String, String>
{
    lap_core::t_cmds::prepare_folder_delete(folder_path, library_id)
}


#[tauri::command]
pub fn delete_folder(
    folder_path: &str,
    library_id: &str,
    expected_identity: &str,
) -> Result<FolderTrashResult, String>
{
    lap_core::t_cmds::delete_folder(folder_path, library_id, expected_identity)
}


#[tauri::command]
pub fn delete_folder_permanently(
    folder_path: &str,
    library_id: &str,
    expected_identity: &str,
) -> Result<usize, String>
{
    lap_core::t_cmds::delete_folder_permanently(folder_path, library_id, expected_identity)
}


#[tauri::command]
pub fn reveal_path(path: &str) -> Result<(), String>
{
    lap_core::t_cmds::reveal_path(path)
}


#[tauri::command]
pub fn open_external_url(url: &str) -> Result<(), String>
{
    lap_core::t_cmds::open_external_url(url)
}


#[tauri::command]
pub fn get_external_app_display_name(app_path: &str) -> Result<String, String>
{
    lap_core::t_cmds::get_external_app_display_name(app_path)
}


#[tauri::command]
pub fn open_file_with_app(file_path: &str, app_path: &str) -> Result<(), String>
{
    lap_core::t_cmds::open_file_with_app(file_path, app_path)
}


#[tauri::command]
pub fn open_files_with_app(file_paths: Vec<String>, app_path: &str) -> Result<(), String>
{
    lap_core::t_cmds::open_files_with_app(file_paths, app_path)
}


#[tauri::command]
pub async fn get_query_count_and_sum(params: QueryParams) -> Result<(i64, i64), String>
{
    lap_core::t_cmds::get_query_count_and_sum(params).await
}


#[tauri::command]
pub async fn get_query_time_line(params: QueryParams) -> Result<Vec<ATimeLine>, String>
{
    lap_core::t_cmds::get_query_time_line(params).await
}


#[tauri::command]
pub async fn get_query_files(
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<Vec<AFile>, String>
{
    lap_core::t_cmds::get_query_files(params, offset, limit).await
}


#[tauri::command]
pub async fn get_files_by_ids(file_ids: Vec<i64>) -> Result<Vec<AFile>, String>
{
    lap_core::t_cmds::get_files_by_ids(file_ids).await
}


#[tauri::command]
pub async fn get_grouped_query_rows(
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<GroupedQueryResult, String>
{
    lap_core::t_cmds::get_grouped_query_rows(params, offset, limit).await
}


#[tauri::command]
pub async fn get_group_file_ids(params: QueryParams, group_id: String) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_group_file_ids(params, group_id).await
}


#[tauri::command]
pub async fn get_grouped_file_position(
    params: QueryParams,
    file_id: i64,
) -> Result<Option<i64>, String>
{
    lap_core::t_cmds::get_grouped_file_position(params, file_id).await
}


#[tauri::command]
pub async fn get_query_file_ids(params: QueryParams) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_query_file_ids(params).await
}


#[tauri::command]
pub async fn get_library_visible_counts(
    id: Option<String>,
) -> Result<t_sqlite::LibraryVisibleCounts, String>
{
    lap_core::t_cmds::get_library_visible_counts(id).await
}


#[tauri::command]
pub async fn get_query_file_position(
    params: QueryParams,
    file_id: i64,
) -> Result<Option<i64>, String>
{
    lap_core::t_cmds::get_query_file_position(params, file_id).await
}


#[tauri::command]
pub fn list_collections() -> Result<Vec<ACollection>, String>
{
    lap_core::t_cmds::list_collections()
}


#[tauri::command]
pub fn get_collection_counts() -> Result<HashMap<i64, i64>, String>
{
    lap_core::t_cmds::get_collection_counts()
}


#[tauri::command]
pub fn create_collection(name: &str) -> Result<ACollection, String>
{
    lap_core::t_cmds::create_collection(name)
}


#[tauri::command]
pub fn rename_collection(id: i64, name: &str) -> Result<usize, String>
{
    lap_core::t_cmds::rename_collection(id, name)
}


#[tauri::command]
pub fn delete_collection(id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::delete_collection(id)
}


#[tauri::command]
pub fn reorder_collections(items: Vec<ACollectionOrder>) -> Result<usize, String>
{
    lap_core::t_cmds::reorder_collections(items)
}


#[tauri::command]
pub fn add_files_to_collection(
    collection_id: i64,
    file_ids: Vec<i64>,
) -> Result<t_write::CollectionAddResult, String>
{
    lap_core::t_cmds::add_files_to_collection(collection_id, file_ids)
}


#[tauri::command]
pub fn remove_files_from_collection(
    collection_id: i64,
    file_ids: Vec<i64>,
) -> Result<usize, String>
{
    lap_core::t_cmds::remove_files_from_collection(collection_id, file_ids)
}


#[tauri::command]
pub fn get_collection_selection_counts(
    file_ids: Vec<i64>,
) -> Result<Vec<ACollectionSelectionCount>, String>
{
    lap_core::t_cmds::get_collection_selection_counts(file_ids)
}


#[tauri::command]
pub fn clear_collection(collection_id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::clear_collection(collection_id)
}


#[tauri::command]
pub fn get_collection_file_ids(collection_id: i64) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_collection_file_ids(collection_id)
}


#[tauri::command]
pub fn get_file_collections(file_id: i64) -> Result<Vec<AFileCollection>, String>
{
    lap_core::t_cmds::get_file_collections(file_id)
}


#[tauri::command]
pub fn get_collection_count_and_sum(
    collection_id: i64,
    params: QueryParams,
) -> Result<(i64, i64), String>
{
    lap_core::t_cmds::get_collection_count_and_sum(collection_id, params)
}


#[tauri::command]
pub fn get_collection_files(
    collection_id: i64,
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<Vec<AFile>, String>
{
    lap_core::t_cmds::get_collection_files(collection_id, params, offset, limit)
}


#[tauri::command]
pub fn get_collection_grouped_query_rows(
    collection_id: i64,
    params: QueryParams,
    offset: i64,
    limit: i64,
) -> Result<GroupedQueryResult, String>
{
    lap_core::t_cmds::get_collection_grouped_query_rows(collection_id, params, offset, limit)
}


#[tauri::command]
pub fn get_collection_group_file_ids(
    collection_id: i64,
    params: QueryParams,
    group_id: String,
) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_collection_group_file_ids(collection_id, params, group_id)
}


#[tauri::command]
pub fn get_collection_query_file_ids(
    collection_id: i64,
    params: QueryParams,
) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_collection_query_file_ids(collection_id, params)
}


#[tauri::command]
pub async fn get_smart_query_count_and_sum(params: SmartQueryParams) -> Result<(i64, i64), String>
{
    lap_core::t_cmds::get_smart_query_count_and_sum(params).await
}


#[tauri::command]
pub async fn get_smart_query_time_line(params: SmartQueryParams) -> Result<Vec<ATimeLine>, String>
{
    lap_core::t_cmds::get_smart_query_time_line(params).await
}


#[tauri::command]
pub async fn get_smart_query_files(
    params: SmartQueryParams,
    offset: i64,
    limit: i64,
) -> Result<Vec<AFile>, String>
{
    lap_core::t_cmds::get_smart_query_files(params, offset, limit).await
}


#[tauri::command]
pub async fn get_smart_grouped_query_rows(
    params: SmartQueryParams,
    offset: i64,
    limit: i64,
) -> Result<GroupedQueryResult, String>
{
    lap_core::t_cmds::get_smart_grouped_query_rows(params, offset, limit).await
}


#[tauri::command]
pub async fn get_smart_group_file_ids(
    params: SmartQueryParams,
    group_id: String,
) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_smart_group_file_ids(params, group_id).await
}


#[tauri::command]
pub async fn get_smart_query_file_ids(params: SmartQueryParams) -> Result<Vec<i64>, String>
{
    lap_core::t_cmds::get_smart_query_file_ids(params).await
}


#[tauri::command]
pub async fn get_smart_query_file_position(
    params: SmartQueryParams,
    file_id: i64,
) -> Result<Option<i64>, String>
{
    lap_core::t_cmds::get_smart_query_file_position(params, file_id).await
}


#[tauri::command]
pub fn get_folder_files(
    file_type: i64,
    sort_type: i64,
    sort_order: i64,
    folder_id: i64,
    folder_path: &str,
    from_db_only: Option<bool>,
) -> (Vec<AFile>, u32, u32)
{
    lap_core::t_cmds::get_folder_files(file_type, sort_type, sort_order, folder_id, folder_path, from_db_only)
}


#[tauri::command]
pub async fn sync_album_folder_mtimes(
    album_id: i64,
    folder_id: i64,
    folder_path: String,
    group_raw_jpeg_pairs: bool,
) -> Result<lap_core::t_utils::FolderMtimeSyncResult, String>
{
    lap_core::t_cmds::sync_album_folder_mtimes(album_id, folder_id, folder_path, group_raw_jpeg_pairs).await
}


#[tauri::command]
pub async fn refresh_album_subfolders(
    album_id: i64,
    folder_path: String,
) -> Result<(), String>
{
    lap_core::t_cmds::refresh_album_subfolders(album_id, folder_path).await
}


#[tauri::command]
pub fn is_directory_accessible(path: &str) -> bool
{
    lap_core::t_cmds::is_directory_accessible(path)
}


#[tauri::command]
pub fn get_folder_thumb_count(file_type: i64, folder_id: i64) -> i64
{
    lap_core::t_cmds::get_folder_thumb_count(file_type, folder_id)
}


#[tauri::command]
pub async fn edit_image(params: t_image::EditParams) -> Result<bool, String>
{
    lap_core::t_cmds::edit_image(params).await
}


#[tauri::command]
pub async fn render_montage(params: t_montage::MontageParams) -> Result<Option<String>, String>
{
    lap_core::t_cmds::render_montage(params).await
}


#[tauri::command]
pub async fn copy_edited_image(params: t_image::EditParams) -> Result<bool, String>
{
    lap_core::t_cmds::copy_edited_image(params).await
}


#[tauri::command]
pub fn copy_file_paths(file_paths: Vec<String>) -> Result<(), String>
{
    lap_core::t_cmds::copy_file_paths(file_paths)
}


#[tauri::command]
pub async fn preview_batch_rename(
    library_id: String,
    file_ids: Vec<i64>,
    options: lap_core::t_batch_rename::RenameOptions,
) -> Result<lap_core::t_batch_rename::RenamePreview, String>
{
    lap_core::t_cmds::preview_batch_rename(library_id, file_ids, options).await
}


#[tauri::command]
pub async fn batch_rename_files(
    library_id: String,
    file_ids: Vec<i64>,
    options: lap_core::t_batch_rename::RenameOptions,
    expected: Vec<lap_core::t_batch_rename::RenameItem>,
) -> Result<lap_core::t_batch_rename::RenamePreview, String>
{
    lap_core::t_cmds::batch_rename_files(library_id, file_ids, options, expected).await
}


#[tauri::command]
pub fn rename_file(
    file_id: i64,
    file_path: &str,
    new_name: &str,
) -> Result<Option<String>, String>
{
    lap_core::t_cmds::rename_file(file_id, file_path, new_name)
}


#[tauri::command]
pub fn move_file(
    file_id: i64,
    file_path: &str,
    new_folder_id: i64,
    new_folder_path: &str,
    conflict_policy: &str,
    batch_id: Option<u64>,
) -> Result<String, String>
{
    lap_core::t_cmds::move_file(file_id, file_path, new_folder_id, new_folder_path, conflict_policy, batch_id)
}


#[tauri::command]
pub fn begin_file_transfer_batch() -> Result<u64, String>
{
    lap_core::t_cmds::begin_file_transfer_batch()
}


#[tauri::command]
pub fn end_file_transfer_batch(batch_id: u64)
{
    lap_core::t_cmds::end_file_transfer_batch(batch_id)
}


#[tauri::command]
pub fn move_file_outside_library(
    file_id: i64,
    file_path: &str,
    new_folder_path: &str,
    conflict_policy: &str,
    batch_id: Option<u64>,
) -> Result<String, String>
{
    lap_core::t_cmds::move_file_outside_library(file_id, file_path, new_folder_path, conflict_policy, batch_id)
}


#[tauri::command]
pub fn copy_file(
    file_path: &str,
    new_folder_path: &str,
    conflict_policy: &str,
    batch_id: Option<u64>,
) -> Result<String, String>
{
    lap_core::t_cmds::copy_file(file_path, new_folder_path, conflict_policy, batch_id)
}


#[tauri::command]
pub async fn import_file(
    file_path: String,
    folder_id: i64,
    folder_path: String,
    library_id: Option<String>,
) -> Result<Option<AFile>, String>
{
    lap_core::t_cmds::import_file(file_path, folder_id, folder_path, library_id).await
}


#[tauri::command]
pub fn import_and_organize(
    album_id: i64,
    source_path: String,
    destination_path: String,
    layout: String,
    completed_paths: Vec<String>,
) -> Result<(), String>
{
    lap_core::t_cmds::import_and_organize(album_id, source_path, destination_path, layout, completed_paths)
}


#[tauri::command]
pub fn cancel_import_and_organize() -> Result<(), String>
{
    lap_core::t_cmds::cancel_import_and_organize()
}


#[tauri::command]
pub async fn import_url(
    url: &str,
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String>
{
    lap_core::t_cmds::import_url(url, folder_id, folder_path).await
}


#[tauri::command]
pub async fn import_file_bytes(
    bytes: Vec<u8>,
    name: String,
    folder_id: i64,
    folder_path: String,
) -> Result<Option<AFile>, String>
{
    lap_core::t_cmds::import_file_bytes(bytes, name, folder_id, folder_path).await
}


#[tauri::command]
pub fn delete_file(file_id: i64, file_path: &str) -> Result<BatchDeleteResult, String>
{
    lap_core::t_cmds::delete_file(file_id, file_path)
}


#[tauri::command]
pub fn delete_file_permanently(file_id: i64, file_path: &str) -> Result<BatchDeleteResult, String>
{
    lap_core::t_cmds::delete_file_permanently(file_id, file_path)
}


#[tauri::command]
pub fn delete_db_file(file_id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::delete_db_file(file_id)
}


#[tauri::command]
pub async fn batch_delete_files(
    files: Vec<BatchDeleteFile>,
    permanently: bool,
) -> Result<BatchDeleteResult, String>
{
    lap_core::t_cmds::batch_delete_files(files, permanently).await
}


#[tauri::command]
pub fn edit_file_comment(file_id: i64, comment: &str) -> Result<usize, String>
{
    lap_core::t_cmds::edit_file_comment(file_id, comment)
}


#[tauri::command]
pub async fn clean_unused_thumbnail_cache(
    library_id: Option<String>,
) -> Result<t_sqlite::ThumbnailCacheCleanupResult, String>
{
    lap_core::t_cmds::clean_unused_thumbnail_cache(library_id).await
}


#[tauri::command]
pub async fn get_file_thumb(
    file_id: i64,
    file_path: &str,
    file_type: i64,
    orientation: i32,
    thumbnail_size: u32,
    raw_display_options: RawDisplayOptions,
    force_regenerate: bool,
    thumbnail_seek_percent: Option<u8>,
    album_id: Option<i64>,
) -> Result<Option<AThumb>, String>
{
    lap_core::t_cmds::get_file_thumb(file_id, file_path, file_type, orientation, thumbnail_size, raw_display_options, force_regenerate, thumbnail_seek_percent, album_id).await
}


#[tauri::command]
pub async fn get_file_thumb_by_id(
    file_id: i64,
    thumbnail_size: u32,
    raw_display_options: RawDisplayOptions,
    force_regenerate: bool,
) -> Result<Option<AThumb>, String>
{
    lap_core::t_cmds::get_file_thumb_by_id(file_id, thumbnail_size, raw_display_options, force_regenerate).await
}


#[tauri::command]
pub async fn get_file_thumbs(
    files: Vec<ThumbRequest>,
    thumbnail_size: u32,
    raw_display_options: RawDisplayOptions,
    force_regenerate: bool,
) -> Result<Vec<Option<AThumb>>, String>
{
    lap_core::t_cmds::get_file_thumbs(files, thumbnail_size, raw_display_options, force_regenerate).await
}


#[tauri::command]
pub fn get_file_info(file_id: i64) -> Result<Option<AFile>, String>
{
    lap_core::t_cmds::get_file_info(file_id)
}


#[tauri::command]
pub async fn get_file_metadata_details(
    file_id: i64,
    section: String,
    request_id: String,
    force: Option<bool>,
    refresh_key: Option<String>,
) -> Result<lap_core::metadata::embedded::Details, String>
{
    lap_core::t_cmds::get_file_metadata_details(file_id, section, request_id, force, refresh_key).await
}


#[tauri::command]
pub fn cancel_file_metadata_requests(request_ids: Vec<String>) -> Result<(), String>
{
    lap_core::t_cmds::cancel_file_metadata_requests(request_ids)
}


#[tauri::command]
pub fn prepare_motion_photo_video(file_id: i64) -> Result<String, String>
{
    lap_core::t_cmds::prepare_motion_photo_video(file_id)
}


#[tauri::command]
pub fn update_file_info(file_id: i64, file_path: &str) -> Result<Option<AFile>, String>
{
    lap_core::t_cmds::update_file_info(file_id, file_path)
}


#[tauri::command]
pub fn begin_file_refresh_batch(library_id: String) -> Result<u64, String>
{
    lap_core::t_cmds::begin_file_refresh_batch(library_id)
}


#[tauri::command]
pub fn end_file_refresh_batch(batch_id: u64)
{
    lap_core::t_cmds::end_file_refresh_batch(batch_id)
}


#[tauri::command]
pub async fn change_video_thumbnail(
    library_id: String,
    file_id: i64,
    thumbnail_size: u32,
    seek_percent: u8,
) -> Result<Option<AThumb>, String>
{
    lap_core::t_cmds::change_video_thumbnail(library_id, file_id, thumbnail_size, seek_percent).await
}


#[tauri::command]
pub async fn refresh_selected_file_info(
    library_id: String,
    file_id: i64,
    batch_id: Option<u64>,
    thumbnail_size: Option<u32>,
    raw_display_options: Option<RawDisplayOptions>,
) -> Result<Option<FileRefreshResult>, String>
{
    lap_core::t_cmds::refresh_selected_file_info(library_id, file_id, batch_id, thumbnail_size, raw_display_options).await
}


#[tauri::command]
pub async fn index_cloud_file(
    library_id: String,
    batch_id: Option<u64>,
    file_id: i64,
    thumbnail_size: u32,
    raw_display_options: RawDisplayOptions,
) -> Result<Option<FileRefreshResult>, String>
{
    lap_core::t_cmds::index_cloud_file(library_id, batch_id, file_id, thumbnail_size, raw_display_options).await
}


#[tauri::command]
pub fn add_file_to_db(folder_id: i64, file_path: &str) -> Result<Option<AFile>, String>
{
    lap_core::t_cmds::add_file_to_db(folder_id, file_path)
}


#[tauri::command]
pub fn check_file_exists(file_path: &str) -> bool
{
    lap_core::t_cmds::check_file_exists(file_path)
}


#[tauri::command]
pub async fn is_cloud_only_file(file_path: String) -> bool
{
    lap_core::t_cmds::is_cloud_only_file(file_path).await
}


#[tauri::command]
pub async fn check_file_accessibility(file_path: String) -> bool
{
    lap_core::t_cmds::check_file_accessibility(file_path).await
}


#[tauri::command]
pub fn set_file_rotate(file_id: i64, rotate: i32) -> Result<usize, String>
{
    lap_core::t_cmds::set_file_rotate(file_id, rotate)
}


#[tauri::command]
pub fn get_file_has_tags(file_id: i64) -> Result<bool, String>
{
    lap_core::t_cmds::get_file_has_tags(file_id)
}


#[tauri::command]
pub fn get_favorite_folders() -> Result<Vec<AFolder>, String>
{
    lap_core::t_cmds::get_favorite_folders()
}


#[tauri::command]
pub fn get_folder_favorite(folder_path: &str) -> Result<bool, String>
{
    lap_core::t_cmds::get_folder_favorite(folder_path)
}


#[tauri::command]
pub fn set_folder_favorite(folder_id: i64, is_favorite: bool) -> Result<usize, String>
{
    lap_core::t_cmds::set_folder_favorite(folder_id, is_favorite)
}


#[tauri::command]
pub fn get_folder_search_excluded(folder_path: &str) -> Result<bool, String>
{
    lap_core::t_cmds::get_folder_search_excluded(folder_path)
}


#[tauri::command]
pub fn set_folder_search_excluded(
    album_id: i64,
    folder_path: &str,
    is_excluded: bool,
) -> Result<usize, String>
{
    lap_core::t_cmds::set_folder_search_excluded(album_id, folder_path, is_excluded)
}


#[tauri::command]
pub fn set_file_favorite(file_id: i64, is_favorite: bool) -> Result<usize, String>
{
    lap_core::t_cmds::set_file_favorite(file_id, is_favorite)
}


#[tauri::command]
pub fn set_file_rating(file_id: i64, rating: i32) -> Result<usize, String>
{
    lap_core::t_cmds::set_file_rating(file_id, rating)
}


#[tauri::command]
pub fn set_file_culling_flag(file_id: i64, culling_flag: i32) -> Result<usize, String>
{
    lap_core::t_cmds::set_file_culling_flag(file_id, culling_flag)
}


#[tauri::command]
pub fn batch_update_file_metadata(
    params: t_write::BatchFileMetadataUpdate,
) -> Result<usize, String>
{
    lap_core::t_cmds::batch_update_file_metadata(params)
}


#[tauri::command]
pub fn get_xmp_import_settings() -> XmpImportSettings
{
    lap_core::t_cmds::get_xmp_import_settings()
}


#[tauri::command]
pub fn set_xmp_import_settings(import_xmp_sidecars: bool)
{
    lap_core::t_cmds::set_xmp_import_settings(import_xmp_sidecars)
}


#[tauri::command]
pub async fn save_xmp_metadata(
    library_id: String,
    file_ids: Vec<i64>,
) -> Result<lap_core::metadata::xmp::BatchResult, String>
{
    lap_core::t_cmds::save_xmp_metadata(library_id, file_ids).await
}


#[tauri::command]
pub fn get_tag_subtree_info(tag_id: i64) -> Result<lap_core::t_tags::SubtreeInfo, String>
{
    lap_core::t_cmds::get_tag_subtree_info(tag_id)
}


#[tauri::command]
pub fn move_tag(tag_id: i64, parent_id: Option<i64>) -> Result<(), String>
{
    lap_core::t_cmds::move_tag(tag_id, parent_id)
}


#[tauri::command]
pub fn get_all_tags(sort: i64) -> Result<Vec<ATag>, String>
{
    lap_core::t_cmds::get_all_tags(sort)
}


#[tauri::command]
pub fn get_tag_counts() -> Result<HashMap<i64, i64>, String>
{
    lap_core::t_cmds::get_tag_counts()
}


#[tauri::command]
pub fn get_tag_name(tag_id: i64) -> Result<String, String>
{
    lap_core::t_cmds::get_tag_name(tag_id)
}


#[tauri::command]
pub fn create_tag(name: &str, parent_id: Option<i64>) -> Result<ATag, String>
{
    lap_core::t_cmds::create_tag(name, parent_id)
}


#[tauri::command]
pub fn rename_tag(tag_id: i64, new_name: &str) -> Result<usize, String>
{
    lap_core::t_cmds::rename_tag(tag_id, new_name)
}


#[tauri::command]
pub fn delete_tag(tag_id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::delete_tag(tag_id)
}


#[tauri::command]
pub fn get_tags_for_file(file_id: i64) -> Result<Vec<ATag>, String>
{
    lap_core::t_cmds::get_tags_for_file(file_id)
}


#[tauri::command]
pub fn add_tag_to_file(file_id: i64, tag_id: i64) -> Result<(), String>
{
    lap_core::t_cmds::add_tag_to_file(file_id, tag_id)
}


#[tauri::command]
pub fn remove_tag_from_file(file_id: i64, tag_id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::remove_tag_from_file(file_id, tag_id)
}


#[tauri::command]
pub fn get_tag_selection_counts(file_ids: Vec<i64>) -> Result<Vec<ATagSelectionCount>, String>
{
    lap_core::t_cmds::get_tag_selection_counts(file_ids)
}


#[tauri::command]
pub fn apply_tags_to_files(
    file_ids: Vec<i64>,
    add_tag_ids: Vec<i64>,
    remove_tag_ids: Vec<i64>,
) -> Result<Vec<ATagFileState>, String>
{
    lap_core::t_cmds::apply_tags_to_files(file_ids, add_tag_ids, remove_tag_ids)
}


#[tauri::command]
pub fn get_taken_dates(sort: i64) -> Result<Vec<(String, i64)>, String>
{
    lap_core::t_cmds::get_taken_dates(sort)
}


#[tauri::command]
pub fn get_camera_info(sort: i64) -> Result<Vec<ACamera>, String>
{
    lap_core::t_cmds::get_camera_info(sort)
}


#[tauri::command]
pub fn get_lens_info(sort: i64) -> Result<Vec<ALens>, String>
{
    lap_core::t_cmds::get_lens_info(sort)
}


#[tauri::command]
pub fn get_location_info(sort: i64) -> Result<Vec<ALocation>, String>
{
    lap_core::t_cmds::get_location_info(sort)
}


#[tauri::command]
pub fn get_gps_map_points(
    params: t_sqlite::QueryParams,
) -> Result<Vec<t_sqlite::AGpsMapPoint>, String>
{
    lap_core::t_cmds::get_gps_map_points(params)
}


#[tauri::command]
pub fn get_package_info() -> t_utils::PackageInfo
{
    lap_core::t_cmds::get_package_info()
}


#[tauri::command]
pub fn get_build_time() -> u64
{
    lap_core::t_cmds::get_build_time()
}


#[tauri::command]
pub fn get_storage_file_info() -> Result<t_utils::FileInfo, String>
{
    lap_core::t_cmds::get_storage_file_info()
}


#[tauri::command]
pub fn check_ai_status() -> String
{
    lap_core::t_cmds::check_ai_status()
}


#[tauri::command]
pub async fn get_image_search_model_status() -> Result<t_ai::ImageSearchModelStatus, String>
{
    lap_core::t_cmds::get_image_search_model_status().await
}


#[tauri::command]
pub async fn set_image_search_model(
    model: i64,
) -> Result<t_ai::ImageSearchModelStatus, String>
{
    lap_core::t_cmds::set_image_search_model(model).await
}


#[tauri::command]
pub async fn download_multilingual_image_search_model() -> Result<(), String>
{
    lap_core::t_cmds::download_multilingual_image_search_model().await
}


#[tauri::command]
pub async fn cancel_multilingual_image_search_model_download() -> Result<(), String>
{
    lap_core::t_cmds::cancel_multilingual_image_search_model_download().await
}


#[tauri::command]
pub async fn generate_embedding(
    file_id: i64,
    library_id: Option<String>,
) -> Result<String, String>
{
    lap_core::t_cmds::generate_embedding(file_id, library_id).await
}


#[tauri::command]
pub async fn search_similar_images(
    params: ImageSearchParams,
    library_id: Option<String>,
) -> Result<Vec<AFile>, String>
{
    lap_core::t_cmds::search_similar_images(params, library_id).await
}


#[tauri::command]
pub fn similar_start_scan(
    scope_key: String,
    source_version: i64,
    similarity_threshold: f32,
    params: Option<QueryParams>,
    collection_id: Option<i64>,
    file_ids: Option<Vec<i64>>,
    keep_rules: Option<lap_core::t_dedup::KeepRules>,
) -> Result<(), String>
{
    lap_core::t_cmds::similar_start_scan(scope_key, source_version, similarity_threshold, params, collection_id, file_ids, keep_rules)
}


#[tauri::command]
pub fn similar_get_scan_status() -> t_similar::SimilarScanStatus
{
    lap_core::t_cmds::similar_get_scan_status()
}


#[tauri::command]
pub fn similar_cancel_scan()
{
    lap_core::t_cmds::similar_cancel_scan()
}


#[tauri::command]
pub fn similar_get_eligible_count(
    params: Option<QueryParams>,
    collection_id: Option<i64>,
    file_ids: Option<Vec<i64>>,
) -> Result<u64, String>
{
    lap_core::t_cmds::similar_get_eligible_count(params, collection_id, file_ids)
}


#[tauri::command]
pub fn similar_list_groups(
    scope_key: String,
    limit: i64,
    offset: i64,
) -> Result<serde_json::Value, String>
{
    lap_core::t_cmds::similar_list_groups(scope_key, limit, offset)
}


#[tauri::command]
pub fn similar_get_overview(scope_key: String) -> Result<serde_json::Value, String>
{
    lap_core::t_cmds::similar_get_overview(scope_key)
}


#[tauri::command]
pub fn similar_get_group(group_id: i64, scope_key: String) -> Result<serde_json::Value, String>
{
    lap_core::t_cmds::similar_get_group(group_id, scope_key)
}


#[tauri::command]
pub fn similar_set_keep(group_id: i64, file_id: i64, scope_key: String) -> Result<(), String>
{
    lap_core::t_cmds::similar_set_keep(group_id, file_id, scope_key)
}


#[tauri::command]
pub fn similar_has_scan(scope_key: String) -> Result<bool, String>
{
    lap_core::t_cmds::similar_has_scan(scope_key)
}


#[tauri::command]
pub fn index_faces(cluster_epsilon: Option<f32>) -> Result<(), String>
{
    lap_core::t_cmds::index_faces(cluster_epsilon)
}


#[tauri::command]
pub fn get_face_stats() -> Result<t_face::FaceStats, String>
{
    lap_core::t_cmds::get_face_stats()
}


#[tauri::command]
pub fn cancel_face_index() -> Result<(), String>
{
    lap_core::t_cmds::cancel_face_index()
}


#[tauri::command]
pub fn reset_faces() -> Result<(), String>
{
    lap_core::t_cmds::reset_faces()
}


#[tauri::command]
pub fn is_face_indexing() -> Result<(bool, Option<t_face::FaceIndexProgress>), String>
{
    lap_core::t_cmds::is_face_indexing()
}


#[tauri::command]
pub fn get_persons(sort: i64) -> Result<Vec<Person>, String>
{
    lap_core::t_cmds::get_persons(sort)
}


#[tauri::command]
pub fn get_persons_page(request: PersonPageRequest) -> Result<PersonPage, String>
{
    lap_core::t_cmds::get_persons_page(request)
}


#[tauri::command]
pub fn rename_person(person_id: i64, name: String) -> Result<usize, String>
{
    lap_core::t_cmds::rename_person(person_id, name)
}


#[tauri::command]
pub fn delete_person(person_id: i64) -> Result<usize, String>
{
    lap_core::t_cmds::delete_person(person_id)
}


#[tauri::command]
pub fn get_faces_for_file(file_id: i64) -> Result<Vec<t_sqlite::Face>, String>
{
    lap_core::t_cmds::get_faces_for_file(file_id)
}


#[tauri::command]
pub fn get_person_thumbnail(person_id: i64) -> Result<Option<String>, String>
{
    lap_core::t_cmds::get_person_thumbnail(person_id)
}


#[tauri::command]
pub fn dedup_start_scan(
    params: Option<lap_core::t_sqlite::QueryParams>,
    collection_id: Option<i64>,
    file_ids: Option<Vec<i64>>,
    keep_rules: Option<lap_core::t_dedup::KeepRules>,
    group_raw_pairs: Option<bool>,
) -> Result<(), String>
{
    lap_core::t_cmds::dedup_start_scan(params, collection_id, file_ids, keep_rules, group_raw_pairs)
}


#[tauri::command]
pub fn dedup_get_scan_status() -> Result<lap_core::t_dedup::DedupScanStatus, String>
{
    lap_core::t_cmds::dedup_get_scan_status()
}


#[tauri::command]
pub fn dedup_cancel_scan() -> Result<(), String>
{
    lap_core::t_cmds::dedup_cancel_scan()
}


#[tauri::command]
pub fn dedup_list_groups(
    page: u32,
    page_size: u32,
    sort_by: String, // E.g., "size_desc", "count_desc"
    filter: String,  // E.g., "all", "unreviewed"
) -> Result<Vec<lap_core::t_dedup::DedupGroup>, String>
{
    lap_core::t_cmds::dedup_list_groups(page, page_size, sort_by, filter)
}


#[tauri::command]
pub fn dedup_get_overview() -> Result<lap_core::t_dedup::DedupOverview, String>
{
    lap_core::t_cmds::dedup_get_overview()
}


#[tauri::command]
pub fn dedup_set_keep(
    group_id: i64,
    file_id: i64,
    group_raw_pairs: Option<bool>,
) -> Result<(), String>
{
    lap_core::t_cmds::dedup_set_keep(group_id, file_id, group_raw_pairs)
}


#[tauri::command]
pub async fn dedup_delete(
    request: lap_core::t_dedup::DedupDeleteRequest,
) -> Result<lap_core::t_dedup::DedupDeleteResult, String>
{
    lap_core::t_cmds::dedup_delete(request).await
}


#[tauri::command]
pub fn get_db_storage_info() -> Result<Vec<t_storage::DbStorageInfo>, String>
{
    lap_core::t_cmds::get_db_storage_info()
}


#[tauri::command]
pub fn backup_databases(
    library_ids: Vec<String>,
    dest_path: String,
) -> Result<t_storage::BackupResult, String>
{
    lap_core::t_cmds::backup_databases(library_ids, dest_path)
}


#[tauri::command]
pub fn parse_backup_file(path: String) -> Result<t_storage::BackupMetaData, String>
{
    lap_core::t_cmds::parse_backup_file(path)
}


#[tauri::command]
pub fn restore_databases(
    backup_path: String,
    selections: Vec<t_storage::RestoreSelection>,
) -> Result<t_storage::RestoreResult, String>
{
    lap_core::t_cmds::restore_databases(backup_path, selections)
}
