//! Desktop command wrappers over lap_core::t_fts (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lap_core::t_fts::*;


#[tauri::command]
pub async fn search_fts(query: String, limit: Option<i64>) -> Result<Vec<FtsHit>, String>
{
    lap_core::t_fts::search_fts(query, limit).await
}


#[tauri::command]
pub async fn fts_reindex() -> Result<usize, String>
{
    lap_core::t_fts::fts_reindex().await
}


#[tauri::command]
pub fn fts_status() -> Result<FtsStatus, String>
{
    lap_core::t_fts::fts_status()
}
