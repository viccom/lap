// Prevents additional console window on Windows, DO NOT REMOVE!!
// The console is opt-in via the `console` feature: the default build (debug
// included) uses the GUI subsystem so double-clicking the exe does not flash
// a black console window; build with `--features console` to keep stdout /
// panic output visible during development.
#![cfg_attr(
    all(target_os = "windows", not(feature = "console")),
    windows_subsystem = "windows"
)]

/**
 * Main entry point for the Tauri application.
 * project: lappp
 * author:  julyx10
 * date:    2024-08-08
 */
use tauri::Manager;
#[cfg(feature = "embedded")]
use tauri_plugin_aptabase::EventTracker;

// The core lives in lappp-core; importing the module names keeps the
// bare `t_x::…` paths below unchanged. (t_http is Linux-only.) The thin
// client build (`--features client`) does not link lappp-core, so all of this —
// and every module that depends on it — is gated to `embedded`.
#[cfg(feature = "embedded")]
use lappp_core::{
    t_config,
    t_events,
    t_paths,
    t_sqlite,
    t_state,
};
#[cfg(all(feature = "embedded", target_os = "linux"))]
use lappp_core::t_http;


#[cfg(feature = "embedded")]
mod t_client;
#[cfg(feature = "embedded")]
mod t_cmds;
#[cfg(feature = "embedded")]
mod t_drag;
#[cfg(feature = "embedded")]
mod t_embed;
#[cfg(feature = "embedded")]
mod t_fts;
#[cfg(feature = "embedded")]
mod t_af_area;
// The menu module is Tauri-only (no lappp-core), so it compiles in both builds.
mod t_menu;
#[cfg(feature = "client")]
mod client_cmds;
#[cfg(feature = "client")]
mod servers;
#[cfg(feature = "embedded")]
mod t_pasteboard;
#[cfg(feature = "embedded")]
mod t_protocol;
#[cfg(feature = "embedded")]
mod t_scopes;
#[cfg(feature = "embedded")]
mod t_startup;
#[cfg(feature = "embedded")]
mod t_video;
#[cfg(feature = "embedded")]
mod t_vl;
#[cfg(feature = "embedded")]
mod t_vl_schedule;

/// Desktop adapter for the process-level event sink: forwards emitted events
/// to the Tauri shell. Lives here (not in `t_events`) so the sink module stays
/// Tauri-free and the same core can later run under `lapd` with an SSE sink.
#[cfg(feature = "embedded")]
struct TauriEventSink(tauri::AppHandle);

#[cfg(feature = "embedded")]
impl t_events::EventSink for TauriEventSink {
    fn emit(&self, name: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.0.emit(name, payload);
    }
}

