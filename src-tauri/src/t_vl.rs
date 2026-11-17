//! Desktop command wrappers over lappp_core::t_vl (serverization phase 3).
//! The module name and command names stay identical, so main.rs
//! registration is unchanged; everything else re-exports the core.

pub use lappp_core::t_vl::*;
// Signatures reference protocol types the core module imports itself.
use lappp_core::t_vl_protocol::*;


#[tauri::command]
pub fn get_vl_config() -> Result<VlConfigView, String>
{
    lappp_core::t_vl::get_vl_config()
}


#[tauri::command]
pub fn set_vl_config(
    gateway_url: String,
    api_key: Option<String>,
    model: String,
    rerank_model: String,
    embed_source: String,
    embed_model: String,
    protocol: String,
    max_concurrency: i64,
) -> Result<VlConfigView, String>
{
    lappp_core::t_vl::set_vl_config(gateway_url, api_key, model, rerank_model, embed_source, embed_model, protocol, max_concurrency)
}


#[tauri::command]
pub fn list_vl_templates() -> Result<Vec<VlTemplate>, String>
{
    lappp_core::t_vl::list_vl_templates()
}


#[tauri::command]
pub fn create_vl_template(
    name: String,
    prompt_template: String,
    system_template: String,
    response_spec: String,
) -> Result<VlTemplate, String>
{
    lappp_core::t_vl::create_vl_template(name, prompt_template, system_template, response_spec)
}


#[tauri::command]
pub fn update_vl_template(
    id: i64,
    name: String,
    prompt_template: String,
    system_template: String,
    response_spec: String,
) -> Result<VlTemplate, String>
{
    lappp_core::t_vl::update_vl_template(id, name, prompt_template, system_template, response_spec)
}


#[tauri::command]
pub fn delete_vl_template(id: i64) -> Result<bool, String>
{
    lappp_core::t_vl::delete_vl_template(id)
}


#[tauri::command]
pub fn run_vl_analysis(
    file_ids: Vec<i64>,
    template_id: i64,
    force: bool,
) -> Result<String, String>
{
    lappp_core::t_vl::run_vl_analysis(file_ids, template_id, force)
}


#[tauri::command]
pub fn get_vl_annotations(file_id: i64) -> Result<Vec<VlAnnotation>, String>
{
    lappp_core::t_vl::get_vl_annotations(file_id)
}


#[tauri::command]
pub fn get_vl_calls(file_id: i64) -> Result<Vec<VlCall>, String>
{
    lappp_core::t_vl::get_vl_calls(file_id)
}


#[tauri::command]
pub fn cancel_vl_run(run_id: String) -> Result<(), String>
{
    lappp_core::t_vl::cancel_vl_run(run_id)
}


#[tauri::command]
pub async fn rerank_search_with_llm(
    query: String,
    file_ids: Vec<i64>,
) -> Result<RerankSelection, String>
{
    lappp_core::t_vl::rerank_search_with_llm(query, file_ids).await
}


#[tauri::command]
pub async fn list_vl_models() -> Result<VlModelList, String>
{
    lappp_core::t_vl::list_vl_models().await
}


#[tauri::command]
pub fn list_vl_runs() -> Result<Vec<VlRunRecord>, String>
{
    lappp_core::t_vl::list_vl_runs()
}


#[tauri::command]
pub fn list_vl_run_calls(run_id: String) -> Result<Vec<VlCall>, String>
{
    lappp_core::t_vl::list_vl_run_calls(run_id)
}


#[tauri::command]
pub fn retry_vl_run(run_id: String) -> Result<String, String>
{
    lappp_core::t_vl::retry_vl_run(run_id)
}


#[tauri::command]
pub fn delete_vl_run(run_id: String) -> Result<(), String>
{
    lappp_core::t_vl::delete_vl_run(run_id)
}


#[tauri::command]
pub fn clear_vl_runs() -> Result<u64, String>
{
    lappp_core::t_vl::clear_vl_runs()
}
