/// Cross‑platform drag‑drop image URL extraction.
///
/// - **macOS**: reads `NSPasteboardNameDrag` which is scoped to the
///   current drag operation and never returns stale data.
/// - **Windows / Linux**: falls back to the system clipboard.  During
///   a browser drag the image URL is normally placed on the clipboard
///   as well, so checking it immediately after the drop event gives
///   the correct URL.  The read is gated behind an `http(s)://` prefix
///   check to reject irrelevant clipboard text.

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::{CStr, c_char};

    unsafe extern "C" {
        fn lap_get_drag_image_url() -> *const c_char;
        fn lap_get_drag_file_paths() -> *const c_char;
        fn lap_free_string(ptr: *const c_char);
    }

    fn read_string(ptr: *const c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let url = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().to_string();
        unsafe { lap_free_string(ptr) };
        Some(url)
    }

    pub fn get_drag_image_url() -> Option<String> {
        read_string(unsafe { lap_get_drag_image_url() })
    }

    pub fn get_drag_file_paths() -> Vec<String> {
        read_string(unsafe { lap_get_drag_file_paths() })
            .and_then(|value| serde_json::from_str(&value).ok())
            .unwrap_or_default()
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn get_drag_image_url() -> Option<String> {
        // Browsers on Windows put the image URL on the system clipboard
        // during a drag, so reading it right after the drop is reliable.
        let mut clipboard = arboard::Clipboard::new().ok()?;
        let text = clipboard.get_text().ok()?;
        let text = text.trim();
        if text.starts_with("http://") || text.starts_with("https://") {
            Some(text.to_string())
        } else {
            None
        }
    }

    pub fn get_drag_file_paths() -> Vec<String> {
        Vec::new()
    }
}

pub fn get_drag_image_url() -> Option<String> {
    imp::get_drag_image_url()
}

pub fn get_drag_file_paths() -> Vec<String> {
    imp::get_drag_file_paths()
}

#[cfg(target_os = "macos")]
pub async fn copy_files_and_image(
    _app_handle: &tauri::AppHandle,
    file_paths: &[String],
    image_bytes: Option<&[u8]>,
) -> Result<(), String> {
    use std::ffi::{CString, c_char, c_uchar};

    unsafe extern "C" {
        fn lap_copy_files_and_image_to_clipboard(
            file_paths_json: *const c_char,
            image_bytes: *const c_uchar,
            image_length: usize,
        ) -> bool;
    }

    let file_paths_json = serde_json::to_string(file_paths)
        .map_err(|e| format!("Failed to serialize clipboard file paths: {}", e))?;
    let file_paths_json = CString::new(file_paths_json)
        .map_err(|_| "Clipboard file paths contain a null byte".to_string())?;
    let (image_ptr, image_len) = image_bytes
        .map(|bytes| (bytes.as_ptr(), bytes.len()))
        .unwrap_or((std::ptr::null(), 0));
    let success = unsafe {
        lap_copy_files_and_image_to_clipboard(file_paths_json.as_ptr(), image_ptr, image_len)
    };
    if success {
        Ok(())
    } else {
        Err("Failed to write file and image to clipboard".to_string())
    }
}

