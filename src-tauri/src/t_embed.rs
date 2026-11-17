//! Desktop command wrappers over lappp_core::t_embed (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lappp_core::t_embed::*;


#[tauri::command]
pub fn embed_status() -> Result<EmbedStatusView, String>
{
    lappp_core::t_embed::embed_status()
}


#[tauri::command]
pub async fn rebuild_caption_embeds() -> Result<(usize, usize), String>
{
    lappp_core::t_embed::rebuild_caption_embeds().await
}


#[tauri::command]
pub async fn search_captions(
    query: String,
    threshold: Option<f32>,
    limit: Option<i64>,
) -> Result<Vec<CaptionHit>, String>
{
    lappp_core::t_embed::search_captions(query, threshold, limit).await
}
