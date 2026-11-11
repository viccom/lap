//! Image understanding (vision-language) backend.
//!
//! Three concerns live here: the immutable prompt templates, the annotations
//! produced from a successful call, and the call evidence kept for every
//! request (successful or not).
use base64::Engine as _;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::Emitter;

use crate::t_common::{
    VL_ANTHROPIC_VERSION, VL_API_KEY_ENV, VL_DEFAULT_MODEL, VL_GATEWAY_URL_ENV, VL_MAX_TOKENS,
};

// ----------------------------------------------------------------------------
// Types
// ----------------------------------------------------------------------------

/// A prompt template. Rows are immutable: editing creates the next version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlTemplate {
    pub id: i64,
    pub name: String,
    pub version: i64,
    pub prompt_template: String,
    /// The system message of the template. It carries the output format.
    pub system_template: String,
    pub response_spec: String,
    pub is_builtin: bool,
    pub enabled: bool,
    pub created_at: i64,
}

/// A successful understanding result for one file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlAnnotation {
    pub id: i64,
    pub file_id: i64,
    pub template_id: i64,
    pub template_version: i64,
    pub model: String,
    pub result_json: String,
    pub raw_text: String,
    pub call_id: i64,
    pub created_at: i64,
}

/// Evidence of one gateway call. Rows are kept for failures too.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlCall {
    pub id: i64,
    pub file_id: i64,
    pub template_id: i64,
    pub template_version: i64,
    pub prompt_actual: String,
    pub system_actual: Option<String>,
    pub raw_response: Option<String>,
    pub thinking_text: Option<String>,
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub thinking_tokens: Option<i64>,
    pub stop_reason: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
    pub created_at: i64,
}

/// Gateway settings. The API key never leaves the backend.
#[derive(Debug, Clone)]
pub struct VlGatewayConfig {
    pub gateway_url: String,
    pub api_key: String,
    pub model: String,
}

/// Gateway settings as seen by the frontend. The API key is never included.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlConfigView {
    pub gateway_url: String,
    pub model: String,
    pub configured: bool,
}

/// Progress of a queued run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlRunStatus {
    pub run_id: i64,
    pub state: String,
    pub total: usize,
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub current_file_id: Option<i64>,
}

/// Storage operations the orchestrator needs. Implemented by the SQLite
/// backed store; tests provide an in-memory variant.
pub trait VlStore: Send + Sync {
    fn template(&self, id: i64) -> Result<Option<VlTemplate>, String>;
    fn templates(&self) -> Result<Vec<VlTemplate>, String>;
    fn insert_template(
        &self,
        name: &str,
        prompt_template: &str,
        system_template: &str,
        response_spec: &str,
        is_builtin: bool,
    ) -> Result<VlTemplate, String>;
    /// Insert the next version of a template.
    ///
    /// The new row always belongs to the user: editing a built-in template
    /// creates a custom copy of it, the built-in row itself stays untouched.
    fn insert_template_version(
        &self,
        base: &VlTemplate,
        name: &str,
        prompt_template: &str,
        system_template: &str,
        response_spec: &str,
    ) -> Result<VlTemplate, String>;
    fn delete_template(&self, id: i64) -> Result<bool, String>;

    /// The annotation of one file for one template version, if any.
    fn annotation(
        &self,
        file_id: i64,
        template_id: i64,
        template_version: i64,
    ) -> Result<Option<VlAnnotation>, String>;
    /// Every annotation of a file, newest first.
    fn annotations(&self, file_id: i64) -> Result<Vec<VlAnnotation>, String>;
    /// Replace the annotation of one file for one template version.
    fn upsert_annotation(&self, annotation: &VlAnnotation) -> Result<i64, String>;
    /// Remove the annotation of one file for one template version.
    fn delete_annotation(
        &self,
        file_id: i64,
        template_id: i64,
        template_version: i64,
    ) -> Result<(), String>;
    /// Store the evidence of one call and return its id.
    fn insert_call(&self, call: &VlCallDraft) -> Result<i64, String>;
    /// Every call of a file, newest first.
    fn calls(&self, file_id: i64) -> Result<Vec<VlCall>, String>;
}

/// The evidence of one call, before it is stored.
#[derive(Debug, Clone, Default)]
pub struct VlCallDraft {
    pub file_id: i64,
    pub template_id: i64,
    pub template_version: i64,
    pub prompt_actual: String,
    pub system_actual: Option<String>,
    pub raw_response: Option<String>,
    pub thinking_text: Option<String>,
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub thinking_tokens: Option<i64>,
    pub stop_reason: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub duration_ms: Option<i64>,
}

