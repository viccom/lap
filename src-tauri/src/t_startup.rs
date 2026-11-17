//! Desktop command wrappers over lappp_core::t_startup (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lappp_core::t_startup::*;


#[tauri::command]
pub fn record_startup_stage(stage: String)
{
    lappp_core::t_startup::record_startup_stage(stage)
}


#[tauri::command]
pub fn finish_startup(model: i64)
{
    lappp_core::t_startup::finish_startup(model)
}


#[tauri::command]
pub fn get_ai_initialization_status() -> String
{
    lappp_core::t_startup::get_ai_initialization_status()
}
