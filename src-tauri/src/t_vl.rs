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
    ///
    /// Kept as the store capability: the orchestrator no longer pre-deletes a
    /// forced run (a failed call must leave the previous annotation in place),
    /// so nothing calls this at the moment.
    #[allow(dead_code)]
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
    .map_err(template_insert_error)?;
    query_template(conn, conn.last_insert_rowid())?.ok_or_else(|| "Template insert failed".into())
}

/// A (name, version) conflict must not surface as a raw SQLite message.
fn template_insert_error(error: rusqlite::Error) -> String {
    let is_unique_conflict = match &error {
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE =>
        {
            true
        }
        _ => false,
    };
    if is_unique_conflict {
        "模板名与版本号冲突，请换一个名字后重试".to_string()
    } else {
        error.to_string()
    }
}

fn next_version(conn: &Connection, name: &str) -> Result<i64, String> {
    conn.query_row(
        "SELECT COALESCE(MAX(version), 0) + 1 FROM vl_templates WHERE name = ?1",
        params![name],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// The version of the next row of a name.
///
/// The preferred version continues the chain of the row the edit came from
/// (a custom copy of a built-in included). When that version is already taken
/// by another row of the same name, the next free version of the name is used
/// instead.
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
    // The preferred version of every edit is the one after the row it came
    // from; when that one is taken (an old row edited again, a rename onto a
    // name whose versions moved on) the next free version of the name is used.
    let version = copy_version(conn, base, name)?;
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
    // The annotation delete and the template delete are one step: a failure
    // in between must not leave the annotations orphaned.
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM vl_annotations WHERE template_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    let deleted = tx
        .execute("DELETE FROM vl_templates WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    // The index is derived data: rebuilding it per file after the commit is
    // enough, but a failure has to be visible instead of swallowed.
    for file_id in file_ids {
        if let Err(e) = crate::t_fts::upsert_doc(conn, file_id) {
            eprintln!("fts index update failed for file {file_id}: {e}");
        }
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
    if let Err(e) = crate::t_fts::upsert_doc(conn, annotation.file_id) {
        eprintln!(
            "fts index update failed for file {}: {e}",
            annotation.file_id
        );
    }
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
    if let Err(e) = crate::t_fts::upsert_doc(conn, file_id) {
        eprintln!("fts index update failed for file {file_id}: {e}");
    }
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
///
/// Every field is read-only metadata of the file: the prompt can describe the
/// photo without the model ever writing back into the library.
#[derive(Debug, Clone, Default)]
pub struct PromptContext {
    // identity and capture
    pub file_name: Option<String>,
    pub exif_summary: Option<String>,
    pub taken_at: Option<String>,
    pub camera: Option<String>,
    // library relations
    pub tags: Option<String>,
    pub people: Option<String>,
    // location
    pub location: Option<String>,
    pub gps: Option<String>,
    // descriptive metadata
    pub comments: Option<String>,
    pub title: Option<String>,
    pub headline: Option<String>,
    pub description: Option<String>,
    pub artist: Option<String>,
    pub copyright: Option<String>,
    pub software: Option<String>,
    // camera and capture details
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub exposure: Option<String>,
    pub f_number: Option<String>,
    pub focal_length: Option<String>,
    pub iso: Option<String>,
    pub flash: Option<String>,
    // file properties
    pub dimensions: Option<String>,
    pub duration: Option<String>,
    pub rating: Option<String>,
    pub favorite: Option<String>,
    pub format: Option<String>,
    pub file_size: Option<String>,
    pub created_at: Option<String>,
    pub modified_at: Option<String>,
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
    let values: [(&str, Option<&str>); 31] = [
        ("{file_name}", context.file_name.as_deref()),
        ("{exif_summary}", context.exif_summary.as_deref()),
        ("{taken_at}", context.taken_at.as_deref()),
        ("{camera}", context.camera.as_deref()),
        ("{tags}", context.tags.as_deref()),
        ("{people}", context.people.as_deref()),
        ("{location}", context.location.as_deref()),
        ("{gps}", context.gps.as_deref()),
        ("{comments}", context.comments.as_deref()),
        ("{title}", context.title.as_deref()),
        ("{headline}", context.headline.as_deref()),
        ("{description}", context.description.as_deref()),
        ("{artist}", context.artist.as_deref()),
        ("{copyright}", context.copyright.as_deref()),
        ("{software}", context.software.as_deref()),
        ("{camera_make}", context.camera_make.as_deref()),
        ("{camera_model}", context.camera_model.as_deref()),
        ("{lens}", context.lens.as_deref()),
        ("{exposure}", context.exposure.as_deref()),
        ("{f_number}", context.f_number.as_deref()),
        ("{focal_length}", context.focal_length.as_deref()),
        ("{iso}", context.iso.as_deref()),
        ("{flash}", context.flash.as_deref()),
        ("{dimensions}", context.dimensions.as_deref()),
        ("{duration}", context.duration.as_deref()),
        ("{rating}", context.rating.as_deref()),
        ("{favorite}", context.favorite.as_deref()),
        ("{format}", context.format.as_deref()),
        ("{file_size}", context.file_size.as_deref()),
        ("{created_at}", context.created_at.as_deref()),
        ("{modified_at}", context.modified_at.as_deref()),
    ];
    for (placeholder, value) in values {
        rendered = rendered.replace(placeholder, value.unwrap_or(""));
    }
    rendered
}

/// A trimmed value, or `None` when it is empty.
fn clean(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Join the non-empty parts with a comma, dropping duplicates.
fn joined_segments(parts: &[Option<String>]) -> Option<String> {
    let mut kept: Vec<String> = Vec::new();
    for part in parts.iter().flatten() {
        let value = part.trim();
        if value.is_empty() || kept.iter().any(|existing| existing == value) {
            continue;
        }
        kept.push(value.to_string());
    }
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(", "))
    }
}

/// A unix timestamp as local-looking `YYYY-MM-DD HH:MM:SS` (UTC fields, the
/// same convention `taken_at` already used).
fn timestamp_text(seconds: Option<i64>) -> Option<String> {
    seconds
        .and_then(|value| chrono::DateTime::from_timestamp(value, 0))
        .map(|value| value.format("%Y-%m-%d %H:%M:%S").to_string())
}

/// A byte count as a short human readable string.
fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// A duration in seconds as `M:SS` or `H:MM:SS`.
fn duration_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= 3600 {
        format!("{}:{:02}:{:02}", seconds / 3600, (seconds % 3600) / 60, seconds % 60)
    } else if seconds >= 60 {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

/// Build the prompt context of a file from its metadata.
///
/// Tags and people live in other tables; `prompt_context_with_conn` fills
/// them. This builder stays pure so tests can exercise it without a database.
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

    let taken_at = timestamp_text(file.taken_date);

    PromptContext {
        file_name: Some(file.name.clone()),
        exif_summary,
        taken_at,
        camera,
        tags: None,
        people: None,
        location: joined_segments(&[
            clean(file.geo_name.as_deref()),
            clean(file.geo_admin1.as_deref()),
            clean(file.geo_admin2.as_deref()),
            clean(file.geo_cc.as_deref()),
        ]),
        gps: match (file.gps_latitude, file.gps_longitude) {
            (Some(latitude), Some(longitude)) => Some(format!("{latitude:.6}, {longitude:.6}")),
            _ => None,
        },
        comments: clean(file.comments.as_deref()),
        title: clean(file.e_title.as_deref()),
        headline: clean(file.e_headline.as_deref()),
        description: clean(file.e_description.as_deref()),
        artist: clean(file.e_artist.as_deref()),
        copyright: clean(file.e_copyright.as_deref()),
        software: clean(file.e_software.as_deref()),
        camera_make: clean(file.e_make.as_deref()),
        camera_model: clean(file.e_model.as_deref()),
        lens: joined_segments(&[
            clean(file.e_lens_make.as_deref()),
            clean(file.e_lens_model.as_deref()),
        ]),
        exposure: clean(file.e_exposure_time.as_deref()),
        f_number: clean(file.e_f_number.as_deref()),
        focal_length: clean(file.e_focal_length.as_deref()),
        iso: clean(file.e_iso_speed.as_deref()),
        flash: clean(file.e_flash.as_deref()),
        dimensions: match (file.width, file.height) {
            (Some(width), Some(height)) => Some(format!("{width}x{height}")),
            _ => None,
        },
        duration: file.duration.map(duration_text),
        rating: file.rating.map(|rating| rating.to_string()),
        favorite: file
            .is_favorite
            .map(|favorite| if favorite { "true" } else { "false" }.to_string()),
        format: clean(file.format_label.as_deref()),
        file_size: (file.size > 0).then(|| human_size(file.size)),
        created_at: timestamp_text(file.created_at),
        modified_at: timestamp_text(file.modified_at),
    }
}

/// The prompt context including the tags and people of the file.
pub fn prompt_context_with_conn(
    conn: &Connection,
    file: &crate::t_sqlite::AFile,
) -> PromptContext {
    let mut context = prompt_context(file);
    if let Some(file_id) = file.id {
        context.tags = names_text(tag_names(conn, file_id));
        context.people = names_text(people_names(conn, file_id));
    }
    context
}

fn names_text(names: Vec<String>) -> Option<String> {
    if names.is_empty() {
        None
    } else {
        Some(names.join(", "))
    }
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1)",
        params![name],
        |row| row.get::<_, i64>(0),
    )
    .map(|exists| exists == 1)
    .unwrap_or(false)
}

/// The names of the tags of a file, ordered by name.
fn tag_names(conn: &Connection, file_id: i64) -> Vec<String> {
    if !table_exists(conn, "afile_tags") || !table_exists(conn, "atags") {
        return Vec::new();
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT t.name FROM afile_tags ft
         JOIN atags t ON t.id = ft.tag_id
         WHERE ft.file_id = ?1 AND t.name IS NOT NULL
         ORDER BY t.name COLLATE NOCASE",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(params![file_id], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter(|name| !name.trim().is_empty())
        .collect()
}

/// The names of the people a file shows, ordered by name.
fn people_names(conn: &Connection, file_id: i64) -> Vec<String> {
    if !table_exists(conn, "faces") || !table_exists(conn, "persons") {
        return Vec::new();
    }
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT p.name FROM faces f
         JOIN persons p ON p.id = f.person_id
         WHERE f.file_id = ?1 AND p.name IS NOT NULL
         ORDER BY p.name COLLATE NOCASE",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(params![file_id], |row| row.get::<_, String>(0)) else {
        return Vec::new();
    };
    rows.flatten()
        .filter(|name| !name.trim().is_empty())
        .collect()
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
// Cloud LLM rerank (search results)
// ----------------------------------------------------------------------------

/// How many candidates the rerank request carries. The prompt stays small
/// enough for a flash model to answer in a few seconds.
const VL_RERANK_MAX_CANDIDATES: usize = 20;
/// The answer is a short id array; the budget also covers the gateway's
/// hidden thinking tokens, which are billed even with thinking disabled.
const VL_RERANK_MAX_TOKENS: usize = 1000;
const VL_RERANK_TIMEOUT_SECS: u64 = 30;
/// Thumbnails above this size are not sent; the rerank prompt must stay small.
const VL_RERANK_MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
/// Override for the model; by default the model configured for image
/// understanding (settings -> image understanding) is used.
pub const VL_RERANK_MODEL_ENV: &str = "LAP_VL_RERANK_MODEL";

/// One candidate for the vision rerank: its annotation text and, when the
/// thumbnail cache has one, the small image that goes to the model.
pub struct RerankCandidate {
    pub file_id: i64,
    pub text: String,
    /// `(media_type, base64 data)`
    pub image: Option<(String, String)>,
}

/// The multimodal instruction sent to the configured model.
///
/// The model is asked to *filter*, not to permute: returning a subset (or an
/// empty array) is the expected answer. The earlier "rank all candidates"
/// wording made the model echo non-matching photos at the tail.
pub fn build_rerank_content(query: &str, candidates: &[RerankCandidate]) -> Vec<Value> {
    let mut content = vec![json!({
        "type": "text",
        "text": format!(
            "用户查询：{query}\n\
             下面依次给出候选照片的编号，以及能取到缩略图时的画面。请逐张看画面判断：\n\
             1) 核心主体、动作、场景是否与描述相符；\n\
             2) 查询中明确写出的人物性别、人数、颜色、地点等限定必须相符，不能因为“差不多”而保留；\n\
             3) 缺少次要细节（如没拍到道具）不算不符，但明显不是同一类场景、或主体相似而动作/场景不符的，必须排除。\n\
             只输出符合描述的候选编号的 JSON 数组，按相关度从高到低；没有符合的返回 []。不要解释。"
        ),
    })];
    for candidate in candidates {
        match &candidate.image {
            Some((media_type, data)) => {
                content.push(json!({
                    "type": "text",
                    "text": format!("候选 [{}]：", candidate.file_id),
                }));
                content.push(json!({
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": media_type,
                        "data": data,
                    },
                }));
            }
            None => content.push(json!({
                "type": "text",
                "text": format!(
                    "候选 [{}] {}（没有缩略图，按标注判断）",
                    candidate.file_id, candidate.text
                ),
            })),
        }
    }
    content
}

/// Normalize the model answer into the ids it judged relevant.
///
/// Unknown and duplicate ids are dropped; omitted candidates stay out (the
/// model is allowed to reject photos). An empty array is a valid answer that
/// means nothing matched. Returns `None` when the answer is unusable, so the
/// caller can retry instead of silently clearing the result list.
pub fn parse_rerank_selection(text: &str, candidates: &[i64]) -> Option<Vec<i64>> {
    let candidate = strip_code_fence(text).unwrap_or_else(|| text.trim().to_string());
    let value: Value = serde_json::from_str(&candidate).ok()?;
    // The model normally answers a bare array; accept the common wrapped
    // shapes too ("{\"order\": [...]}") instead of failing the whole search.
    let array = value.as_array().or_else(|| {
        value
            .as_object()
            .and_then(|object| {
                ["order", "ids", "result", "results", "ranking"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(Value::as_array))
            })
    })?;
    let mut selection: Vec<i64> = Vec::new();
    let mut saw_item = false;
    for item in array {
        saw_item = true;
        let id = item
            .as_i64()
            .or_else(|| item.as_str().and_then(|raw| raw.trim().parse::<i64>().ok()));
        let Some(id) = id else { continue };
        if candidates.contains(&id) && !selection.contains(&id) {
            selection.push(id);
        }
    }
    // An answer with entries but no usable id is broken, not a rejection.
    if saw_item && selection.is_empty() {
        return None;
    }
    Some(selection)
}

/// A one-line summary of an annotation result for the rerank prompt.
pub fn snippet_from_result_json(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    let mut parts: Vec<String> = Vec::new();
    if let Some(object) = value.as_object() {
        for item in object.values() {
            match item {
                Value::String(part) if !part.trim().is_empty() => parts.push(part.trim().to_string()),
                Value::Array(items) => {
                    for entry in items {
                        if let Value::String(part) = entry && !part.trim().is_empty() {
                            parts.push(part.trim().to_string());
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(truncate_chars(&parts.join(" "), 240))
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max).collect();
    truncated.push('…');
    truncated
}

/// Environment override first, then the model configured for image
/// understanding. This keeps the lab in sync with the settings panel.
fn rerank_model(config: &VlGatewayConfig) -> String {
    std::env::var(VL_RERANK_MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if config.model.trim().is_empty() {
                VL_DEFAULT_MODEL.to_string()
            } else {
                config.model.trim().to_string()
            }
        })
}

/// Read the annotation text and cached thumbnail of every candidate.
fn load_rerank_candidates(ids: &[i64]) -> Result<Vec<RerankCandidate>, String> {
    let conn = crate::t_sqlite::open_conn()?;
    let mut candidates = Vec::with_capacity(ids.len());
    for file_id in ids {
        let annotation = conn
            .query_row(
                "SELECT result_json, raw_text FROM vl_annotations
                 WHERE file_id = ?1 ORDER BY id DESC LIMIT 1",
                params![file_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let mut text = annotation
            .as_ref()
            .and_then(|(result, _)| result.as_deref())
            .and_then(snippet_from_result_json)
            .or_else(|| {
                annotation
                    .as_ref()
                    .and_then(|(_, raw)| raw.as_deref())
                    .map(|raw| truncate_chars(raw.trim(), 240))
                    .filter(|raw| !raw.is_empty())
            });
        if text.is_none() {
            text = conn
                .query_row(
                    "SELECT name FROM afiles WHERE id = ?1",
                    params![file_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
        }
        let image = crate::t_sqlite::AThumb::fetch(*file_id)
            .ok()
            .flatten()
            .and_then(|thumb| {
                if thumb.error_code != 0 {
                    return None;
                }
                let data = thumb.thumb_data?;
                if data.is_empty() || data.len() > VL_RERANK_MAX_IMAGE_BYTES {
                    return None;
                }
                let media_type = crate::t_protocol::detect_image_mime(&data).to_string();
                Some((
                    media_type,
                    base64::engine::general_purpose::STANDARD.encode(&data),
                ))
            });
        candidates.push(RerankCandidate {
            file_id: *file_id,
            text: text.unwrap_or_else(|| "-".to_string()),
            image,
        });
    }
    Ok(candidates)
}

/// Ask the configured model to keep only the candidates that match. Thinking
/// is disabled: the job is a short visual check, not reasoning.
async fn call_rerank_gateway(
    client: &reqwest::Client,
    config: &VlGatewayConfig,
    content: &[Value],
    model: &str,
) -> Result<String, String> {
    let endpoint = format!(
        "{}/v1/messages",
        config.gateway_url.trim().trim_end_matches('/')
    );
    let body = json!({
        "model": model,
        "max_tokens": VL_RERANK_MAX_TOKENS,
        "temperature": 0,
        "thinking": { "type": "disabled" },
        "system": "只输出 JSON 数组，不要解释。",
        "messages": [{ "role": "user", "content": content }],
    });
    let response = client
        .post(&endpoint)
        .header("x-api-key", config.api_key.trim())
        .header("anthropic-version", VL_ANTHROPIC_VERSION)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("重排请求失败: {e}"))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("重排响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(redact(
            &format!("重排网关返回 {status}: {}", truncate_chars(&text, 300)),
            &config.api_key,
        ));
    }
    Ok(parse_response(&text)?.text)
}

/// Keep only the top search candidates that the cloud model judges relevant,
/// in the model's order. Returns a subset of `file_ids` (possibly empty).
#[tauri::command]
pub async fn rerank_search_with_llm(
    query: String,
    file_ids: Vec<i64>,
) -> Result<Vec<i64>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(file_ids);
    }
    let mut ids: Vec<i64> = Vec::new();
    for id in file_ids {
        if id > 0 && !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.truncate(VL_RERANK_MAX_CANDIDATES);
    if ids.is_empty() {
        return Ok(ids);
    }

    let config = resolve_gateway_config()?;
    if config.gateway_url.trim().is_empty() || config.api_key.trim().is_empty() {
        return Err("图片理解网关未配置，无法使用云端重排".to_string());
    }
    let candidates = load_rerank_candidates(&ids)?;
    let content = build_rerank_content(&query, &candidates);
    let model = rerank_model(&config);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(VL_RERANK_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let mut last_answer = String::new();
    for _ in 0..2 {
        last_answer = call_rerank_gateway(&client, &config, &content, &model).await?;
        if let Some(selection) = parse_rerank_selection(&last_answer, &ids) {
            return Ok(selection);
        }
    }
    Err(format!(
        "重排结果无法解析: {}",
        truncate_chars(last_answer.trim(), 160)
    ))
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

    // The body reached us over a 2xx, but it still must never echo the key
    // back into the stored evidence.
    let parsed = parse_response(&raw_body).map_err(|error| VlGatewayError {
        message: error,
        raw_body: Some(redact(&raw_body, &config.api_key)),
    })?;

    if let Some(reason) = parsed.missing_body_reason() {
        return Err(VlGatewayError {
            message: reason,
            raw_body: Some(redact(&raw_body, &config.api_key)),
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
    // A forced run does not drop the old annotation up front: the upsert of a
    // successful call replaces it atomically, so a failed call leaves the
    // previous annotation and its search entry intact.

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
        let Some(file) = file else {
            return Ok(None);
        };
        let conn = crate::t_sqlite::open_conn()?;
        Ok(Some(prompt_context_with_conn(&conn, &file)))
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
            // The annotations reach the search, so the store carries the index.
            conn.execute_batch(crate::t_fts::FTS_SCHEMA_SQL).unwrap();
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
    fn editing_an_old_version_row_picks_the_next_free_version() {
        let store = MemStore::new();
        let first = create_template(&store, "caption", "v1", "s", "{}").unwrap();
        let second = update_template(&store, first.id, "caption", "v2", "s", "{}").unwrap();
        assert_eq!(second.version, 2);

        // Editing the old v1 row must not collide with the v2 row of the name:
        // the edit lands on the next free version instead.
        let third = update_template(&store, first.id, "caption", "v1 again", "s", "{}").unwrap();
        assert_eq!(third.version, 3);
        assert_eq!(third.prompt_template, "v1 again");
    }

    #[test]
    fn a_name_and_version_conflict_reports_a_readable_error() {
        let store = MemStore::new();
        create_template(&store, "caption", "p", "s", "{}").unwrap();
        let conn = store.conn.lock().unwrap();
        // Inserting a taken (name, version) pair again is a conflict the user
        // can read, not a raw SQLite message.
        let error = insert_template_row(&conn, "caption", 1, "p", "s", "{}", false).unwrap_err();
        assert!(
            error.contains("模板名与版本号冲突"),
            "the raw SQLite error must be mapped, got: {error}"
        );
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
            ..PromptContext::default()
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
    fn every_declared_placeholder_is_replaced() {
        let mut context = PromptContext::default();
        context.file_name = Some("v".into());
        context.exif_summary = Some("v".into());
        context.taken_at = Some("v".into());
        context.camera = Some("v".into());
        context.tags = Some("v".into());
        context.people = Some("v".into());
        context.location = Some("v".into());
        context.gps = Some("v".into());
        context.comments = Some("v".into());
        context.title = Some("v".into());
        context.headline = Some("v".into());
        context.description = Some("v".into());
        context.artist = Some("v".into());
        context.copyright = Some("v".into());
        context.software = Some("v".into());
        context.camera_make = Some("v".into());
        context.camera_model = Some("v".into());
        context.lens = Some("v".into());
        context.exposure = Some("v".into());
        context.f_number = Some("v".into());
        context.focal_length = Some("v".into());
        context.iso = Some("v".into());
        context.flash = Some("v".into());
        context.dimensions = Some("v".into());
        context.duration = Some("v".into());
        context.rating = Some("v".into());
        context.favorite = Some("v".into());
        context.format = Some("v".into());
        context.file_size = Some("v".into());
        context.created_at = Some("v".into());
        context.modified_at = Some("v".into());
        let template = "{file_name}{exif_summary}{taken_at}{camera}{tags}{people}\
             {location}{gps}{comments}{title}{headline}{description}{artist}{copyright}\
             {software}{camera_make}{camera_model}{lens}{exposure}{f_number}{focal_length}\
             {iso}{flash}{dimensions}{duration}{rating}{favorite}{format}{file_size}\
             {created_at}{modified_at}";
        let rendered = render_prompt(template, &context);
        assert!(
            !rendered.contains('{'),
            "a declared placeholder was left unreplaced: {rendered}"
        );
        assert_eq!(rendered.chars().filter(|ch| *ch == 'v').count(), 31);
    }

    #[test]
    fn extended_fields_render_their_own_values() {
        let context = PromptContext {
            tags: Some("春节, 旅行".into()),
            people: Some("小明".into()),
            location: Some("杭州西湖, 浙江, CN".into()),
            gps: Some("30.123456, 120.654321".into()),
            dimensions: Some("6000x4000".into()),
            comments: Some("春节团聚".into()),
            ..PromptContext::default()
        };
        assert_eq!(
            render_prompt(
                "[{tags}][{people}][{location}][{gps}][{dimensions}][{comments}]",
                &context
            ),
            "[春节, 旅行][小明][杭州西湖, 浙江, CN][30.123456, 120.654321][6000x4000][春节团聚]"
        );
    }

    #[test]
    fn the_context_carries_the_extended_metadata() {
        let file = crate::t_sqlite::AFile {
            id: Some(1),
            name: "a.jpg".to_string(),
            comments: Some(" 春节 ".to_string()),
            e_title: Some("家庭聚会".to_string()),
            e_headline: Some("春节".to_string()),
            e_description: Some("团聚".to_string()),
            e_artist: Some("Some Artist".to_string()),
            e_copyright: Some("(c)".to_string()),
            e_software: Some("Lap".to_string()),
            e_make: Some("Canon".to_string()),
            e_model: Some("EOS R5".to_string()),
            e_lens_make: Some("Canon".to_string()),
            e_lens_model: Some("RF 24-70".to_string()),
            e_exposure_time: Some("1/125".to_string()),
            e_f_number: Some("f/2.8".to_string()),
            e_focal_length: Some("35mm".to_string()),
            e_iso_speed: Some("800".to_string()),
            e_flash: Some("no".to_string()),
            width: Some(6000),
            height: Some(4000),
            duration: Some(3723),
            rating: Some(5),
            is_favorite: Some(true),
            format_label: Some("JPEG".to_string()),
            size: 1_500_000,
            created_at: Some(1_700_000_000),
            modified_at: Some(1_700_000_100),
            geo_name: Some("杭州西湖".to_string()),
            geo_admin1: Some("浙江".to_string()),
            geo_admin2: Some("西湖区".to_string()),
            geo_cc: Some("CN".to_string()),
            gps_latitude: Some(30.123456),
            gps_longitude: Some(120.654321),
            ..Default::default()
        };
        let context = prompt_context(&file);
        assert_eq!(context.location.as_deref(), Some("杭州西湖, 浙江, 西湖区, CN"));
        assert_eq!(context.lens.as_deref(), Some("Canon, RF 24-70"));
        assert_eq!(context.dimensions.as_deref(), Some("6000x4000"));
        assert_eq!(context.duration.as_deref(), Some("1:02:03"));
        assert_eq!(context.favorite.as_deref(), Some("true"));
        assert_eq!(context.rating.as_deref(), Some("5"));
        assert_eq!(context.file_size.as_deref(), Some("1.4 MB"));
        assert_eq!(context.comments.as_deref(), Some("春节"));
        assert_eq!(context.title.as_deref(), Some("家庭聚会"));
        assert_eq!(context.camera_make.as_deref(), Some("Canon"));
        assert_eq!(context.camera_model.as_deref(), Some("EOS R5"));
        assert_eq!(context.focal_length.as_deref(), Some("35mm"));
        assert_eq!(context.gps.as_deref(), Some("30.123456, 120.654321"));
        assert_eq!(context.format.as_deref(), Some("JPEG"));
        assert!(context.created_at.is_some());
        assert!(context.modified_at.is_some());
        // Relations need a connection; the pure builder leaves them empty.
        assert!(context.tags.is_none());
        assert!(context.people.is_none());
    }

    #[test]
    fn tags_and_people_are_read_from_the_library() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE atags(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE afile_tags(file_id INTEGER, tag_id INTEGER);
             CREATE TABLE persons(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE faces(id INTEGER PRIMARY KEY, file_id INTEGER, person_id INTEGER);
             INSERT INTO atags VALUES(1,'旅行'),(2,'春节');
             INSERT INTO afile_tags VALUES(7,1),(7,2);
             INSERT INTO persons VALUES(1,'小明');
             INSERT INTO faces VALUES(1,7,1);",
        )
        .unwrap();
        let file = crate::t_sqlite::AFile {
            id: Some(7),
            name: "a.jpg".to_string(),
            ..Default::default()
        };
        let context = prompt_context_with_conn(&conn, &file);
        assert_eq!(context.tags.as_deref(), Some("旅行, 春节"));
        assert_eq!(context.people.as_deref(), Some("小明"));

        // A library without the relation tables yields no relations, and a
        // file without an id cannot be looked up at all.
        let empty = Connection::open_in_memory().unwrap();
        let context = prompt_context_with_conn(&empty, &file);
        assert!(context.tags.is_none());
        assert!(context.people.is_none());
        let without_id = crate::t_sqlite::AFile {
            name: "b.jpg".to_string(),
            ..Default::default()
        };
        let context = prompt_context_with_conn(&empty, &without_id);
        assert!(context.tags.is_none());
        assert!(context.people.is_none());
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

    // ----- LLM rerank -----

    fn rerank_candidate(file_id: i64, text: &str, image: bool) -> RerankCandidate {
        RerankCandidate {
            file_id,
            text: text.to_string(),
            image: image.then(|| ("image/jpeg".to_string(), "aGVsbG8=".to_string())),
        }
    }

    #[test]
    fn the_rerank_prompt_asks_for_matches_not_a_full_permutation() {
        let content = build_rerank_content(
            "摘葡萄",
            &[
                rerank_candidate(7, "一个男孩在葡萄园里采摘葡萄", true),
                rerank_candidate(9, "城市夜景", false),
            ],
        );
        let intro = content[0]["text"].as_str().expect("intro text block");
        assert!(intro.contains("摘葡萄"));
        assert!(intro.contains("JSON"));
        // The model may drop candidates: "no match" is a valid answer.
        assert!(intro.contains("没有符合"));
        assert!(!intro.contains("不要遗漏"));
    }

    #[test]
    fn the_rerank_content_carries_one_image_block_per_candidate_with_a_thumb() {
        let content = build_rerank_content(
            "摘葡萄",
            &[
                rerank_candidate(7, "一个男孩在葡萄园里采摘葡萄", true),
                rerank_candidate(9, "城市夜景", false),
            ],
        );
        let images: Vec<&Value> = content
            .iter()
            .filter(|block| block["type"] == "image")
            .collect();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0]["source"]["media_type"], json!("image/jpeg"));
        assert_eq!(images[0]["source"]["data"], json!("aGVsbG8="));
        // A candidate without a thumbnail keeps its annotation text.
        let texts: Vec<String> = content
            .iter()
            .filter_map(|block| block["text"].as_str().map(str::to_string))
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text.contains("[9]") && text.contains("城市夜景"))
        );
    }

    #[test]
    fn a_plain_id_array_is_the_rerank_selection() {
        assert_eq!(parse_rerank_selection("[9, 7]", &[7, 9]), Some(vec![9, 7]));
    }

    #[test]
    fn a_fenced_id_array_is_accepted() {
        assert_eq!(
            parse_rerank_selection("```json\n[9, 7]\n```", &[7, 9]),
            Some(vec![9, 7])
        );
    }

    #[test]
    fn a_wrapped_id_array_is_accepted() {
        assert_eq!(
            parse_rerank_selection("{\"order\": [9, 7]}", &[7, 9]),
            Some(vec![9, 7])
        );
    }

    #[test]
    fn an_empty_answer_means_nothing_matches() {
        assert_eq!(parse_rerank_selection("[]", &[7, 9]), Some(Vec::new()));
        assert_eq!(
            parse_rerank_selection("{\"ids\": []}", &[7, 9]),
            Some(Vec::new())
        );
    }

    #[test]
    fn unknown_and_duplicate_ids_are_dropped_without_backfill() {
        // 42 is not a candidate, 9 repeats, 7 was not selected -> 7 stays out.
        assert_eq!(parse_rerank_selection("[42, 9, 9]", &[7, 9]), Some(vec![9]));
        assert_eq!(parse_rerank_selection("[7]", &[7, 9]), Some(vec![7]));
    }

    #[test]
    fn an_unreadable_rerank_answer_is_rejected() {
        assert_eq!(parse_rerank_selection("不是 JSON", &[7, 9]), None);
        assert_eq!(parse_rerank_selection("{\"foo\": 1}", &[7, 9]), None);
        // An answer made up entirely of unknown ids is a broken answer, not
        // a deliberate "nothing matches".
        assert_eq!(parse_rerank_selection("[1]", &[7, 9]), None);
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

    #[tokio::test]
    async fn a_failing_2xx_body_is_redacted_in_the_stored_evidence() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        let client = reqwest::Client::new();

        // 200 with a body that is not JSON at all.
        let gateway = mock::spawn(
            "200 OK",
            "<html>bad json echoing test-key-not-a-real-secret</html>",
        )
        .await;
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
        assert!(matches!(outcome, VlFileOutcome::Failed { .. }));
        let calls = store.calls(1).unwrap();
        assert_eq!(calls[0].status, "error");
        let raw = calls[0].raw_response.clone().unwrap();
        assert!(
            !raw.contains("test-key-not-a-real-secret"),
            "the stored evidence must not carry the key, got: {raw}"
        );
        assert!(raw.contains("***"));

        // 200 with JSON but no text block: the same rule holds.
        let gateway = mock::spawn(
            "200 OK",
            r#"{"content":[],"note":"key test-key-not-a-real-secret leaked"}"#,
        )
        .await;
        let outcome = analyze_file(
            &store,
            &StubSource,
            &client,
            &test_config(&gateway.url),
            2,
            &template,
            false,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, VlFileOutcome::Failed { .. }));
        let calls = store.calls(2).unwrap();
        assert_eq!(calls[0].status, "error");
        let raw = calls[0].raw_response.clone().unwrap();
        assert!(
            !raw.contains("test-key-not-a-real-secret"),
            "the stored evidence must not carry the key, got: {raw}"
        );
        assert!(raw.contains("***"));
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
                ..PromptContext::default()
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
    async fn a_failed_force_rerun_keeps_the_previous_annotation() {
        let store = MemStore::new();
        let template = analysis_template(&store);
        seed_annotation(&store, &template, 1);

        let gateway = mock::spawn(
            "500 Internal Server Error",
            r#"{"error":{"message":"gateway is down"}}"#,
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

        assert!(matches!(outcome, VlFileOutcome::Failed { .. }));
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);

        // The previous annotation survives the failed forced run.
        let annotation = store
            .annotation(1, template.id, template.version)
            .unwrap()
            .unwrap();
        assert_eq!(annotation.result_json, "{\"caption\":\"old\"}");
        assert_eq!(annotation.raw_text, "old");

        // The failure leaves evidence behind, on top of the seeded call.
        let calls = store.calls(1).unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].status, "error");

        // The old caption is still the one the search finds.
        {
            let conn = store.conn.lock().unwrap();
            let hits = crate::t_fts::search(&conn, "old", 100).unwrap();
            assert_eq!(hits.len(), 1, "the old caption must stay searchable");
        }
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
    const VL_COMMANDS: [&str; 11] = [
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
        "rerank_search_with_llm",
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