/// The image sent to the gateway.
#[derive(Debug, Clone)]
pub struct VlImage {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

/// Where the metadata and the image of a file come from.
pub trait VlSource: Send + Sync {
    /// The prompt context of a file, or `None` when the file is unknown.
    fn prompt_context(&self, file_id: i64) -> Result<Option<PromptContext>, String>;
    /// The image of a file, or `None` when it cannot be read.
    fn image(&self, file_id: i64) -> Result<Option<VlImage>, String>;
}

fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

const TEMPLATE_COLUMNS: &str = "id, name, version, prompt_template, system_template, response_spec, is_builtin, enabled, created_at";

fn template_from_row(row: &rusqlite::Row) -> Result<VlTemplate, rusqlite::Error> {
    Ok(VlTemplate {
        id: row.get(0)?,
        name: row.get(1)?,
        version: row.get(2)?,
        prompt_template: row.get(3)?,
        system_template: row.get(4)?,
        response_spec: row.get(5)?,
        is_builtin: row.get::<_, i64>(6)? != 0,
        enabled: row.get::<_, i64>(7)? != 0,
        created_at: row.get(8)?,
    })
}

fn query_template(conn: &Connection, id: i64) -> Result<Option<VlTemplate>, String> {
    conn.query_row(
        &format!("SELECT {TEMPLATE_COLUMNS} FROM vl_templates WHERE id = ?1"),
        params![id],
        template_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn query_templates(conn: &Connection) -> Result<Vec<VlTemplate>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TEMPLATE_COLUMNS} FROM vl_templates ORDER BY name COLLATE NOCASE, version DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], template_from_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn insert_template_row(
    conn: &Connection,
    name: &str,
    version: i64,
    prompt_template: &str,
    system_template: &str,
    response_spec: &str,
    is_builtin: bool,
) -> Result<VlTemplate, String> {
    conn.execute(
        "INSERT INTO vl_templates(name, version, prompt_template, system_template, response_spec, is_builtin, enabled, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
        params![
            name,
            version,
            prompt_template,
            system_template,
            response_spec,
            i64::from(is_builtin),
            now_ts()
        ],
    )
    .map_err(|e| e.to_string())?;
    query_template(conn, conn.last_insert_rowid())?.ok_or_else(|| "Template insert failed".into())
}

fn next_version(conn: &Connection, name: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM vl_templates WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// The version of a custom copy of a built-in template.
///
/// The copy continues the version chain of the row it was made from. When
/// that version is already taken by another row of the same name, the next
/// free version of the name is used instead.
fn copy_version(conn: &Connection, base: &VlTemplate, name: &str) -> Result<i64, String> {
    let preferred = base.version + 1;
    let taken: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM vl_templates WHERE name = ?1 AND version = ?2)",
            params![name, preferred],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if taken {
        return next_version(conn, name);
    }
    Ok(preferred)
}

/// Insert the next version of a template.
///
/// The new row belongs to the user: editing a built-in template creates a
/// custom copy of it and leaves the built-in row untouched.
fn insert_template_version_row(
    conn: &Connection,
    base: &VlTemplate,
    name: &str,
    prompt_template: &str,
    system_template: &str,
    response_spec: &str,
) -> Result<VlTemplate, String> {
    let version = if base.is_builtin {
        copy_version(conn, base, name)?
    } else {
        base.version + 1
    };
    insert_template_row(
        conn,
        name,
        version,
        prompt_template,
        system_template,
        response_spec,
        false,
    )
}

fn delete_template_row(conn: &Connection, id: i64) -> Result<bool, String> {
    let template = query_template(conn, id)?.ok_or_else(|| "Template not found".to_string())?;
    if template.is_builtin {
        return Err("内置模板不可删除".to_string());
    }
    // The annotations of the template go with it. The files have to be
    // indexed again, so the caption leaves the search with the annotation.
    let file_ids = files_with_template(conn, id)?;
    conn.execute(
        "DELETE FROM vl_annotations WHERE template_id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())?;
    let deleted = conn
        .execute("DELETE FROM vl_templates WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    for file_id in file_ids {
        let _ = crate::t_fts::upsert_doc(conn, file_id);
    }
    Ok(deleted > 0)
}

/// The files that carry an annotation of a template.
fn files_with_template(conn: &Connection, template_id: i64) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT file_id FROM vl_annotations WHERE template_id = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![template_id], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

const ANNOTATION_COLUMNS: &str =
    "id, file_id, template_id, template_version, model, result_json, raw_text, call_id, created_at";

fn annotation_from_row(row: &rusqlite::Row) -> Result<VlAnnotation, rusqlite::Error> {
    Ok(VlAnnotation {
        id: row.get(0)?,
        file_id: row.get(1)?,
        template_id: row.get(2)?,
        template_version: row.get(3)?,
        model: row.get(4)?,
        result_json: row.get(5)?,
        raw_text: row.get(6)?,
        call_id: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn query_annotation(
    conn: &Connection,
    file_id: i64,
    template_id: i64,
    template_version: i64,
) -> Result<Option<VlAnnotation>, String> {
    conn.query_row(
        &format!(
            "SELECT {ANNOTATION_COLUMNS} FROM vl_annotations
             WHERE file_id = ?1 AND template_id = ?2 AND template_version = ?3"
        ),
        params![file_id, template_id, template_version],
        annotation_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn query_annotations(conn: &Connection, file_id: i64) -> Result<Vec<VlAnnotation>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {ANNOTATION_COLUMNS} FROM vl_annotations WHERE file_id = ?1 ORDER BY id DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![file_id], annotation_from_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn upsert_annotation_row(conn: &Connection, annotation: &VlAnnotation) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO vl_annotations(file_id, template_id, template_version, model, result_json, raw_text, call_id, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(file_id, template_id, template_version)
         DO UPDATE SET model = excluded.model, result_json = excluded.result_json,
                       raw_text = excluded.raw_text, call_id = excluded.call_id",
        params![
            annotation.file_id,
            annotation.template_id,
            annotation.template_version,
            annotation.model,
            annotation.result_json,
            annotation.raw_text,
            annotation.call_id,
            annotation.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    let id = conn
        .query_row(
            "SELECT id FROM vl_annotations WHERE file_id = ?1 AND template_id = ?2 AND template_version = ?3",
            params![
                annotation.file_id,
                annotation.template_id,
                annotation.template_version
            ],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    // The annotation text has to be findable right away.
    let _ = crate::t_fts::upsert_doc(conn, annotation.file_id);
    Ok(id)
}

fn delete_annotation_row(
    conn: &Connection,
    file_id: i64,
    template_id: i64,
    template_version: i64,
) -> Result<(), String> {
    conn.execute(
        "DELETE FROM vl_annotations WHERE file_id = ?1 AND template_id = ?2 AND template_version = ?3",
        params![file_id, template_id, template_version],
    )
    .map_err(|e| e.to_string())?;
    // The annotation text has to leave the search with the annotation.
    let _ = crate::t_fts::upsert_doc(conn, file_id);
    Ok(())
}

const CALL_COLUMNS: &str =
    "id, file_id, template_id, template_version, prompt_actual, system_actual,
    raw_response, thinking_text, model, request_id, input_tokens, output_tokens,
    cache_read_tokens, thinking_tokens, stop_reason, status, error, duration_ms, created_at";

fn call_from_row(row: &rusqlite::Row) -> Result<VlCall, rusqlite::Error> {
    Ok(VlCall {
        id: row.get(0)?,
        file_id: row.get(1)?,
        template_id: row.get(2)?,
        template_version: row.get(3)?,
        prompt_actual: row.get(4)?,
        system_actual: row.get(5)?,
        raw_response: row.get(6)?,
        thinking_text: row.get(7)?,
        model: row.get(8)?,
        request_id: row.get(9)?,
        input_tokens: row.get(10)?,
        output_tokens: row.get(11)?,
        cache_read_tokens: row.get(12)?,
        thinking_tokens: row.get(13)?,
        stop_reason: row.get(14)?,
        status: row.get(15)?,
        error: row.get(16)?,
        duration_ms: row.get(17)?,
        created_at: row.get(18)?,
    })
}

fn insert_call_row(conn: &Connection, draft: &VlCallDraft) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO vl_calls(file_id, template_id, template_version, prompt_actual, system_actual,
            raw_response, thinking_text, model, request_id, input_tokens, output_tokens,
            cache_read_tokens, thinking_tokens, stop_reason, status, error, duration_ms, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            draft.file_id,
            draft.template_id,
            draft.template_version,
            draft.prompt_actual,
            draft.system_actual,
            draft.raw_response,
            draft.thinking_text,
            draft.model,
            draft.request_id,
            draft.input_tokens,
            draft.output_tokens,
            draft.cache_read_tokens,
            draft.thinking_tokens,
            draft.stop_reason,
            draft.status,
            draft.error,
            draft.duration_ms,
            now_ts()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

fn query_calls(conn: &Connection, file_id: i64) -> Result<Vec<VlCall>, String> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {CALL_COLUMNS} FROM vl_calls WHERE file_id = ?1 ORDER BY id DESC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![file_id], call_from_row)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// SQLite backed store used by the commands.
pub struct SqliteVlStore;

impl VlStore for SqliteVlStore {
    fn template(&self, id: i64) -> Result<Option<VlTemplate>, String> {
        let conn = crate::t_sqlite::open_conn()?;
        query_template(&conn, id)
    }

    fn templates(&self) -> Result<Vec<VlTemplate>, String> {
        let conn = crate::t_sqlite::open_conn()?;
        query_templates(&conn)
    }

    fn insert_template(
        &self,
        name: &str,
        prompt_template: &str,
        system_template: &str,
        response_spec: &str,
        is_builtin: bool,
    ) -> Result<VlTemplate, String> {
        let conn = crate::t_sqlite::open_conn()?;
        let version = next_version(&conn, name)?;
        insert_template_row(
            &conn,
            name,
            version,
            prompt_template,
            system_template,
            response_spec,
            is_builtin,
        )
    }

    fn insert_template_version(
        &self,
        base: &VlTemplate,
        name: &str,
        prompt_template: &str,
        system_template: &str,
        response_spec: &str,
    ) -> Result<VlTemplate, String> {
        let conn = crate::t_sqlite::open_conn()?;
        insert_template_version_row(
            &conn,
            base,
            name,
            prompt_template,
            system_template,
            response_spec,
        )
    }

    fn delete_template(&self, id: i64) -> Result<bool, String> {
        let conn = crate::t_sqlite::open_conn()?;
        delete_template_row(&conn, id)
    }

    fn annotation(
        &self,
        file_id: i64,
        template_id: i64,
        template_version: i64,
    ) -> Result<Option<VlAnnotation>, String> {
        let conn = crate::t_sqlite::open_conn()?;
        query_annotation(&conn, file_id, template_id, template_version)
    }

    fn annotations(&self, file_id: i64) -> Result<Vec<VlAnnotation>, String> {
        let conn = crate::t_sqlite::open_conn()?;
        query_annotations(&conn, file_id)
    }

    fn upsert_annotation(&self, annotation: &VlAnnotation) -> Result<i64, String> {
        let conn = crate::t_sqlite::open_conn()?;
        upsert_annotation_row(&conn, annotation)
    }

    fn delete_annotation(
        &self,
        file_id: i64,
        template_id: i64,
        template_version: i64,
    ) -> Result<(), String> {
        let conn = crate::t_sqlite::open_conn()?;
        delete_annotation_row(&conn, file_id, template_id, template_version)
    }

    fn insert_call(&self, call: &VlCallDraft) -> Result<i64, String> {
        let conn = crate::t_sqlite::open_conn()?;
        insert_call_row(&conn, call)
    }

    fn calls(&self, file_id: i64) -> Result<Vec<VlCall>, String> {
        let conn = crate::t_sqlite::open_conn()?;
        query_calls(&conn, file_id)
    }
}

// ----------------------------------------------------------------------------
// Template commands
// ----------------------------------------------------------------------------

fn validate_template_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("模板名称不能为空".to_string());
    }
    if name.chars().count() > 120 {
        return Err("模板名称过长".to_string());
    }
    Ok(name.to_string())
}

/// List every template version, newest version of a name first.
pub fn list_templates(store: &dyn VlStore) -> Result<Vec<VlTemplate>, String> {
    store.templates()
}

/// Create a new template. The first version of a name is version 1.
pub fn create_template(
    store: &dyn VlStore,
    name: &str,
    prompt_template: &str,
    system_template: &str,
    response_spec: &str,
) -> Result<VlTemplate, String> {
    let name = validate_template_name(name)?;
    if prompt_template.trim().is_empty() {
        return Err("提示词模板不能为空".to_string());
    }
    store.insert_template(
        &name,
        prompt_template,
        system_template,
        response_spec,
        false,
    )
}

/// Editing a template creates the next version; the previous row stays.
///
/// Editing a built-in template does not change it: the edit creates a custom
/// copy of it that belongs to the user. The built-in row stays read-only.
pub fn update_template(
    store: &dyn VlStore,
    id: i64,
    name: &str,
    prompt_template: &str,
    system_template: &str,
    response_spec: &str,
) -> Result<VlTemplate, String> {
    let base = store
        .template(id)?
        .ok_or_else(|| "模板不存在".to_string())?;
    let name = validate_template_name(name)?;
    if prompt_template.trim().is_empty() {
        return Err("提示词模板不能为空".to_string());
    }
    store.insert_template_version(
        &base,
        &name,
        prompt_template,
        system_template,
        response_spec,
    )
}

/// Delete a template. Built-in templates are protected.
pub fn delete_template(store: &dyn VlStore, id: i64) -> Result<bool, String> {
    store.delete_template(id)
}

// ----------------------------------------------------------------------------
// Prompt rendering
// ----------------------------------------------------------------------------

/// Values substituted into a prompt template.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    pub file_name: Option<String>,
    pub exif_summary: Option<String>,
    pub taken_at: Option<String>,
    pub camera: Option<String>,
}

/// Substitute the known placeholders. Missing values become an empty string
/// and unknown placeholders are kept verbatim.
pub fn render_prompt(template: &str, context: &PromptContext) -> String {
    render_template(template, context)
}

/// Render the system message of a template. It uses the same placeholders as
/// the prompt. An empty system template stays empty.
pub fn render_system(template: &str, context: &PromptContext) -> String {
    if template.trim().is_empty() {
        return String::new();
    }
    render_template(template, context)
}

/// The shared placeholder substitution of the prompt and the system message.
fn render_template(template: &str, context: &PromptContext) -> String {
    let mut rendered = template.to_string();
    for (placeholder, value) in [
        ("{file_name}", context.file_name.as_deref()),
        ("{exif_summary}", context.exif_summary.as_deref()),
        ("{taken_at}", context.taken_at.as_deref()),
        ("{camera}", context.camera.as_deref()),
    ] {
        rendered = rendered.replace(placeholder, value.unwrap_or(""));
    }
    rendered
}

/// Build the prompt context of a file from its metadata.
pub fn prompt_context(file: &crate::t_sqlite::AFile) -> PromptContext {
    let camera = match (
        file.e_make.as_deref().unwrap_or("").trim(),
        file.e_model.as_deref().unwrap_or("").trim(),
    ) {
        ("", "") => None,
        (make, model) => Some(format!("{make} {model}").trim().to_string()),
    };

    let exif_summary = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(lens) = file
            .e_lens_model
            .as_deref()
            .filter(|v| !v.trim().is_empty())
        {
            parts.push(lens.trim().to_string());
        }
        if let Some(time) = file
            .e_exposure_time
            .as_deref()
            .filter(|v| !v.trim().is_empty())
        {
            parts.push(time.trim().to_string());
        }
        if let Some(f_number) = file.e_f_number.as_deref().filter(|v| !v.trim().is_empty()) {
            parts.push(f_number.trim().to_string());
        }
        if let Some(iso) = file.e_iso_speed.as_deref().filter(|v| !v.trim().is_empty()) {
            parts.push(format!("ISO{}", iso.trim()));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" "))
        }
    };

    let taken_at = file.taken_date.and_then(|seconds| {
        chrono::DateTime::from_timestamp(seconds, 0)
            .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
    });

    PromptContext {
        file_name: Some(file.name.clone()),
        exif_summary,
        taken_at,
        camera,
    }
}

// ----------------------------------------------------------------------------
// Response parsing
// ----------------------------------------------------------------------------

/// Token usage reported by the gateway (Anthropic naming).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VlUsage {
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub cache_read_tokens: Option<i64>,
    pub thinking_tokens: Option<i64>,
}

/// The parts of a gateway response the backend cares about.
#[derive(Debug, Clone, Default)]
pub struct VlResponse {
    /// All `text` blocks joined in order.
    pub text: String,
    /// All `thinking` blocks joined in order.
    pub thinking: Option<String>,
    /// The model the gateway actually served.
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub stop_reason: Option<String>,
    pub usage: VlUsage,
}

impl VlResponse {
    /// A readable reason when the response carries no usable body.
    pub fn missing_body_reason(&self) -> Option<String> {
        if !self.text.trim().is_empty() {
            return None;
        }
        if self.stop_reason.as_deref() == Some("max_tokens") {
            return Some("模型输出被 max_tokens 上限截断，未返回正文内容".to_string());
        }
        Some("网关响应中没有正文内容".to_string())
    }
}

/// Parse a gateway response body.
pub fn parse_response(body: &str) -> Result<VlResponse, String> {
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("网关响应不是合法 JSON: {e}"))?;

    let blocks = value
        .get("content")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "网关响应缺少 content 字段".to_string())?;

    let mut text_parts: Vec<&str> = Vec::new();
    let mut thinking_parts: Vec<&str> = Vec::new();
    for block in blocks {
        match block.get("type").and_then(serde_json::Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(serde_json::Value::as_str) {
                    text_parts.push(text);
                }
            }
            Some("thinking") => {
                if let Some(thinking) = block.get("thinking").and_then(serde_json::Value::as_str) {
                    thinking_parts.push(thinking);
                }
            }
            _ => {}
        }
    }

    let usage_value = value.get("usage");
    let usage = VlUsage {
        input_tokens: usage_value
            .and_then(|u| u.get("input_tokens"))
            .and_then(serde_json::Value::as_i64),
        output_tokens: usage_value
            .and_then(|u| u.get("output_tokens"))
            .and_then(serde_json::Value::as_i64),
        cache_read_tokens: usage_value
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(serde_json::Value::as_i64),
        thinking_tokens: usage_value
            .and_then(|u| u.get("output_tokens_details"))
            .and_then(|d| d.get("thinking_tokens"))
            .and_then(serde_json::Value::as_i64),
    };

    Ok(VlResponse {
        text: text_parts.join(""),
        thinking: if thinking_parts.is_empty() {
            None
        } else {
            Some(thinking_parts.join(""))
        },
        model: value
            .get("model")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        request_id: value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        stop_reason: value
            .get("stop_reason")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        usage,
    })
}