#[cfg(target_os = "windows")]
pub async fn copy_files_and_image(
    _app_handle: &tauri::AppHandle,
    file_paths: &[String],
    image_bytes: Option<&[u8]>,
) -> Result<(), String> {
    use clipboard_win::{Clipboard, Setter, formats::FileList};

    let _clipboard =
        Clipboard::new_attempts(10).map_err(|e| format!("Failed to open clipboard: {}", e))?;
    clipboard_win::raw::empty().map_err(|e| format!("Failed to clear clipboard: {}", e))?;

    FileList
        .write_clipboard(file_paths)
        .map_err(|e| format!("Failed to copy files to clipboard: {}", e))?;

    if let Some(bytes) = image_bytes {
        let png_format = clipboard_win::register_format("PNG")
            .ok_or_else(|| "Failed to register PNG clipboard format".to_string())?;
        clipboard_win::raw::set_without_clear(png_format.get(), bytes)
            .map_err(|e| format!("Failed to copy image preview to clipboard: {}", e))?;
    }

    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn copy_files_and_image(
    app_handle: &tauri::AppHandle,
    file_paths: &[String],
    image_bytes: Option<&[u8]>,
) -> Result<(), String> {
    use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
    use std::ffi::c_uchar;

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

    unsafe extern "C" {
        fn lap_copy_files_and_image_to_clipboard(
            uri_list: *const c_uchar,
            uri_list_len: usize,
            png: *const c_uchar,
            png_len: usize,
        ) -> bool;
    }

    let uri_list = file_paths
        .iter()
        .filter_map(|path| std::fs::canonicalize(path).ok())
        .filter_map(|path| {
            path.to_str().map(|path| {
                format!(
                    "file://{}\r\n",
                    utf8_percent_encode(path, URI_PATH_ENCODE_SET),
                )
            })
        })
        .collect::<String>()
        .into_bytes();
    if uri_list.is_empty() {
        return Err("No valid files to copy".to_string());
    }
    let image_bytes = image_bytes.map(Vec::from);
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app_handle
        .run_on_main_thread(move || {
            let (image_ptr, image_len) = image_bytes
                .as_deref()
                .map(|bytes| (bytes.as_ptr(), bytes.len()))
                .unwrap_or((std::ptr::null(), 0));
            let success = unsafe {
                lap_copy_files_and_image_to_clipboard(
                    uri_list.as_ptr(),
                    uri_list.len(),
                    image_ptr,
                    image_len,
                )
            };
            let _ = sender.send(success);
        })
        .map_err(|e| format!("Failed to schedule clipboard update: {}", e))?;
    match receiver.await {
        Ok(true) => Ok(()),
        Ok(false) => Err("Failed to write files and image to clipboard".to_string()),
        Err(_) => Err("Clipboard update was cancelled".to_string()),
    }
}

#[cfg(target_os = "linux")]
fn get_clipboard_file_paths_sync() -> Vec<String> {
    use std::ffi::c_void;

    unsafe extern "C" {
        fn lap_get_clipboard_file_paths(out_len: *mut usize) -> *mut u8;
        fn lap_clipboard_free(ptr: *mut c_void);
    }

    let mut len = 0usize;
    let ptr = unsafe { lap_get_clipboard_file_paths(&mut len) };
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }

    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { lap_clipboard_free(ptr.cast()) };

    String::from_utf8(bytes)
        .ok()
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn get_clipboard_png_sync() -> Option<Vec<u8>> {
    use std::ffi::c_void;

    unsafe extern "C" {
        fn lap_get_clipboard_png(out_len: *mut usize) -> *mut u8;
        fn lap_clipboard_free(ptr: *mut c_void);
    }

    let mut len = 0usize;
    let ptr = unsafe { lap_get_clipboard_png(&mut len) };
    if ptr.is_null() || len == 0 {
        return None;
    }

    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec();
    unsafe { lap_clipboard_free(ptr.cast()) };
    Some(bytes)
}

#[cfg(target_os = "linux")]
pub struct ClipboardImportData {
    pub file_paths: Vec<String>,
    pub png: Option<Vec<u8>>,
}

#[cfg(target_os = "linux")]
pub async fn get_clipboard_import_data(
    app_handle: &tauri::AppHandle,
) -> Result<ClipboardImportData, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app_handle
        .run_on_main_thread(move || {
            let payload = ClipboardImportData {
                file_paths: get_clipboard_file_paths_sync(),
                png: get_clipboard_png_sync(),
            };
            let _ = sender.send(payload);
        })
        .map_err(|e| format!("Failed to schedule clipboard read: {}", e))?;

    receiver
        .await
        .map_err(|_| "Clipboard read was cancelled".to_string())
}

/// Copy files (plus an optional preview image) to the OS clipboard.
///
/// Lives in the shell: only the pasteboard needs the `AppHandle` (Linux
/// marshals the native clipboard call onto the main thread); the preview
/// bytes come from the core's image pipeline.
pub async fn copy_files_to_clipboard(
    app_handle: &tauri::AppHandle,
    file_paths: Vec<String>,
    files_only: bool,
) -> Result<usize, String> {
    let file_paths = file_paths
        .into_iter()
        // The UI limits copying to 10 content items. RAW+JPEG pairs expand
        // one item into two physical files, so retain both components.
        .take(20)
        .collect::<Vec<_>>();
    if file_paths.is_empty() {
        return Err("No valid files to copy".to_string());
    }
    // Do not silently copy only part of a RAW pair or multi-file selection.
    if let Some(path) = file_paths
        .iter()
        .find(|path| !std::path::Path::new(path).is_file())
    {
        return Err(format!("File is unavailable: {}", path));
    }
    // A single-image representation lets paste targets pick pixels instead of
    // the file list. Copy a complete RAW pair as files only.
    let preview = if files_only {
        None
    } else {
        lappp_core::t_image::clipboard_preview_png(&file_paths).await
    };
    copy_files_and_image(app_handle, &file_paths, preview.as_deref()).await?;
    Ok(file_paths.len())
}