/// Runtime console opt-in for GUI-subsystem builds: setting `LAP_CONSOLE=1`
/// (or `true`) in the environment allocates a console window and rewires
/// stdout/stderr to it, so a shipped portable exe can show startup logs /
/// panic output on demand without a rebuild. Default (variable unset) keeps
/// the process windowless. Compiled out when the console subsystem itself is
/// enabled via the `console` feature (the loader already made one).
#[cfg(all(target_os = "windows", not(feature = "console")))]
fn enable_console_if_requested() {
    let requested = std::env::var_os("LAP_CONSOLE")
        .map(|v| v == std::ffi::OsStr::new("1") || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !requested {
        return;
    }
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        AllocConsole, SetConsoleOutputCP, SetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        if AllocConsole().is_err() {
            return;
        }
        // UTF-8 output so the startup logs / panic text render correctly.
        let _ = SetConsoleOutputCP(65001);
        let conout: Vec<u16> = "CONOUT$\0".encode_utf16().collect();
        // Rebind the std handles to the new console explicitly: AllocConsole
        // only fills in handles that are currently invalid, and inherited
        // pipes would otherwise keep swallowing the log output.
        if let Ok(handle) = CreateFileW(
            PCWSTR(conout.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        ) {
            let _ = SetStdHandle(STD_OUTPUT_HANDLE, handle);
            let _ = SetStdHandle(STD_ERROR_HANDLE, handle);
        }
    }
}

/// The main function is the entry point for the Tauri application.
#[tokio::main]
async fn main() {
    #[cfg(all(target_os = "windows", not(feature = "console")))]
    enable_console_if_requested();
    std::panic::set_hook(Box::new(|panic_info| {
        eprintln!("Unhandled panic: {}", panic_info);
    }));

    #[cfg(feature = "embedded")]
    run_embedded().await;

    #[cfg(all(feature = "client", not(feature = "embedded")))]
    run_client().await;
}

/// Full desktop app: links the backend, opens the library, and registers every
/// local Tauri command. This is the default build and is unchanged from before
/// the embedded/client split.
#[cfg(feature = "embedded")]
async fn run_embedded() {
    // Pin process-level state before anything reads it, so timing state (like
    // StartupState::started) reflects process start (see t_state).
    t_state::init_process_states();

    // Two processes must never write one library (risk R2, plan §3.7): if a
    // live `lapd` owns the data directory we would open, refuse to start
    // rather than race it for the SQLite files. This desktop build is the
    // full/embedded app; it never connects to a server itself.
    let args: Vec<String> = std::env::args().collect();
    // Optional isolated data directory (mirrors `lapd --data-dir`): applied
    // before anything reads the data path, so the lock probe below and the
    // embedded boot both resolve against it. Absent → the per-user default.
    if let Some(dir) = t_client::data_dir_from_args(&args) {
        eprintln!("lappp: data dir → {}", dir.display());
        t_config::set_data_dir_override(dir);
    }
    if let Some(addr) = t_config::get_app_data_dir()
        .ok()
        .and_then(|dir| t_client::conflicting_server(&dir, std::process::id()))
    {
        eprintln!(
            "lappp: another instance is using this library (a lappp-server is running at {addr}).\n\
             Close it, or use the Web client / thin-client build to connect to it."
        );
        std::process::exit(1);
    }

    let builder = tauri::Builder::default();
    let builder = t_protocol::register_protocols(builder);

    let aptabase_enabled = option_env!("APTABASE_KEY")
        .filter(|k| !k.is_empty())
        .is_some();

    let builder = match option_env!("APTABASE_KEY").filter(|k| !k.is_empty()) {
        Some(key) => builder.plugin(tauri_plugin_aptabase::Builder::new(key).build()),
        None => builder,
    };

    let run_result = builder
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Let the frontend show the window after its first themed render.
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |_app| {
            t_startup::mark("setup-started");
            t_events::set_sink(std::sync::Arc::new(TauriEventSink(_app.handle().clone())));
            // The identifier is needed for the window title / app-data naming.
            t_config::set_app_identifier(&_app.config().identifier);

            // Pin the directories the core needs so it never reaches for an
            // AppHandle (see t_paths). Seeded verbatim from Tauri so the
            // resolved locations stay byte-identical.
            {
                let resource_dir = _app.path().resource_dir().ok();
                let app_cache_dir = _app.path().app_cache_dir().ok();
                t_paths::set_paths(t_paths::PathsConfig {
                    resource_dir,
                    app_cache_dir,
                });
            }
            t_video::init_ffmpeg_path();
            t_menu::install_app_menu(&_app.handle())?;

            #[cfg(not(target_os = "macos"))]
            if let Some(window) = _app.get_webview_window("main") {
                if let Err(e) = window.set_decorations(false) {
                    eprintln!("Failed to disable main window decorations: {}", e);
                }
            }

            // tauri.windows.conf.json sets zoomHotkeysEnabled=true so wry sets
            // both IsZoomControlEnabled and IsPinchZoomEnabled to true at WebView
            // creation. That combination is what allows Chromium to synthesize
            // wheel+ctrlKey events for touchpad pinch. Touchscreen pinch is still
            // handled by our pointer-event logic (with `touch-action: none`).

            t_startup::mark("database-started");
            // Create the database on startup
            match t_sqlite::create_db() {
                Ok(()) => t_startup::mark("database-ready"),
                Err(error) => {
                    t_startup::mark("database-failed");
                    eprintln!("Failed to initialize database: {}", error);
                }
            }

            // Initialize video HTTP server for Linux
            #[cfg(target_os = "linux")]
            t_http::init_video_http_server();

            // Cleanup video cache
            t_video::init_video_cache();

            if !t_sqlite::is_database_corrupted() {
                if let Err(e) = t_scopes::restore_album_scopes(&_app.handle()) {
                    eprintln!("Failed to restore asset scopes: {}", e);
                }
            }
            t_startup::mark("setup-ready");

            // Scheduled VL runs tick in the background for as long as the
            // app lives; see t_vl_schedule.rs.
            t_vl_schedule::spawn_scheduler();

            // Open devtools in development mode
            // #[cfg(debug_assertions)] // only include this block in debug builds
            // {
            //     let window = app.get_webview_window("main").unwrap();
            //     window.open_devtools();
            // }

            Ok(())
        })
        .on_window_event(move |window, event| {
            if window.label() != "main" {
                return;
            }

            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent the default close so we can decide what to do per platform.
                api.prevent_close();

                // macOS convention: closing the window hides it; the app stays
                // alive and can be reopened via the Dock icon (handled by
                // RunEvent::Reopen). Real quit keeps using the native Quit menu.
                #[cfg(target_os = "macos")]
                {
                    let _ = window.hide();
                }

                #[cfg(not(target_os = "macos"))]
                {
                    let app_handle = window.app_handle();

                    // Close every other window first (image viewer, settings, ...),
                    // then exit the process.
                    let windows = app_handle.webview_windows();
                    for (_, other_window) in windows {
                        if other_window.label() != "main" {
                            if let Err(err) = other_window.close() {
                                eprintln!("Failed to close window: {}", err);
                            }
                        }
                    }

                    app_handle.exit(0);
                }
            }
        })
        .on_menu_event(|app, event| {
            t_menu::handle_menu_event(app, event);
        })
        .invoke_handler(tauri::generate_handler![
            t_startup::record_startup_stage,
            t_startup::finish_startup,
            t_startup::get_ai_initialization_status,
            t_drag::start_file_drag,
            t_drag::cancel_file_drag,
            // library
            t_cmds::get_app_config,
            t_cmds::get_system_region_locale,
            t_cmds::get_supported_format_extensions,
            t_cmds::get_ffmpeg_backed_image_extensions,
            t_cmds::check_gstreamer_available,
            t_cmds::set_last_selected_item_index,
            t_cmds::get_thumbnail_cache_location,
            t_cmds::get_thumbnail_cache_migration_count,
            t_cmds::change_thumbnail_cache_location,
            t_cmds::cancel_thumbnail_cache_migration,
            t_cmds::get_db_storage_dir,
            t_cmds::is_using_custom_db_storage,
            t_cmds::change_db_storage_dir,
            t_cmds::reset_db_storage_dir,
            t_cmds::add_library,
            t_cmds::edit_library,
            t_cmds::remove_library,
            t_cmds::hide_library,
            t_cmds::reorder_libraries,
            t_cmds::switch_library,
            t_cmds::is_database_corrupted,
            t_cmds::get_library_info,
            t_cmds::save_library_state,
            t_cmds::get_library_state,
            t_cmds::get_current_library_state,
            // album
            t_cmds::get_all_albums,
            t_cmds::get_all_album_folders,
            t_cmds::generate_directory_thumbnails,
            t_cmds::get_album,
            t_cmds::check_album_accessibility,
            t_cmds::recount_album,
            t_cmds::get_album_visible_counts,
            t_cmds::add_album,
            t_cmds::add_album_by_path,
            t_cmds::edit_album,
            t_cmds::remove_album,
            t_cmds::reorder_albums,
            t_cmds::set_album_cover,
            t_cmds::index_album,
            t_cmds::cancel_indexing,
            t_cmds::get_index_recovery_info,
            t_cmds::clear_index_recovery_info,
            // folder
            t_cmds::select_folder,
            t_cmds::fetch_folder,
            t_cmds::count_folder,
            t_cmds::list_album_subfolders,
            t_cmds::create_folder,
            t_cmds::rename_folder,
            t_cmds::move_folder,
            t_cmds::move_folder_outside_library,
            t_cmds::copy_folder,
            t_cmds::delete_folder,
            t_cmds::prepare_folder_delete,
            t_cmds::delete_folder_permanently,
            t_cmds::reveal_path,
            t_cmds::open_external_url,
            t_cmds::set_desktop_wallpaper,
            t_cmds::get_external_app_display_name,
            t_cmds::open_file_with_app,
            t_cmds::open_files_with_app,
            // file query
            t_cmds::get_query_count_and_sum,
            t_cmds::get_query_time_line,
            t_cmds::get_query_files,
            t_cmds::get_grouped_query_rows,
            t_cmds::get_group_file_ids,
            t_cmds::get_grouped_file_position,
            t_cmds::get_query_file_ids,
            t_cmds::get_library_visible_counts,
            t_cmds::get_query_file_position,
            // smart album
            t_cmds::get_smart_query_count_and_sum,
            t_cmds::get_smart_query_time_line,
            t_cmds::get_smart_query_files,
            t_cmds::get_smart_grouped_query_rows,
            t_cmds::get_smart_group_file_ids,
            t_cmds::get_smart_query_file_ids,
            t_cmds::get_smart_query_file_position,
            // collection
            t_cmds::list_collections,
            t_cmds::get_collection_counts,
            t_cmds::create_collection,
            t_cmds::rename_collection,
            t_cmds::delete_collection,
            t_cmds::reorder_collections,
            t_cmds::add_files_to_collection,
            t_cmds::remove_files_from_collection,
            t_cmds::get_collection_selection_counts,
            t_cmds::clear_collection,
            t_cmds::get_collection_file_ids,
            t_cmds::get_file_collections,
            t_cmds::get_collection_count_and_sum,
            t_cmds::get_collection_files,
            t_cmds::get_collection_grouped_query_rows,
            t_cmds::get_collection_group_file_ids,
            t_cmds::get_collection_query_file_ids,
            // folder file query
            t_cmds::get_files_by_ids,
            t_cmds::get_folder_files,
            t_cmds::sync_album_folder_mtimes,
            t_cmds::reconcile_raw_jpeg_pairs,
            t_cmds::refresh_album_folders,
            t_cmds::is_directory_accessible,
            t_cmds::get_folder_thumb_count,
            // file operations
            t_cmds::edit_image,
            t_cmds::render_montage,
            t_cmds::copy_edited_image,
            t_cmds::copy_images,
            t_cmds::copy_file_paths,
            t_cmds::rename_file,
            t_cmds::preview_batch_rename,
            t_cmds::batch_rename_files,
            t_cmds::move_file,
            t_cmds::move_file_outside_library,
            t_cmds::copy_file,
            t_cmds::import_file,
            t_cmds::import_files,
            t_cmds::import_and_organize,
            t_cmds::cancel_import_and_organize,
            t_cmds::import_url,
            t_cmds::import_from_drag,
            t_cmds::get_drag_payload,
            t_cmds::import_file_bytes,
            t_cmds::has_importable_clipboard,
            t_cmds::import_clipboard,
            t_cmds::delete_file,
            t_cmds::delete_file_permanently,
            t_cmds::delete_db_file,
            t_cmds::batch_delete_files,
            // file metadata
            t_cmds::edit_file_metadata,
            t_cmds::clean_unused_thumbnail_cache,
            t_cmds::get_file_thumb,
            t_cmds::get_file_thumb_by_id,
            t_cmds::get_file_thumbs,
            t_cmds::get_file_info,
            t_cmds::get_file_metadata_details,
            t_cmds::cancel_file_metadata_requests,
            t_cmds::update_file_info,
            t_cmds::begin_file_transfer_batch,
            t_cmds::end_file_transfer_batch,
            t_cmds::begin_file_refresh_batch,
            t_cmds::end_file_refresh_batch,
            t_cmds::refresh_selected_file_info,
            t_cmds::change_video_thumbnail,
            t_cmds::is_cloud_only_file,
            t_cmds::prepare_motion_photo_video,
            t_cmds::add_file_to_db,
            t_cmds::check_file_exists,
            t_cmds::check_file_accessibility,
            t_cmds::set_file_rotate,
            t_cmds::get_file_has_tags,
            // favorite
            t_cmds::get_favorite_folders,
            t_cmds::get_folder_favorite,
            t_cmds::set_folder_favorite,
            t_cmds::get_folder_search_excluded,
            t_cmds::set_folder_search_excluded,
            t_cmds::set_file_favorite,
            t_cmds::set_file_rating,
            t_cmds::set_file_culling_flag,
            t_cmds::batch_update_file_metadata,
            // xmp
            t_cmds::get_xmp_import_settings,
            t_cmds::set_xmp_import_settings,
            t_cmds::save_xmp_metadata,
            // tag
            t_cmds::get_all_tags,
            t_cmds::get_tag_counts,
            t_cmds::get_tag_name,
            t_cmds::create_tag,
            t_cmds::move_tag,
            t_cmds::get_tag_subtree_info,
            t_cmds::rename_tag,
            t_cmds::delete_tag,
            t_cmds::get_tags_for_file,
            t_cmds::add_tag_to_file,
            t_cmds::remove_tag_from_file,
            t_cmds::get_tag_selection_counts,
            t_cmds::apply_tags_to_files,
            // calendar
            t_cmds::get_taken_dates,
            // camera
            t_cmds::get_camera_info,
            t_cmds::get_lens_info,
            // location
            t_cmds::get_location_info,
            t_cmds::get_gps_map_points,
            // settings
            t_cmds::get_package_info,
            t_cmds::get_build_time,
            t_cmds::get_storage_file_info,
            // ai
            t_cmds::check_ai_status,
            t_cmds::get_image_search_model_status,
            t_cmds::set_image_search_model,
            t_cmds::download_multilingual_image_search_model,
            t_cmds::cancel_multilingual_image_search_model_download,
            t_cmds::generate_embedding,
            t_cmds::search_similar_images,
            // caption embeddings (semantic search over descriptions)
            t_embed::embed_status,
            t_embed::rebuild_caption_embeds,
            t_embed::search_captions,
            // person (face recognition)
            t_cmds::index_faces,
            t_cmds::cancel_face_index,
            t_cmds::reset_faces,
            t_cmds::is_face_indexing,
            t_cmds::get_face_stats,
            t_cmds::get_persons,
            t_cmds::get_persons_page,
            t_cmds::rename_person,
            t_cmds::delete_person,
            t_cmds::get_faces_for_file,
            t_af_area::get_af_areas,
            t_cmds::get_person_thumbnail,
            // dedup
            t_cmds::dedup_start_scan,
            t_cmds::dedup_get_scan_status,
            t_cmds::dedup_cancel_scan,
            t_cmds::dedup_list_groups,
            t_cmds::dedup_get_overview,
            t_cmds::dedup_set_keep,
            t_cmds::dedup_get_group_items,
            t_cmds::dedup_delete,
            t_cmds::similar_start_scan,
            t_cmds::similar_get_scan_status,
            t_cmds::similar_cancel_scan,
            t_cmds::similar_get_eligible_count,
            t_cmds::similar_list_groups,
            t_cmds::similar_get_overview,
            t_cmds::similar_get_group,
            t_cmds::similar_set_keep,
            t_cmds::similar_has_scan,
            // image understanding (vision language)
            t_vl::get_vl_config,
            t_vl::set_vl_config,
            t_vl::list_vl_templates,
            t_vl::create_vl_template,
            t_vl::update_vl_template,
            t_vl::delete_vl_template,
            t_vl::run_vl_analysis,
            t_vl::get_vl_annotations,
            t_vl::get_vl_calls,
            t_vl::cancel_vl_run,
            t_vl::rerank_search_with_llm,
            t_vl::list_vl_models,
            t_vl::list_vl_runs,
            t_vl::list_vl_run_calls,
            t_vl::retry_vl_run,
            t_vl::delete_vl_run,
            t_vl::clear_vl_runs,
            t_vl_schedule::queue_vl_task,
            t_vl_schedule::list_vl_tasks,
            t_vl_schedule::update_vl_task_time,
            t_vl_schedule::delete_vl_task,
            // exact search
            t_fts::search_fts,
            t_fts::fts_reindex,
            t_fts::fts_status,
            // video
            t_video::prepare_video,
            t_video::get_video_stream_url,
            t_video::cancel_video_prepare,
            t_video::clear_video_cache,
            // backup / restore
            t_cmds::get_db_storage_info,
            t_cmds::backup_databases,
            t_cmds::parse_backup_file,
            t_cmds::restore_databases,
        ])
        .build(tauri::generate_context!());

    match run_result {
        Ok(app) => {
            app.run(move |app_handle, event| match event {
                tauri::RunEvent::Ready => {
                    if aptabase_enabled {
                        let _ = app_handle.track_event("app_started", None);
                    }
                }
                tauri::RunEvent::Exit { .. } => {
                    if aptabase_enabled {
                        let _ = app_handle.track_event("app_exited", None);
                        app_handle.flush_events_blocking();
                    }
                }

                // macOS: clicking the Dock icon of a running app reopens it.
                // When the main window is hidden (closed-to-hide), show it again.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                _ => {}
            });
        }
        Err(err) => {
            eprintln!("Error while running application: {}", err);
        }
    }
}