/// Strip a ```json fence around the model text.
fn strip_code_fence(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let rest = trimmed.strip_prefix("```")?;
    let (first_line, body) = match rest.find('\n') {
        Some(index) => (&rest[..index], &rest[index + 1..]),
        None => (rest, ""),
    };
    let language = first_line.trim();
    if !language.is_empty() && !language.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let body = body.trim_end();
    let inner = body.strip_suffix("```")?;
    Some(inner.trim().to_string())
}

/// Extract the structured result from the model text. The second value is
/// `true` when the text could not be parsed as JSON.
pub fn result_json(text: &str, spec: &str) -> (serde_json::Value, bool) {
    let candidate = strip_code_fence(text).unwrap_or_else(|| text.trim().to_string());
    let parsed: serde_json::Value = match serde_json::from_str(&candidate) {
        Ok(value) => value,
        Err(_) => return (serde_json::json!({"_unparsed": true}), true),
    };

    let declared: Vec<String> = serde_json::from_str::<serde_json::Value>(spec)
        .ok()
        .and_then(|value| match value {
            serde_json::Value::Object(map) => Some(map.keys().cloned().collect()),
            _ => None,
        })
        .unwrap_or_default();

    if declared.is_empty() {
        return (parsed, false);
    }
    let Some(object) = parsed.as_object() else {
        return (parsed, false);
    };
    let mut filtered = serde_json::Map::new();
    for key in declared {
        if let Some(value) = object.get(&key) {
            filtered.insert(key, value.clone());
        }
    }
    if filtered.is_empty() {
        return (parsed, false);
    }
    (serde_json::Value::Object(filtered), false)
}

// ----------------------------------------------------------------------------
// Gateway client
// ----------------------------------------------------------------------------

/// A request sent to the gateway.
#[derive(Debug, Clone)]
pub struct VlRequest {
    pub image_base64: String,
    pub media_type: String,
    pub prompt: String,
    pub system: Option<String>,
}

/// What the gateway answered.
#[derive(Debug, Clone)]
pub struct VlCallOutcome {
    pub response: VlResponse,
    pub raw_body: String,
    pub duration_ms: i64,
}

/// A failed gateway call. The API key is never part of the message.
#[derive(Debug, Clone)]
pub struct VlGatewayError {
    pub message: String,
    pub raw_body: Option<String>,
}

impl VlGatewayError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            raw_body: None,
        }
    }
}

/// Call the gateway. Returns the parsed response or a readable error.
pub async fn call_gateway(
    client: &reqwest::Client,
    config: &VlGatewayConfig,
    request: &VlRequest,
) -> Result<VlCallOutcome, VlGatewayError> {
    if config.gateway_url.trim().is_empty() || config.api_key.trim().is_empty() {
        return Err(VlGatewayError::new(
            "图片理解网关未配置，请先设置网关地址与 API Key",
        ));
    }

    let endpoint = format!(
        "{}/v1/messages",
        config.gateway_url.trim().trim_end_matches('/')
    );

    let mut body = json!({
        "model": config.model,
        "max_tokens": VL_MAX_TOKENS,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": request.media_type,
                        "data": request.image_base64,
                    }
                },
                { "type": "text", "text": request.prompt }
            ]
        }]
    });
    if let Some(system) = request.system.as_ref().filter(|value| !value.is_empty()) {
        body["system"] = json!(system);
    }

    let started = Instant::now();
    let response = client
        .post(&endpoint)
        .header("x-api-key", config.api_key.as_str())
        .header("anthropic-version", VL_ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                VlGatewayError::new(format!("网关请求超时: {error}"))
            } else {
                VlGatewayError::new(format!("网关请求失败: {error}"))
            }
        })?;

    let status = response.status();
    let raw_body = response
        .text()
        .await
        .map_err(|error| VlGatewayError::new(format!("读取网关响应失败: {error}")))?;

    if !status.is_success() {
        let code = status.as_u16();
        let detail = error_detail(&raw_body).unwrap_or_else(|| raw_body.clone());
        // The detail comes from the gateway; it must never echo the key back.
        let detail = redact(&detail, &config.api_key);
        let message = match code {
            401 | 403 => format!("网关鉴权失败({code}): {detail}"),
            404 => format!("网关未找到该模型({code}): {detail}"),
            other => format!("网关返回错误({other}): {detail}"),
        };
        return Err(VlGatewayError {
            message,
            raw_body: Some(redact(&raw_body, &config.api_key)),
        });
    }

    let parsed = parse_response(&raw_body).map_err(|error| VlGatewayError {
        message: error,
        raw_body: Some(raw_body.clone()),
    })?;

    if let Some(reason) = parsed.missing_body_reason() {
        return Err(VlGatewayError {
            message: reason,
            raw_body: Some(raw_body.clone()),
        });
    }

    Ok(VlCallOutcome {
        response: parsed,
        raw_body,
        duration_ms: started.elapsed().as_millis() as i64,
    })
}

/// Replace the API key with a placeholder so it can never be stored or shown.
fn redact(text: &str, api_key: &str) -> String {
    if api_key.trim().is_empty() {
        return text.to_string();
    }
    text.replace(api_key, "***")
}

/// Extract the message of an error body. The gateway uses an OpenAI style
/// `error` object; the Anthropic style `error` object is accepted too.
fn error_detail(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let message = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .or_else(|| value.get("message").and_then(Value::as_str))?;
    Some(message.to_string())
}

// ----------------------------------------------------------------------------
// Configuration
// ----------------------------------------------------------------------------

/// Effective gateway settings: environment variables win over stored values.
pub fn resolve_gateway_config() -> Result<VlGatewayConfig, String> {
    let stored = crate::t_config::load_app_config()?;
    let gateway_url = std::env::var(VL_GATEWAY_URL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| stored.vl.gateway_url.trim().to_string());
    let api_key = std::env::var(VL_API_KEY_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| stored.vl.api_key.trim().to_string());
    let model = if stored.vl.model.trim().is_empty() {
        VL_DEFAULT_MODEL.to_string()
    } else {
        stored.vl.model.trim().to_string()
    };
    Ok(VlGatewayConfig {
        gateway_url,
        api_key,
        model,
    })
}

/// Configuration for the frontend. The API key is never included.
pub fn config_view() -> Result<VlConfigView, String> {
    let config = resolve_gateway_config()?;
    Ok(VlConfigView {
        gateway_url: config.gateway_url.clone(),
        model: config.model.clone(),
        configured: is_configured(&config),
    })
}

fn is_configured(config: &VlGatewayConfig) -> bool {
    !config.gateway_url.is_empty() && !config.api_key.is_empty()
}

/// Store gateway settings. `api_key` of `None` keeps the stored key.
pub fn set_config(
    gateway_url: &str,
    api_key: Option<String>,
    model: &str,
) -> Result<VlConfigView, String> {
    let gateway_url = gateway_url.trim().to_string();
    let model = if model.trim().is_empty() {
        VL_DEFAULT_MODEL.to_string()
    } else {
        model.trim().to_string()
    };
    crate::t_config::update_app_config(|config| {
        config.vl.gateway_url = gateway_url.clone();
        config.vl.model = model.clone();
        if let Some(key) = api_key.as_ref() {
            config.vl.api_key = key.trim().to_string();
        }
        Ok(())
    })?;
    config_view()
}

// ----------------------------------------------------------------------------
// Orchestration
// ----------------------------------------------------------------------------

/// What happened to one file.
#[derive(Debug, Clone, PartialEq)]
pub enum VlFileOutcome {
    /// A matching annotation existed, so no call was made.
    Skipped,
    /// The call succeeded and the annotation was stored.
    Annotated { call_id: i64 },
    /// The call failed; the evidence was stored.
    Failed { call_id: i64, error: String },
}

/// Understand one file with one template.
///
/// A file that already has an annotation for this template version is skipped
/// unless `force` is set. Every call leaves evidence behind, successful or not.
pub async fn analyze_file(
    store: &dyn VlStore,
    source: &dyn VlSource,
    client: &reqwest::Client,
    config: &VlGatewayConfig,
    file_id: i64,
    template: &VlTemplate,
    force: bool,
) -> Result<VlFileOutcome, String> {
    if !force
        && store
            .annotation(file_id, template.id, template.version)?
            .is_some()
    {
        return Ok(VlFileOutcome::Skipped);
    }
    if force {
        // A forced run replaces the annotation; the old evidence stays in vl_calls.
        store.delete_annotation(file_id, template.id, template.version)?;
    }

    // A failure before the call still leaves evidence behind.
    let prepared = prepare_call(source, file_id, template).await;
    let (rendered, request, failure) = match prepared {
        Ok(prepared) => (prepared.rendered, Some(prepared.request), None),
        Err((rendered, error)) => (rendered, None, Some(error)),
    };

    let outcome = match &request {
        Some(request) => call_gateway(client, config, request).await,
        None => Err(VlGatewayError::new(
            failure.clone().unwrap_or_else(|| "调用失败".to_string()),
        )),
    };

    let draft = match &outcome {
        Ok(success) => VlCallDraft {
            file_id,
            template_id: template.id,
            template_version: template.version,
            prompt_actual: rendered.prompt.clone(),
            system_actual: rendered.system.clone(),
            raw_response: Some(success.raw_body.clone()),
            thinking_text: success.response.thinking.clone(),
            model: success.response.model.clone(),
            request_id: success.response.request_id.clone(),
            input_tokens: success.response.usage.input_tokens,
            output_tokens: success.response.usage.output_tokens,
            cache_read_tokens: success.response.usage.cache_read_tokens,
            thinking_tokens: success.response.usage.thinking_tokens,
            stop_reason: success.response.stop_reason.clone(),
            status: "ok".to_string(),
            error: None,
            duration_ms: Some(success.duration_ms),
        },
        Err(error) => VlCallDraft {
            file_id,
            template_id: template.id,
            template_version: template.version,
            prompt_actual: rendered.prompt.clone(),
            system_actual: rendered.system.clone(),
            raw_response: error.raw_body.clone(),
            thinking_text: None,
            model: None,
            request_id: None,
            input_tokens: None,
            output_tokens: None,
            cache_read_tokens: None,
            thinking_tokens: None,
            stop_reason: None,
            status: "error".to_string(),
            error: Some(error.message.clone()),
            duration_ms: None,
        },
    };

    let call_id = store.insert_call(&draft)?;

    match outcome {
        Ok(success) => {
            let (value, _unparsed) = result_json(&success.response.text, &template.response_spec);
            let model = success
                .response
                .model
                .clone()
                .unwrap_or_else(|| config.model.clone());
            store.upsert_annotation(&VlAnnotation {
                id: 0,
                file_id,
                template_id: template.id,
                template_version: template.version,
                model,
                result_json: value.to_string(),
                raw_text: success.response.text.clone(),
                call_id,
                created_at: now_ts(),
            })?;
            Ok(VlFileOutcome::Annotated { call_id })
        }
        Err(error) => Ok(VlFileOutcome::Failed {
            call_id,
            error: error.message,
        }),
    }
}

