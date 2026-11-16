//! Desktop command wrappers over lap_core::t_video (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lap_core::t_video::*;


#[tauri::command]
pub async fn prepare_video(
    file_path: String,
    player_id: String,
    force: Option<String>,
    request_id: Option<u64>,
) -> Result<VideoPrepareResult, String>
{
    lap_core::t_video::prepare_video(file_path, player_id, force, request_id).await
}


#[tauri::command]
pub fn get_video_stream_url(file_path: String) -> Result<String, String>
{
    lap_core::t_video::get_video_stream_url(file_path)
}


#[tauri::command]
pub async fn cancel_video_prepare(
    player_id: String,
    request_id: Option<u64>,
) -> Result<(), String>
{
    lap_core::t_video::cancel_video_prepare(player_id, request_id).await
}


#[tauri::command]
pub async fn clear_video_cache() -> Result<(), String>
{
    lap_core::t_video::clear_video_cache().await
}