/// Thin client: no local backend. The window shows a remote `lapd` server's
/// Web UI; a native menu (Rust-side, the only control surface that works when a
/// remote page is shown) switches between servers, and a local picker page
/// manages the list. Built with `--no-default-features --features client`, so
/// `lappp-core` (the C image libraries, the AI stack, every local command) is
/// never linked.
#[cfg(all(feature = "client", not(feature = "embedded")))]
async fn run_client() {
    // Load the persisted server list. A missing/corrupt file yields an empty
    // list (never a hard failure) so the user can always re-add servers.
    let path = match servers::default_config_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("thin client: {e}");
            return;
        }
    };
    let config = servers::load_from(&path);
    // An explicit `LAP_CONNECT` (handy for scripts/tests) wins over the saved
    // choice without persisting it.
    let startup_target = std::env::var("LAP_CONNECT")
        .ok()
        .and_then(|raw| servers::normalize_url(&raw))
        .or_else(|| config.current.clone());

    let builder = client_cmds::register_picker_protocol(tauri::Builder::default())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        & !tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .manage(client_cmds::ClientState {
            path: path.clone(),
            config: std::sync::Mutex::new(config.clone()),
        })
        .invoke_handler(tauri::generate_handler![
            client_cmds::client_list_servers,
            client_cmds::client_add_server,
            client_cmds::client_remove_server,
            client_cmds::client_connect,
            client_cmds::client_show_picker,
        ])
        .setup(move |app| {
            let _ = t_menu::install_client_menu(&app.handle().clone(), &config);
            if let Some(window) = app.get_webview_window("main") {
                // Our UI normally draws its own title bar (decorations:false in
                // tauri.conf.json). The remote Web UI does not, so restore the
                // OS-native frame or the window could not be moved or closed.
                let _ = window.set_decorations(true);
                match startup_target.as_deref() {
                    Some(url) => {
                        if let Err(e) = client_cmds::navigate_main(&app.handle().clone(), url) {
                            eprintln!("thin client: {e}");
                        }
                    }
                    // No configured server: open the local picker so the user
                    // can add one. This page is served by our own origin and so
                    // has the Tauri API.
                    None => {
                        let _ = client_cmds::client_show_picker(app.handle().clone());
                    }
                }
                let _ = window.show();
            }
            Ok(())
        })
        .on_menu_event(|app, event| {
            let servers = app
                .state::<client_cmds::ClientState>()
                .config
                .lock()
                .unwrap()
                .servers
                .clone();
            t_menu::handle_client_menu_event(app, &event.id().0, &servers);
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            // No local core or tray to keep alive: closing the window quits.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                window.app_handle().exit(0);
            }
        });

    match builder.build(tauri::generate_context!()) {
        Ok(app) => app.run(|_app_handle, _event| {}),
        Err(err) => eprintln!("Error while running thin client: {err}"),
    }
}
