//! Desktop command wrappers over lap_core::t_vl_schedule (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lap_core::t_vl_schedule::*;


#[tauri::command]
pub fn queue_vl_task(
    file_ids: Vec<i64>,
    template_id: i64,
    force: bool,
    time: String,
    date: String,
) -> Result<VlTaskView, String>
{
    lap_core::t_vl_schedule::queue_vl_task(file_ids, template_id, force, time, date)
}


#[tauri::command]
pub fn list_vl_tasks() -> Result<Vec<VlTaskView>, String>
{
    lap_core::t_vl_schedule::list_vl_tasks()
}


#[tauri::command]
pub fn update_vl_task_time(
    id: String,
    time: String,
    date: String,
) -> Result<(), String>
{
    lap_core::t_vl_schedule::update_vl_task_time(id, time, date)
}


#[tauri::command]
pub fn delete_vl_task(id: String) -> Result<(), String>
{
    lap_core::t_vl_schedule::delete_vl_task(id)
}