/// The rendered prompt and system of one call.
#[derive(Debug, Clone, Default)]
struct RenderedTemplates {
    prompt: String,
    /// `None` when the template has no system message.
    system: Option<String>,
}

/// The rendered templates and the request to send.
struct PreparedCall {
    rendered: RenderedTemplates,
    request: VlRequest,
}

/// Render the prompt and the system and read the image. The rendered
/// templates are returned even when the image cannot be read, so the failure
/// can still be recorded.
async fn prepare_call(
    source: &dyn VlSource,
    file_id: i64,
    template: &VlTemplate,
) -> Result<PreparedCall, (RenderedTemplates, String)> {
    let context = match source.prompt_context(file_id) {
        Ok(Some(context)) => context,
        Ok(None) => {
            return Err((
                RenderedTemplates::default(),
                format!("文件 {file_id} 不存在"),
            ));
        }
        Err(error) => return Err((RenderedTemplates::default(), error)),
    };
    let rendered = render_templates(template, &context);

    let image = match source.image(file_id) {
        Ok(Some(image)) => image,
        Ok(None) => return Err((rendered, format!("文件 {file_id} 没有可用的缩略图"))),
        Err(error) => return Err((rendered, error)),
    };

    Ok(PreparedCall {
        request: VlRequest {
            image_base64: base64::engine::general_purpose::STANDARD.encode(&image.bytes),
            media_type: image.media_type,
            prompt: rendered.prompt.clone(),
            system: rendered.system.clone(),
        },
        rendered,
    })
}

/// Render the prompt and the system of a template with one context.
fn render_templates(template: &VlTemplate, context: &PromptContext) -> RenderedTemplates {
    let system = render_system(&template.system_template, context);
    RenderedTemplates {
        prompt: render_prompt(&template.prompt_template, context),
        // An empty system message is not sent to the gateway.
        system: if system.trim().is_empty() {
            None
        } else {
            Some(system)
        },
    }
}

// ----------------------------------------------------------------------------
// Library source
// ----------------------------------------------------------------------------

/// Reads files and thumbnails from the local library.
pub struct LibrarySource;

impl VlSource for LibrarySource {
    fn prompt_context(&self, file_id: i64) -> Result<Option<PromptContext>, String> {
        let file = crate::t_sqlite::AFile::get_file_info(file_id)?;
        Ok(file.as_ref().map(prompt_context))
    }

    fn image(&self, file_id: i64) -> Result<Option<VlImage>, String> {
        // Prefer the cached thumbnail; it is small enough to upload.
        if let Ok(Some(thumb)) = crate::t_sqlite::AThumb::fetch(file_id)
            && let Some(data) = thumb.thumb_data
            && !data.is_empty()
        {
            let media_type = crate::t_protocol::detect_image_mime(&data).to_string();
            return Ok(Some(VlImage {
                bytes: data,
                media_type,
            }));
        }

        // Fall back to the original file, but never download a cloud placeholder.
        let Some(file) = crate::t_sqlite::AFile::get_file_info(file_id)? else {
            return Ok(None);
        };
        let Some(path) = file.file_path.clone() else {
            return Ok(None);
        };
        if file.cloud_only || crate::t_utils::is_cloud_only_path(&path) {
            return Ok(None);
        }
        let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if crate::t_utils::is_cloud_only(&metadata) {
            return Ok(None);
        }
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        if bytes.is_empty() {
            return Ok(None);
        }
        let media_type = crate::t_protocol::detect_image_mime(&bytes).to_string();
        Ok(Some(VlImage { bytes, media_type }))
    }
}

// ----------------------------------------------------------------------------
// Commands
// ----------------------------------------------------------------------------

/// The gateway settings. The API key is never returned.
#[tauri::command]
pub fn get_vl_config() -> Result<VlConfigView, String> {
    config_view()
}

/// Store the gateway settings. `api_key` of `None` keeps the stored key.
#[tauri::command]
pub fn set_vl_config(
    gateway_url: String,
    api_key: Option<String>,
    model: String,
) -> Result<VlConfigView, String> {
    set_config(&gateway_url, api_key, &model)
}

/// Every template version, newest version of a name first.
#[tauri::command]
pub fn list_vl_templates() -> Result<Vec<VlTemplate>, String> {
    list_templates(&SqliteVlStore)
}

/// Create a template. The first version of a name is version 1.
#[tauri::command]
pub fn create_vl_template(
    name: String,
    prompt_template: String,
    system_template: String,
    response_spec: String,
) -> Result<VlTemplate, String> {
    create_template(
        &SqliteVlStore,
        &name,
        &prompt_template,
        &system_template,
        &response_spec,
    )
}

/// Edit a template. This creates the next version; the previous row stays.
#[tauri::command]
pub fn update_vl_template(
    id: i64,
    name: String,
    prompt_template: String,
    system_template: String,
    response_spec: String,
) -> Result<VlTemplate, String> {
    update_template(
        &SqliteVlStore,
        id,
        &name,
        &prompt_template,
        &system_template,
        &response_spec,
    )
}

/// Delete a template. Built-in templates are protected.
#[tauri::command]
pub fn delete_vl_template(id: i64) -> Result<bool, String> {
    delete_template(&SqliteVlStore, id)
}

/// Queue an analysis run and return its id.
#[tauri::command]
pub fn run_vl_analysis(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, VlState>,
    file_ids: Vec<i64>,
    template_id: i64,
    force: bool,
) -> Result<i64, String> {
    start_run(&app_handle, &state, file_ids, template_id, force)
}

/// The annotations of one file, newest first.
#[tauri::command]
pub fn get_vl_annotations(file_id: i64) -> Result<Vec<VlAnnotation>, String> {
    SqliteVlStore.annotations(file_id)
}

/// The call evidence of one file, newest first.
#[tauri::command]
pub fn get_vl_calls(file_id: i64) -> Result<Vec<VlCall>, String> {
    SqliteVlStore.calls(file_id)
}

/// Cancel a running analysis.
#[tauri::command]
pub fn cancel_vl_run(state: tauri::State<'_, VlState>, run_id: i64) -> Result<(), String> {
    let flag = state
        .cancel_flags
        .lock()
        .map_err(|_| "VL run state is unavailable".to_string())?
        .get(&run_id)
        .cloned();
    match flag {
        Some(flag) => {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }
        None => Err(format!("未找到运行 {run_id}")),
    }
}

// ----------------------------------------------------------------------------
// Run state
// ----------------------------------------------------------------------------

/// State of the queued runs.
#[derive(Default)]
pub struct VlState {
    pub next_run_id: Arc<AtomicI64>,
    pub runs: Arc<Mutex<HashMap<i64, VlRunStatus>>>,
    pub cancel_flags: Arc<Mutex<HashMap<i64, Arc<AtomicBool>>>>,
}

impl VlState {
    fn new_run(&self, total: usize) -> Result<(i64, Arc<AtomicBool>), String> {
        let run_id = self.next_run_id.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let status = VlRunStatus {
            run_id,
            state: "running".to_string(),
            total,
            ..Default::default()
        };
        self.runs
            .lock()
            .map_err(|_| "VL run state is unavailable".to_string())?
            .insert(run_id, status);
        self.cancel_flags
            .lock()
            .map_err(|_| "VL run state is unavailable".to_string())?
            .insert(run_id, cancel.clone());
        Ok((run_id, cancel))
    }

    fn update_run(&self, run_id: i64, update: impl FnOnce(&mut VlRunStatus)) {
        if let Ok(mut runs) = self.runs.lock()
            && let Some(status) = runs.get_mut(&run_id)
        {
            update(status);
        }
    }

    /// The status of one run.
    pub fn run_status(&self, run_id: i64) -> Option<VlRunStatus> {
        self.runs
            .lock()
            .ok()
            .and_then(|runs| runs.get(&run_id).cloned())
    }
}

/// Queue an analysis run. The run happens in the background.
pub fn start_run(
    app_handle: &tauri::AppHandle,
    state: &VlState,
    file_ids: Vec<i64>,
    template_id: i64,
    force: bool,
) -> Result<i64, String> {
    if file_ids.is_empty() {
        return Err("未选择要理解的文件".to_string());
    }
    let config = resolve_gateway_config()?;
    if !is_configured(&config) {
        return Err("图片理解网关未配置，请先设置网关地址与 API Key".to_string());
    }
    let template = SqliteVlStore
        .template(template_id)?
        .ok_or_else(|| "模板不存在".to_string())?;
    if !template.enabled {
        return Err("模板已停用".to_string());
    }

    let (run_id, cancel) = state.new_run(file_ids.len())?;
    let state = VlState {
        next_run_id: state.next_run_id.clone(),
        runs: state.runs.clone(),
        cancel_flags: state.cancel_flags.clone(),
    };
    let app_handle = app_handle.clone();

    tauri::async_runtime::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                crate::t_common::VL_REQUEST_TIMEOUT_SECS,
            ))
            .build()
        {
            Ok(client) => client,
            Err(error) => {
                eprintln!("Failed to create the VL gateway client: {}", error);
                return;
            }
        };

        for file_id in file_ids {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            state.update_run(run_id, |status| {
                status.current_file_id = Some(file_id);
            });
            let _ = app_handle.emit("vl-analysis-progress", status_payload(&state, run_id));

            match analyze_file(
                &SqliteVlStore,
                &LibrarySource,
                &client,
                &config,
                file_id,
                &template,
                force,
            )
            .await
            {
                Ok(VlFileOutcome::Skipped) => {
                    state.update_run(run_id, |status| {
                        status.processed += 1;
                        status.skipped += 1;
                    });
                }
                Ok(VlFileOutcome::Annotated { .. }) => {
                    state.update_run(run_id, |status| {
                        status.processed += 1;
                        status.succeeded += 1;
                    });
                }
                Ok(VlFileOutcome::Failed { .. }) => {
                    state.update_run(run_id, |status| {
                        status.processed += 1;
                        status.failed += 1;
                    });
                }
                Err(error) => {
                    eprintln!("VL analysis failed for file {}: {}", file_id, error);
                    state.update_run(run_id, |status| {
                        status.processed += 1;
                        status.failed += 1;
                    });
                }
            }
            let _ = app_handle.emit("vl-analysis-progress", status_payload(&state, run_id));
        }

        state.update_run(run_id, |status| {
            status.current_file_id = None;
            status.state = if cancel.load(Ordering::SeqCst) {
                "cancelled".to_string()
            } else {
                "finished".to_string()
            };
        });
        let _ = app_handle.emit("vl-analysis-finished", status_payload(&state, run_id));
    });

    Ok(run_id)
}

