//! Desktop command wrappers over lappp_core::t_af_area (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lappp_core::t_af_area::*;


#[tauri::command]
pub async fn get_af_areas(file_path: String) -> Result<Vec<AfArea>, String>
{
    lappp_core::t_af_area::get_af_areas(file_path).await
}