fn status_payload(state: &VlState, run_id: i64) -> VlRunStatus {
    state.run_status(run_id).unwrap_or_else(|| VlRunStatus {
        run_id,
        state: "unknown".to_string(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t_migration::VL_SCHEMA_SQL;

    /// In-memory store used by the tests.
    struct MemStore {
        conn: Mutex<Connection>,
    }

    impl MemStore {
        fn new() -> Self {
            let conn = Connection::open_in_memory().unwrap();
            // The annotation table references afiles; the parent table must exist.
            conn.execute_batch(
                "PRAGMA foreign_keys=ON;
                 CREATE TABLE afiles(id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                 INSERT INTO afiles VALUES(1,'a.jpg'),(2,'b.jpg');",
            )
            .unwrap();
            conn.execute_batch(VL_SCHEMA_SQL).unwrap();
            Self {
                conn: Mutex::new(conn),
            }
        }
    }

    impl VlStore for MemStore {
        fn template(&self, id: i64) -> Result<Option<VlTemplate>, String> {
            let conn = self.conn.lock().unwrap();
            query_template(&conn, id)
        }

        fn templates(&self) -> Result<Vec<VlTemplate>, String> {
            let conn = self.conn.lock().unwrap();
            query_templates(&conn)
        }

        fn insert_template(
            &self,
            name: &str,
            prompt_template: &str,
            system_template: &str,
            response_spec: &str,
            is_builtin: bool,
        ) -> Result<VlTemplate, String> {
            let conn = self.conn.lock().unwrap();
            let version = next_version(&conn, name)?;
            insert_template_row(
                &conn,
                name,
                version,
                prompt_template,
                system_template,
                response_spec,
                is_builtin,
            )
        }

        fn insert_template_version(
            &self,
            base: &VlTemplate,
            name: &str,
            prompt_template: &str,
            system_template: &str,
            response_spec: &str,
        ) -> Result<VlTemplate, String> {
            let conn = self.conn.lock().unwrap();
            insert_template_version_row(
                &conn,
                base,
                name,
                prompt_template,
                system_template,
                response_spec,
            )
        }

        fn delete_template(&self, id: i64) -> Result<bool, String> {
            let conn = self.conn.lock().unwrap();
            delete_template_row(&conn, id)
        }

        fn annotation(
            &self,
            file_id: i64,
            template_id: i64,
            template_version: i64,
        ) -> Result<Option<VlAnnotation>, String> {
            let conn = self.conn.lock().unwrap();
            query_annotation(&conn, file_id, template_id, template_version)
        }

        fn annotations(&self, file_id: i64) -> Result<Vec<VlAnnotation>, String> {
            let conn = self.conn.lock().unwrap();
            query_annotations(&conn, file_id)
        }

        fn upsert_annotation(&self, annotation: &VlAnnotation) -> Result<i64, String> {
            let conn = self.conn.lock().unwrap();
            upsert_annotation_row(&conn, annotation)
        }

        fn delete_annotation(
            &self,
            file_id: i64,
            template_id: i64,
            template_version: i64,
        ) -> Result<(), String> {
            let conn = self.conn.lock().unwrap();
            delete_annotation_row(&conn, file_id, template_id, template_version)
        }

        fn insert_call(&self, call: &VlCallDraft) -> Result<i64, String> {
            let conn = self.conn.lock().unwrap();
            insert_call_row(&conn, call)
        }

        fn calls(&self, file_id: i64) -> Result<Vec<VlCall>, String> {
            let conn = self.conn.lock().unwrap();
            query_calls(&conn, file_id)
        }
    }

    #[test]
    fn create_and_list_templates() {
        let store = MemStore::new();
        let created =
            create_template(&store, "caption", "describe {file_name}", "only json", "{}").unwrap();
        assert_eq!(created.name, "caption");
        assert_eq!(created.version, 1);
        assert!(!created.is_builtin);
        assert!(created.enabled);
        assert_eq!(created.system_template, "only json");

        let listed = list_templates(&store).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, created.id);
        // The system template survives the round trip through the list.
        assert_eq!(listed[0].system_template, "only json");
    }

    #[test]
    fn update_creates_the_next_version_and_keeps_the_old_row() {
        let store = MemStore::new();
        let first = create_template(&store, "caption", "v1 prompt", "v1 system", "{}").unwrap();
        let second = update_template(
            &store,
            first.id,
            "caption",
            "v2 prompt",
            "v2 system",
            "{\"caption\":\"c\"}",
        )
        .unwrap();

        assert_eq!(second.version, 2);
        assert_ne!(second.id, first.id);
        assert_eq!(second.prompt_template, "v2 prompt");
        assert_eq!(second.system_template, "v2 system");

        let old = store.template(first.id).unwrap().unwrap();
        assert_eq!(old.version, 1);
        assert_eq!(old.prompt_template, "v1 prompt");
        assert_eq!(old.system_template, "v1 system");
        assert_eq!(old.response_spec, "{}");

        let listed = list_templates(&store).unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].version, 2);
        assert_eq!(listed[0].system_template, "v2 system");
        assert_eq!(listed[1].version, 1);
        assert_eq!(listed[1].system_template, "v1 system");
    }

    #[test]
    fn a_template_without_a_system_template_keeps_it_empty() {
        let store = MemStore::new();
        let created = create_template(&store, "caption", "prompt", "", "{}").unwrap();
        assert_eq!(created.system_template, "");
        let listed = list_templates(&store).unwrap();
        assert_eq!(listed[0].system_template, "");
    }

    #[test]
    fn editing_a_builtin_template_creates_a_custom_copy() {
        let store = MemStore::new();
        let builtin = store
            .insert_template(
                "builtin caption",
                "builtin prompt",
                "builtin system",
                "{}",
                true,
            )
            .unwrap();
        assert!(builtin.is_builtin);
        assert_eq!(builtin.version, 1);

        let copy = update_template(
            &store,
            builtin.id,
            "builtin caption",
            "custom prompt",
            "custom system",
            "{\"caption\":\"c\"}",
        )
        .unwrap();

        // The copy is a new row of the user: the next version, not a built-in.
        assert_eq!(copy.version, 2);
        assert!(!copy.is_builtin);
        assert_ne!(copy.id, builtin.id);
        assert_eq!(copy.prompt_template, "custom prompt");
        assert_eq!(copy.system_template, "custom system");

        // The built-in row is untouched.
        let original = store.template(builtin.id).unwrap().unwrap();
        assert_eq!(original.version, 1);
        assert!(original.is_builtin);
        assert_eq!(original.prompt_template, "builtin prompt");
        assert_eq!(original.system_template, "builtin system");

        // The copy belongs to the user: it can be deleted, the built-in cannot.
        assert!(delete_template(&store, copy.id).unwrap());
        assert!(delete_template(&store, builtin.id).is_err());
    }

    #[test]
    fn editing_a_builtin_template_twice_does_not_clash_with_the_first_copy() {
        let store = MemStore::new();
        let builtin = store
            .insert_template("builtin caption", "builtin prompt", "system", "{}", true)
            .unwrap();
        let first = update_template(&store, builtin.id, "builtin caption", "one", "system", "{}")
            .unwrap();
        let second =
            update_template(&store, builtin.id, "builtin caption", "two", "system", "{}").unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.version, second.version);
        assert!(!first.is_builtin);
        assert!(!second.is_builtin);
        // The built-in row still exists.
        assert!(store.template(builtin.id).unwrap().is_some());
    }

    #[test]
    fn builtin_templates_cannot_be_deleted() {
        let store = MemStore::new();
        let builtin = store
            .insert_template(
                "builtin caption",
                "builtin prompt",
                "builtin system",
                "{}",
                true,
            )
            .unwrap();
        assert!(builtin.is_builtin);
        assert!(delete_template(&store, builtin.id).is_err());
        assert!(store.template(builtin.id).unwrap().is_some());
    }

    #[test]
    fn deleting_a_template_removes_it_and_reports_missing_rows() {
        let store = MemStore::new();
        let template = create_template(&store, "caption", "prompt", "system", "{}").unwrap();
        assert!(delete_template(&store, template.id).unwrap());
        assert!(store.template(template.id).unwrap().is_none());
        assert!(delete_template(&store, template.id).is_err());
    }

    #[test]
    fn empty_names_and_prompts_are_rejected() {
        let store = MemStore::new();
        assert!(create_template(&store, "  ", "prompt", "system", "{}").is_err());
        assert!(create_template(&store, "name", "   ", "system", "{}").is_err());
    }

    fn ctx(
        file_name: Option<&str>,
        exif_summary: Option<&str>,
        taken_at: Option<&str>,
        camera: Option<&str>,
    ) -> PromptContext {
        PromptContext {
            file_name: file_name.map(str::to_string),
            exif_summary: exif_summary.map(str::to_string),
            taken_at: taken_at.map(str::to_string),
            camera: camera.map(str::to_string),
        }
    }

    #[test]
    fn render_replaces_every_known_placeholder() {
        let context = ctx(
            Some("IMG_0001.jpg"),
            Some("f/1.8 1/60s ISO400"),
            Some("2024-05-01 10:00:00"),
            Some("Canon EOS R5"),
        );
        let rendered = render_prompt(
            "File: {file_name}\nEXIF: {exif_summary}\nTaken: {taken_at}\nCamera: {camera}",
            &context,
        );
        assert_eq!(
            rendered,
            "File: IMG_0001.jpg\nEXIF: f/1.8 1/60s ISO400\nTaken: 2024-05-01 10:00:00\nCamera: Canon EOS R5"
        );
    }

    #[test]
    fn missing_values_render_as_empty_strings() {
        let context = ctx(Some("a.jpg"), None, None, None);
        let rendered = render_prompt(
            "[{file_name}][{exif_summary}][{taken_at}][{camera}]",
            &context,
        );
        assert_eq!(rendered, "[a.jpg][][][]");
    }

    #[test]
    fn unknown_placeholders_are_kept_verbatim() {
        let context = ctx(Some("a.jpg"), None, None, None);
        let rendered = render_prompt("{file_name} {unknown} {{literal}}", &context);
        assert_eq!(rendered, "a.jpg {unknown} {{literal}}");
    }

    #[test]
    fn the_system_template_uses_the_same_placeholders_as_the_prompt() {
        let context = ctx(
            Some("IMG_0001.jpg"),
            Some("f/1.8 1/60s ISO400"),
            Some("2024-05-01 10:00:00"),
            Some("Canon EOS R5"),
        );
        let rendered = render_system(
            "File: {file_name}\nEXIF: {exif_summary}\nTaken: {taken_at}\nCamera: {camera}",
            &context,
        );
        assert_eq!(
            rendered,
            "File: IMG_0001.jpg\nEXIF: f/1.8 1/60s ISO400\nTaken: 2024-05-01 10:00:00\nCamera: Canon EOS R5"
        );
    }

    #[test]
    fn the_system_template_renders_missing_values_as_empty_strings() {
        let context = ctx(Some("a.jpg"), None, None, None);
        let rendered = render_system(
            "[{file_name}][{exif_summary}][{taken_at}][{camera}]",
            &context,
        );
        assert_eq!(rendered, "[a.jpg][][][]");
    }

    #[test]
    fn the_system_template_keeps_unknown_placeholders_verbatim() {
        let context = ctx(Some("a.jpg"), None, None, None);
        let rendered = render_system("{file_name} {unknown} {{literal}}", &context);
        assert_eq!(rendered, "a.jpg {unknown} {{literal}}");
    }

    #[test]
    fn an_empty_system_template_renders_as_an_empty_string() {
        let context = ctx(Some("a.jpg"), None, None, None);
        assert_eq!(render_system("", &context), "");
        assert_eq!(render_system("   ", &context), "");
    }

    #[test]
    fn context_is_built_from_file_metadata() {
        let mut file = crate::t_sqlite::AFile {
            name: "IMG_2.jpg".to_string(),
            taken_date: Some(1_700_000_000),
            e_make: Some("FUJIFILM".to_string()),
            e_model: Some("X-T5".to_string()),
            e_exposure_time: Some("1/125".to_string()),
            e_f_number: Some("f/2.8".to_string()),
            e_iso_speed: Some("800".to_string()),
            e_lens_model: Some("XF35mm".to_string()),
            ..Default::default()
        };
        let context = prompt_context(&file);
        assert_eq!(context.file_name.as_deref(), Some("IMG_2.jpg"));
        assert_eq!(context.camera.as_deref(), Some("FUJIFILM X-T5"));
        assert!(context.taken_at.is_some());
        let summary = context.exif_summary.clone().unwrap_or_default();
        assert!(summary.contains("1/125"));
        assert!(summary.contains("f/2.8"));
        assert!(summary.contains("800"));
        assert!(summary.contains("XF35mm"));

        // Metadata that is missing leaves the fields empty instead of failing.
        file.e_make = None;
        file.e_model = None;
        file.taken_date = None;
        file.e_exposure_time = None;
        file.e_f_number = None;
        file.e_iso_speed = None;
        file.e_lens_model = None;
        let context = prompt_context(&file);
        assert!(context.camera.is_none());
        assert!(context.taken_at.is_none());
        assert!(context.exif_summary.is_none());
    }

    // ----- response parsing -----

    #[test]
    fn thinking_block_before_text_is_skipped() {
        let body = r#"{"content":[{"type":"thinking","thinking":"reasoning"},{"type":"text","text":"{\"caption\":\"a cat\"}"}],
            "id":"msg_1","model":"group/auto-deepseek-v4-1-flash","stop_reason":"end_turn",
            "usage":{"input_tokens":71,"output_tokens":205,"cache_read_input_tokens":384,
                     "output_tokens_details":{"thinking_tokens":144}}}"#;
        let response = parse_response(body).unwrap();
        assert_eq!(response.text, "{\"caption\":\"a cat\"}");
        assert_eq!(response.thinking.as_deref(), Some("reasoning"));
        assert_eq!(
            response.model.as_deref(),
            Some("group/auto-deepseek-v4-1-flash")
        );
        assert_eq!(response.request_id.as_deref(), Some("msg_1"));
        assert_eq!(response.stop_reason.as_deref(), Some("end_turn"));
    }

    #[test]
    fn multiple_text_blocks_are_joined_in_order() {
        let body =
            r#"{"content":[{"type":"text","text":"first "},{"type":"text","text":"second"}]}"#;
        let response = parse_response(body).unwrap();
        assert_eq!(response.text, "first second");
    }

    #[test]
    fn fenced_json_is_unwrapped_and_parsed() {
        let body = "{\"content\":[{\"type\":\"text\",\"text\":\"```json\\n{\\\"caption\\\":\\\"a cat\\\"}\\n```\"}]}";
        let response = parse_response(body).unwrap();
        assert_eq!(response.text, "```json\n{\"caption\":\"a cat\"}\n```");
        let (value, unparsed) = result_json(&response.text, "{}");
        assert!(!unparsed);
        assert_eq!(value["caption"], serde_json::json!("a cat"));
    }

    #[test]
    fn broken_json_keeps_the_original_text_and_is_marked_unparsed() {
        let (value, unparsed) = result_json("not json at all", "{}");
        assert!(unparsed);
        assert_eq!(value, serde_json::json!({"_unparsed": true}));
    }

    #[test]
    fn empty_content_reports_a_readable_reason() {
        let body = r#"{"content":[],"stop_reason":"end_turn"}"#;
        let response = parse_response(body).unwrap();
        assert!(response.text.is_empty());
        let reason = response.missing_body_reason().unwrap();
        assert!(reason.contains("没有正文内容"));

        let body = r#"{"content":[{"type":"thinking","thinking":"only reasoning"}],"stop_reason":"max_tokens"}"#;
        let response = parse_response(body).unwrap();
        let reason = response.missing_body_reason().unwrap();
        assert!(reason.contains("max_tokens"));
    }

    #[test]
    fn usage_fields_are_mapped_from_the_anthropic_shape() {
        let body = r#"{"content":[{"type":"text","text":"x"}],
            "usage":{"input_tokens":71,"output_tokens":205,"cache_read_input_tokens":384,
                     "output_tokens_details":{"thinking_tokens":144}}}"#;
        let response = parse_response(body).unwrap();
        assert_eq!(response.usage.input_tokens, Some(71));
        assert_eq!(response.usage.output_tokens, Some(205));
        assert_eq!(response.usage.cache_read_tokens, Some(384));
        assert_eq!(response.usage.thinking_tokens, Some(144));
    }

    #[test]
    fn a_body_that_is_not_json_is_rejected() {
        assert!(parse_response("<html>gateway error</html>").is_err());
    }

    #[test]
    fn result_json_keeps_only_the_declared_spec_fields() {
        let text = r#"{"caption":"a cat","tags":["cat","pet"],"extra":1}"#;
        let (value, unparsed) = result_json(text, r#"{"caption":"描述","tags":"标签数组"}"#);
        assert!(!unparsed);
        assert_eq!(value["caption"], serde_json::json!("a cat"));
        assert_eq!(value["tags"], serde_json::json!(["cat", "pet"]));
        assert!(value.get("extra").is_none());
    }

    #[test]
    fn result_json_keeps_every_field_when_the_spec_is_unusable() {
        let text = r#"{"caption":"a cat"}"#;
        let (value, unparsed) = result_json(text, "not json");
        assert!(!unparsed);
        assert_eq!(value["caption"], serde_json::json!("a cat"));
    }

    // ----- gateway client -----

    /// Minimal HTTP server used to exercise the gateway client.
    mod mock {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        /// A running mock server plus the number of requests it received.
        pub struct MockGateway {
            pub url: String,
            pub requests: Arc<AtomicUsize>,
            pub bodies: Arc<Mutex<Vec<String>>>,
        }

        /// Start a server that answers every request with `status` and `body`.
        pub async fn spawn(status: &'static str, body: &'static str) -> MockGateway {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let bodies = Arc::new(Mutex::new(Vec::new()));
            let counter = requests.clone();
            let collected = bodies.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut buffer = vec![0u8; 64 * 1024];
                    let read = match stream.read(&mut buffer).await {
                        Ok(read) => read,
                        Err(_) => continue,
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                    if let Some(index) = request.find("\r\n\r\n") {
                        let head = &request[..index];
                        let mut length = 0usize;
                        for line in head.lines() {
                            let Some((name, value)) = line.split_once(':') else {
                                continue;
                            };
                            if name.trim().eq_ignore_ascii_case("content-length") {
                                length = value.trim().parse().unwrap_or(0);
                            }
                        }
                        let body_start = index + 4;
                        let mut payload = request[body_start..].to_string();
                        while payload.len() < length {
                            let mut more = vec![0u8; 64 * 1024];
                            let read = match stream.read(&mut more).await {
                                Ok(read) => read,
                                Err(_) => break,
                            };
                            if read == 0 {
                                break;
                            }
                            payload.push_str(&String::from_utf8_lossy(&more[..read]));
                        }
                        collected.lock().unwrap().push(payload);
                    }
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.flush().await;
                }
            });
            MockGateway {
                url: format!("http://{address}"),
                requests,
                bodies,
            }
        }

        /// Start a server that accepts connections but never answers.
        pub async fn spawn_silent() -> MockGateway {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let counter = requests.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let mut buffer = vec![0u8; 1024];
                    let _ = stream.read(&mut buffer).await;
                    std::future::pending::<()>().await;
                }
            });
            MockGateway {
                url: format!("http://{address}"),
                requests,
                bodies: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    fn test_request() -> VlRequest {
        VlRequest {
            image_base64: "aGVsbG8=".to_string(),
            media_type: "image/jpeg".to_string(),
            prompt: "describe this photo".to_string(),
            system: Some("be brief".to_string()),
        }
    }

    fn test_config(url: &str) -> VlGatewayConfig {
        VlGatewayConfig {
            gateway_url: url.to_string(),
            api_key: "test-key-not-a-real-secret".to_string(),
            model: "auto/deepseek-v4.1-flash".to_string(),
        }
    }

    #[tokio::test]
    async fn gateway_success_is_parsed() {
        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[{"type":"text","text":"{\"caption\":\"a cat\"}"}],"id":"msg_1",
                "model":"group/auto-deepseek-v4-1-flash","stop_reason":"end_turn",
                "usage":{"input_tokens":71,"output_tokens":205}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let outcome = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap();
        assert_eq!(outcome.response.text, "{\"caption\":\"a cat\"}");
        assert_eq!(
            outcome.response.model.as_deref(),
            Some("group/auto-deepseek-v4-1-flash")
        );
        assert_eq!(outcome.response.usage.input_tokens, Some(71));
        assert!(outcome.duration_ms >= 0);
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);

        // The request carries the Anthropic protocol shape.
        let bodies = gateway.bodies.lock().unwrap();
        let body: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(body["model"], json!("auto/deepseek-v4.1-flash"));
        assert_eq!(body["max_tokens"], json!(VL_MAX_TOKENS));
        assert_eq!(body["system"], json!("be brief"));
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][0]["content"][0]["type"], json!("image"));
        assert_eq!(
            body["messages"][0]["content"][0]["source"]["type"],
            json!("base64")
        );
        assert_eq!(
            body["messages"][0]["content"][0]["source"]["media_type"],
            json!("image/jpeg")
        );
        // The image is bare base64 without a data: prefix.
        assert_eq!(
            body["messages"][0]["content"][0]["source"]["data"],
            json!("aGVsbG8=")
        );
        assert_eq!(body["messages"][0]["content"][1]["type"], json!("text"));
        assert_eq!(
            body["messages"][0]["content"][1]["text"],
            json!("describe this photo")
        );
    }

    #[tokio::test]
    async fn an_empty_system_is_not_sent_to_the_gateway() {
        let gateway = mock::spawn("200 OK", r#"{"content":[{"type":"text","text":"x"}]}"#).await;
        let client = reqwest::Client::new();
        let request = VlRequest {
            system: None,
            ..test_request()
        };
        call_gateway(&client, &test_config(&gateway.url), &request)
            .await
            .unwrap();

        let bodies = gateway.bodies.lock().unwrap();
        let body: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert!(
            body.get("system").is_none(),
            "an empty system must not be part of the request"
        );
    }

    #[tokio::test]
    async fn a_system_is_sent_to_the_gateway_as_a_top_level_field() {
        let gateway = mock::spawn("200 OK", r#"{"content":[{"type":"text","text":"x"}]}"#).await;
        let client = reqwest::Client::new();
        let request = VlRequest {
            system: Some("只输出 JSON".to_string()),
            ..test_request()
        };
        call_gateway(&client, &test_config(&gateway.url), &request)
            .await
            .unwrap();

        let bodies = gateway.bodies.lock().unwrap();
        let body: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(body["system"], json!("只输出 JSON"));
    }

    #[tokio::test]
    async fn gateway_authentication_failure_is_reported_without_the_key() {
        let gateway = mock::spawn(
            "401 Unauthorized",
            r#"{"error":{"code":null,"message":"invalid api key","param":null,"type":"authentication_error"}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let error = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap_err();
        assert!(error.message.contains("401"));
        assert!(error.message.contains("鉴权"));
        assert!(!error.message.contains("test-key-not-a-real-secret"));
    }

    #[tokio::test]
    async fn gateway_unknown_model_is_reported() {
        let gateway = mock::spawn(
            "404 Not Found",
            r#"{"error":{"message":"magpie knows no model auto/nope"}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let error = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap_err();
        assert!(error.message.contains("404"));
        assert!(error.message.contains("magpie knows no model"));
    }

    #[tokio::test]
    async fn gateway_timeout_is_reported() {
        let gateway = mock::spawn_silent().await;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(300))
            .build()
            .unwrap();
        let error = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap_err();
        assert!(error.message.contains("超时") || error.message.contains("timeout"));
    }

    #[tokio::test]
    async fn gateway_broken_json_is_reported() {
        let gateway = mock::spawn("200 OK", "<html>not json</html>").await;
        let client = reqwest::Client::new();
        let error = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap_err();
        assert!(error.message.contains("JSON"));
    }

    #[tokio::test]
    async fn gateway_response_without_text_is_reported() {
        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[{"type":"thinking","thinking":"only reasoning"}],"stop_reason":"max_tokens"}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let error = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap_err();
        assert!(error.message.contains("max_tokens"));
    }

    #[tokio::test]
    async fn gateway_without_configuration_is_rejected() {
        let client = reqwest::Client::new();
        let config = VlGatewayConfig {
            gateway_url: String::new(),
            api_key: "test-key-not-a-real-secret".to_string(),
            model: VL_DEFAULT_MODEL.to_string(),
        };
        let error = call_gateway(&client, &config, &test_request())
            .await
            .unwrap_err();
        assert!(error.message.contains("未配置"));
    }

    #[tokio::test]
    async fn gateway_echoes_the_key_back_are_redacted() {
        let gateway = mock::spawn(
            "401 Unauthorized",
            r#"{"error":{"message":"key test-key-not-a-real-secret is invalid","type":"authentication_error"}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let error = call_gateway(&client, &test_config(&gateway.url), &test_request())
            .await
            .unwrap_err();
        assert!(!error.message.contains("test-key-not-a-real-secret"));
        let raw = error.raw_body.unwrap();
        assert!(!raw.contains("test-key-not-a-real-secret"));
    }

    #[test]
    fn the_config_view_never_carries_the_api_key() {
        let config = VlGatewayConfig {
            gateway_url: "http://127.0.0.1:1".to_string(),
            api_key: "test-key-not-a-real-secret".to_string(),
            model: VL_DEFAULT_MODEL.to_string(),
        };
        assert!(is_configured(&config));
        // The view is built from the gateway URL, the model and a flag only.
        let view = VlConfigView {
            gateway_url: config.gateway_url.clone(),
            model: config.model.clone(),
            configured: is_configured(&config),
        };
        let serialized = serde_json::to_string(&view).unwrap();
        assert!(!serialized.contains("test-key-not-a-real-secret"));
        assert!(serialized.contains("configured"));
    }

    // ----- orchestration -----

    /// Source that serves a fixed image and prompt context.
    struct StubSource;

    impl VlSource for StubSource {
        fn prompt_context(&self, file_id: i64) -> Result<Option<PromptContext>, String> {
            Ok(Some(PromptContext {
                file_name: Some(format!("file-{file_id}.jpg")),
                exif_summary: Some("f/1.8".to_string()),
                taken_at: Some("2024-05-01 10:00:00".to_string()),
                camera: Some("Test Camera".to_string()),
            }))
        }

        fn image(&self, _file_id: i64) -> Result<Option<VlImage>, String> {
            Ok(Some(VlImage {
                bytes: vec![0xFF, 0xD8, 0xFF, 0x00],
                media_type: "image/jpeg".to_string(),
            }))
        }
    }

    /// Source whose file has no readable image.
    struct NoImageSource;

    impl VlSource for NoImageSource {
        fn prompt_context(&self, file_id: i64) -> Result<Option<PromptContext>, String> {
            StubSource.prompt_context(file_id)
        }

        fn image(&self, _file_id: i64) -> Result<Option<VlImage>, String> {
            Ok(None)
        }
    }

    fn analysis_template(store: &MemStore) -> VlTemplate {
        create_template(store, "caption", "describe {file_name}", "", "{}").unwrap()
    }

    /// Store an existing annotation the way a previous run would have.
    fn seed_annotation(store: &MemStore, template: &VlTemplate, file_id: i64) {
        let call_id = store
            .insert_call(&VlCallDraft {
                file_id,
                template_id: template.id,
                template_version: template.version,
                prompt_actual: "describe".to_string(),
                status: "ok".to_string(),
                ..Default::default()
            })
            .unwrap();
        store
            .upsert_annotation(&VlAnnotation {
                id: 0,
                file_id,
                template_id: template.id,
                template_version: template.version,
                model: "previous-model".to_string(),
                result_json: "{\"caption\":\"old\"}".to_string(),
                raw_text: "old".to_string(),
                call_id,
                created_at: 1,
            })
            .unwrap();
    }

    #[tokio::test]
    async fn an_existing_annotation_skips_the_call_entirely() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        seed_annotation(&store, &template, 1);

        let gateway = mock::spawn("200 OK", r#"{"content":[{"type":"text","text":"new"}]}"#).await;
        let client = reqwest::Client::new();
        let outcome = analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        assert_eq!(outcome, VlFileOutcome::Skipped);
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);

        // The stored annotation is untouched.
        let annotation = store
            .annotation(1, template.id, template.version)
            .unwrap()
            .unwrap();
        assert_eq!(annotation.model, "previous-model");
        assert_eq!(annotation.result_json, "{\"caption\":\"old\"}");
    }

    #[tokio::test]
    async fn force_replaces_the_existing_annotation_and_calls_the_gateway() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        seed_annotation(&store, &template, 1);

        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[{"type":"text","text":"{\"caption\":\"new\"}"}],"model":"group/auto-deepseek-v4-1-flash","id":"msg_9","stop_reason":"end_turn"}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let outcome = analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            true,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, VlFileOutcome::Annotated { .. }));
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);

        let annotation = store
            .annotation(1, template.id, template.version)
            .unwrap()
            .unwrap();
        assert_eq!(annotation.model, "group/auto-deepseek-v4-1-flash");
        assert_eq!(annotation.result_json, "{\"caption\":\"new\"}");
        assert_eq!(annotation.raw_text, "{\"caption\":\"new\"}");
    }

    #[tokio::test]
    async fn an_annotation_of_another_template_version_does_not_block_the_call() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        let call_id = store
            .insert_call(&VlCallDraft {
                file_id: 1,
                template_id: template.id,
                template_version: template.version + 1,
                prompt_actual: "describe".to_string(),
                status: "ok".to_string(),
                ..Default::default()
            })
            .unwrap();
        store
            .upsert_annotation(&VlAnnotation {
                id: 0,
                file_id: 1,
                template_id: template.id,
                template_version: template.version + 1,
                model: "other".to_string(),
                result_json: "{}".to_string(),
                raw_text: "other".to_string(),
                call_id,
                created_at: 1,
            })
            .unwrap();

        let gateway = mock::spawn("200 OK", r#"{"content":[{"type":"text","text":"new"}]}"#).await;
        let client = reqwest::Client::new();
        let outcome = analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        assert!(matches!(outcome, VlFileOutcome::Annotated { .. }));
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);
    }

    // ----- evidence -----

    #[tokio::test]
    async fn a_successful_call_leaves_the_full_evidence() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[{"type":"thinking","thinking":"why"},{"type":"text","text":"{\"caption\":\"a cat\"}"}],
                "id":"msg_42","model":"group/auto-deepseek-v4-1-flash","stop_reason":"end_turn",
                "usage":{"input_tokens":71,"output_tokens":205,"cache_read_input_tokens":384,
                         "output_tokens_details":{"thinking_tokens":144}}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        let calls = store.calls(1).unwrap();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call.status, "ok");
        assert_eq!(call.file_id, 1);
        assert_eq!(call.template_id, template.id);
        assert_eq!(call.template_version, template.version);
        // The echoed model is recorded, not the requested one.
        assert_eq!(
            call.model.as_deref(),
            Some("group/auto-deepseek-v4-1-flash")
        );
        assert_eq!(call.request_id.as_deref(), Some("msg_42"));
        assert_eq!(call.input_tokens, Some(71));
        assert_eq!(call.output_tokens, Some(205));
        assert_eq!(call.cache_read_tokens, Some(384));
        assert_eq!(call.thinking_tokens, Some(144));
        assert_eq!(call.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(call.thinking_text.as_deref(), Some("why"));
        assert!(call.duration_ms.is_some_and(|value| value >= 0));
        assert!(call.raw_response.is_some());
        assert!(call.error.is_none());
        // The rendered prompt is recorded.
        assert_eq!(call.prompt_actual, "describe file-1.jpg");
        // A template without a system template records no system.
        assert_eq!(call.system_actual, None);
    }

    #[tokio::test]
    async fn the_rendered_system_is_stored_as_call_evidence() {
        let store = MemStore::new();
        let template = create_template(
            &store,
            "caption",
            "describe {file_name}",
            "只输出 JSON，字段形如 {caption, tags}；拍摄于 {taken_at}，相机 {camera}",
            "{}",
        )
        .unwrap();
        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[{"type":"text","text":"{\"caption\":\"a cat\"}"}],"model":"group/auto-deepseek-v4-1-flash"}"#,
        )
        .await;
        let client = reqwest::Client::new();
        analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        let calls = store.calls(1).unwrap();
        assert_eq!(calls.len(), 1);
        // The system is rendered with the same placeholders as the prompt.
        assert_eq!(
            calls[0].system_actual.as_deref(),
            Some(
                "只输出 JSON，字段形如 {caption, tags}；拍摄于 2024-05-01 10:00:00，相机 Test Camera"
            )
        );

        // The rendered system is also what the gateway receives.
        let bodies = gateway.bodies.lock().unwrap();
        let body: Value = serde_json::from_str(&bodies[0]).unwrap();
        assert_eq!(
            body["system"],
            json!(
                "只输出 JSON，字段形如 {caption, tags}；拍摄于 2024-05-01 10:00:00，相机 Test Camera"
            )
        );
    }

    #[tokio::test]
    async fn a_failed_call_leaves_evidence_but_no_annotation() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        let gateway = mock::spawn(
            "401 Unauthorized",
            r#"{"error":{"message":"invalid api key","type":"authentication_error"}}"#,
        )
        .await;
        let client = reqwest::Client::new();
        let outcome = analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        let VlFileOutcome::Failed { call_id, error } = outcome else {
            panic!("expected a failed outcome, got {outcome:?}");
        };
        assert!(error.contains("鉴权"));
        assert!(!error.contains("test-key-not-a-real-secret"));

        let calls = store.calls(1).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, call_id);
        assert_eq!(calls[0].status, "error");
        assert!(calls[0].error.is_some());
        assert!(calls[0].raw_response.is_some());

        // A failed call produces no annotation.
        assert!(
            store
                .annotation(1, template.id, template.version)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_file_without_an_image_leaves_evidence_and_does_not_call_the_gateway() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        let gateway = mock::spawn("200 OK", r#"{"content":[{"type":"text","text":"x"}]}"#).await;
        let client = reqwest::Client::new();
        let outcome = analyze_file(
            &store,
            &NoImageSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        let VlFileOutcome::Failed { error, .. } = outcome else {
            panic!("expected a failed outcome, got {outcome:?}");
        };
        assert!(error.contains("缩略图"));
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);

        let calls = store.calls(1).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].status, "error");
        assert!(calls[0].error.is_some());
        assert!(
            store
                .annotation(1, template.id, template.version)
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_annotation_points_at_the_call_that_produced_it() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[{"type":"text","text":"{\"caption\":\"a cat\"}"}],"model":"group/auto-deepseek-v4-1-flash"}"#,
        )
        .await;
        let client = reqwest::Client::new();
        analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            1,
            &template,
            false,
        )
        .await
        .unwrap();

        let calls = store.calls(1).unwrap();
        let annotations = store.annotations(1).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(annotations.len(), 1);
        assert_eq!(annotations[0].call_id, calls[0].id);
        assert_eq!(annotations[0].result_json, "{\"caption\":\"a cat\"}");
    }

    // ----- command registration -----

    /// The commands this module contributes to the Tauri handler.
    const VL_COMMANDS: [&str; 10] = [
        "get_vl_config",
        "set_vl_config",
        "list_vl_templates",
        "create_vl_template",
        "update_vl_template",
        "delete_vl_template",
        "run_vl_analysis",
        "get_vl_annotations",
        "get_vl_calls",
        "cancel_vl_run",
    ];

    #[test]
    fn every_command_is_registered_in_main() {
        let main = include_str!("main.rs");
        let handler = main
            .split("generate_handler![")
            .nth(1)
            .and_then(|rest| rest.split("])").next())
            .expect("main.rs must register a handler list");
        for command in VL_COMMANDS {
            assert!(
                handler.contains(&format!("t_vl::{command}")),
                "command {command} is not registered in main.rs"
            );
        }
    }

    #[test]
    fn the_handler_does_not_register_unknown_vl_commands() {
        let main = include_str!("main.rs");
        let handler = main
            .split("generate_handler![")
            .nth(1)
            .and_then(|rest| rest.split("])").next())
            .expect("main.rs must register a handler list");
        let registered: Vec<&str> = handler
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("t_vl::"))
            .map(|line| line.trim_end_matches(','))
            .collect();
        assert_eq!(
            registered.len(),
            VL_COMMANDS.len(),
            "unexpected vl commands in main.rs: {registered:?}"
        );
        for command in registered {
            assert!(
                VL_COMMANDS.contains(&command),
                "main.rs registers an unknown vl command: {command}"
            );
        }
    }

    /// 手动验收：打真实网关，走一张本地图片。
    ///
    /// 需要三个环境变量，缺一即 panic（这是有意的手动工具，不是日常门禁）：
    ///   - `LAP_VL_GATEWAY_URL`  网关地址
    ///   - `LAP_VL_API_KEY`     网关 key
    ///   - `LAP_VL_PROBE_IMAGE` 本地图片路径
    ///
    /// 跑法：`cargo test -- --ignored real_gateway_probe --nocapture`
    ///
    /// 它覆盖的是 mock server 覆盖不到的一段：真实网关的真实响应形状
    /// （thinking 块、model 回显值、usage 字段、真实时延）。
    #[tokio::test]
    #[ignore = "需要真实网关与本地图片，手动验收用"]
    async fn real_gateway_probe() {
        let gateway_url =
            std::env::var(crate::t_common::VL_GATEWAY_URL_ENV).expect("set LAP_VL_GATEWAY_URL");
        let api_key = std::env::var(crate::t_common::VL_API_KEY_ENV).expect("set LAP_VL_API_KEY");
        let image_path = std::env::var("LAP_VL_PROBE_IMAGE").expect("set LAP_VL_PROBE_IMAGE");

        let bytes = std::fs::read(&image_path).expect("read probe image");
        let extension = image_path
            .to_ascii_lowercase()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_string();
        let media_type = match extension.as_str() {
            "png" => "image/png",
            "webp" => "image/webp",
            "gif" => "image/gif",
            _ => "image/jpeg",
        };

        let config = VlGatewayConfig {
            gateway_url,
            api_key,
            model: crate::t_common::VL_DEFAULT_MODEL.to_string(),
        };
        let request = VlRequest {
            image_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            media_type: media_type.to_string(),
            prompt: "用一句中文描述这张照片，并给出 3 个中文标签。输出 JSON：{\"caption\":\"…\",\"tags\":[\"…\"]}"
                .to_string(),
            system: Some("你是一个相册标注助手。只输出 JSON，不要解释。".to_string()),
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                crate::t_common::VL_REQUEST_TIMEOUT_SECS,
            ))
            .build()
            .expect("build client");
        let outcome = call_gateway(&client, &config, &request)
            .await
            .unwrap_or_else(|error| panic!("gateway call failed: {}", error.message));
        let response = outcome.response;

        println!("model       = {:?}", response.model);
        println!("request_id  = {:?}", response.request_id);
        println!("stop_reason = {:?}", response.stop_reason);
        println!("usage       = {:?}", response.usage);
        println!("duration_ms = {}", outcome.duration_ms);
        println!(
            "thinking    = {}",
            response.thinking.as_deref().unwrap_or("")
        );
        println!("text        = {}", response.text);

        assert!(
            !response.text.trim().is_empty(),
            "gateway returned no text block: {:?}",
            response.missing_body_reason()
        );
    }

    /// 手动验收：真实库 → 缩略图 → 网关 → 标注入库（完整生产链路）。
    ///
    /// 与 `real_gateway_probe` 的区别：那一支直接读本地图片文件，这一支走的是
    /// **生产路径**——从当前库里取文件、取缓存缩略图、渲染模板、落标注与证据。
    /// debug 构建指向 `com.julyx10.lap.debug` 数据目录。
    ///
    /// 需要 `LAP_VL_GATEWAY_URL` 与 `LAP_VL_API_KEY`；会真的发一次请求（可能 1–2 分钟）。
    /// 跑法：`cargo test -- --ignored real_library_e2e --nocapture`
    #[tokio::test]
    #[ignore = "需要真实网关与真实库，手动验收用"]
    async fn real_library_e2e() {
        crate::t_sqlite::create_db().expect("create_db");
        let config = resolve_gateway_config().expect("set LAP_VL_GATEWAY_URL / LAP_VL_API_KEY");

        let store = SqliteVlStore;
        let source = LibrarySource;
        let templates = list_templates(&store).expect("list templates");
        let template = templates
            .iter()
            .find(|template| template.is_builtin)
            .or_else(|| templates.first())
            .cloned()
            .expect("at least one template");

        let file_id: i64 = {
            let conn = crate::t_sqlite::open_conn().expect("open connection");
            conn.query_row(
                "SELECT id FROM afiles WHERE file_type IN (1, 3) ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .expect("a photo in the library")
        };
        println!("file_id = {file_id}, template = {} v{}", template.name, template.version);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(
                crate::t_common::VL_REQUEST_TIMEOUT_SECS,
            ))
            .build()
            .expect("build client");
        let outcome = analyze_file(
            &store,
            &source,
            &client,
            &config,
            file_id,
            &template,
            true,
        )
        .await
        .expect("analyze_file");
        println!("outcome = {outcome:?}");

        let annotation = store
            .annotation(file_id, template.id, template.version)
            .expect("read annotation")
            .expect("an annotation row must exist after a successful call");
        println!("result_json = {}", annotation.result_json);
        println!("raw_text    = {}", annotation.raw_text);
        assert!(
            !annotation.raw_text.trim().is_empty(),
            "the annotation must keep the model's own text"
        );

        let calls = store.calls(file_id).expect("read calls");
        let call = calls.first().expect("call evidence");
        println!(
            "evidence: model={:?} request_id={:?} usage_in={:?} usage_out={:?} duration={:?}ms status={}",
            call.model,
            call.request_id,
            call.input_tokens,
            call.output_tokens,
            call.duration_ms,
            call.status
        );
        assert_eq!(call.status, "ok");
        assert!(call.model.is_some(), "the served model must be recorded");
        assert!(call.request_id.is_some(), "the gateway request id must be recorded");
    }

    /// A library with the tables the annotation writes and the index need.
    fn annotation_fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE afiles(id INTEGER PRIMARY KEY, name TEXT NOT NULL);
             INSERT INTO afiles VALUES(1,'a.jpg');",
        )
        .unwrap();
        conn.execute_batch(VL_SCHEMA_SQL).unwrap();
        conn.execute_batch(crate::t_fts::FTS_SCHEMA_SQL).unwrap();
        conn
    }

    /// A template with the call that an annotation points at.
    fn seed_template(conn: &Connection, id: i64, name: &str) {
        conn.execute(
            "INSERT INTO vl_templates(id,name,version,prompt_template,response_spec,created_at)
             VALUES(?1,?2,1,'p','{}',1)",
            params![id, name],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO vl_calls(id,file_id,template_id,template_version,prompt_actual,status,created_at)
             VALUES(?1,1,?1,1,'p','ok',1)",
            params![id],
        )
        .unwrap();
    }

    /// An annotation with the given caption for a template.
    fn seed_caption(conn: &Connection, template_id: i64, caption: &str) {
        let text = format!("{{\"caption\":\"{}\"}}", caption);
        upsert_annotation_row(
            conn,
            &VlAnnotation {
                id: 0,
                file_id: 1,
                template_id,
                template_version: 1,
                model: "m".to_string(),
                result_json: text.clone(),
                raw_text: text,
                call_id: template_id,
                created_at: 1,
            },
        )
        .unwrap();
    }

    /// The indexed caption of the one file of the fixture.
    fn indexed_caption(conn: &Connection) -> String {
        conn.query_row("SELECT caption FROM files_fts WHERE rowid = 1", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn an_annotation_is_findable_right_away() {
        let conn = annotation_fixture();
        seed_template(&conn, 1, "first");
        seed_caption(&conn, 1, "a cat");

        let hits = crate::t_fts::search(&conn, "cat", 10).unwrap();
        assert_eq!(hits.len(), 1, "the annotation has to be findable");
        assert_eq!(hits[0].file_id, 1);
        assert_eq!(hits[0].matched_columns, vec!["caption".to_string()]);
    }

    #[test]
    fn a_deleted_annotation_is_not_findable_any_more() {
        let conn = annotation_fixture();
        seed_template(&conn, 1, "first");
        seed_caption(&conn, 1, "a cat");
        assert_eq!(crate::t_fts::search(&conn, "cat", 10).unwrap().len(), 1);

        delete_annotation_row(&conn, 1, 1, 1).unwrap();

        assert!(
            crate::t_fts::search(&conn, "cat", 10).unwrap().is_empty(),
            "the deleted annotation must not be findable any more"
        );
        assert!(indexed_caption(&conn).is_empty());
    }

    #[test]
    fn the_caption_column_holds_the_annotations_of_every_template() {
        let conn = annotation_fixture();
        seed_template(&conn, 1, "first");
        seed_template(&conn, 2, "second");
        seed_caption(&conn, 1, "a cat");
        seed_caption(&conn, 2, "a dog");

        let caption = indexed_caption(&conn);
        assert!(caption.contains("cat"), "caption = {caption}");
        assert!(caption.contains("dog"), "caption = {caption}");
    }

    #[test]
    fn a_deleted_template_drops_its_annotations_from_the_index() {
        let conn = annotation_fixture();
        seed_template(&conn, 1, "first");
        seed_template(&conn, 2, "second");
        seed_caption(&conn, 1, "a cat");
        seed_caption(&conn, 2, "a dog");

        delete_template_row(&conn, 1).unwrap();

        let caption = indexed_caption(&conn);
        assert!(!caption.contains("cat"), "caption = {caption}");
        assert!(caption.contains("dog"), "caption = {caption}");
        assert!(crate::t_fts::search(&conn, "cat", 10).unwrap().is_empty());
        assert_eq!(crate::t_fts::search(&conn, "dog", 10).unwrap().len(), 1);
    }
}
