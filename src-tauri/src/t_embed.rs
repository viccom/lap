//! Caption embeddings: the semantic-search index over image descriptions.
//!
//! The image understanding flow stores a text description per photo. The
//! semantic search compares a query with those descriptions through an
//! embedding model, so both sides have to live in the same text-vector space.
//! Two sources are supported:
//!
//! * `local` — a BGE-family ONNX model under `<app data>/models/bge-local`,
//!   falling back to a copy bundled in the `models` directory beside the
//!   binary (no network; bge-small-zh-v1.5 measured R@1 0.945 on the eval set);
//! * `cloud` — the configured VL gateway `/v1/embeddings` (comparison source).
//!
//! Embeddings are derived data: they are refreshed next to the FTS caption
//! hooks, can always be rebuilt, and a failure never blocks an annotation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use ort::{inputs, value::Value};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Value as JsonValue, json};

/// Cosine similarity cut-off below which a caption no longer counts as a hit.
/// Measured on the 113-query eval set (SEARCH-RESEARCH §4.5): 0.45 keeps every
/// target photo while removing the bulk of partial-match noise.
pub const CAPTION_HIT_THRESHOLD: f32 = 0.45;
/// The gateway embeddings API accepts at most this many inputs per call.
const CLOUD_BATCH: usize = 16;
/// Safe upper bound for BGE token inputs; captions and queries are far below.
const LOCAL_MAX_TOKENS: usize = 512;
/// Files inside `<app data>/models/bge-local` that make the local source
/// usable: `model.onnx` (BERT/BGE export, CLS pooling) and `tokenizer.json`.
const LOCAL_EMBED_MODEL_FILE: &str = "model.onnx";

pub(crate) const EMBED_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS caption_embeds (
    file_id INTEGER PRIMARY KEY,
    model TEXT NOT NULL,
    dims INTEGER NOT NULL,
    embeds BLOB NOT NULL,
    updated_at INTEGER NOT NULL
);";

pub const VL_EMBED_SOURCE_ENV: &str = "LAP_VL_EMBED_SOURCE";
pub const VL_EMBED_MODEL_ENV: &str = "LAP_VL_EMBED_MODEL";
/// Local by default: the BGE model under models/bge-local keeps the semantic
/// search offline; the cloud source stays available for comparison.
pub const VL_EMBED_DEFAULT_SOURCE: &str = "local";
pub const VL_EMBED_DEFAULT_CLOUD_MODEL: &str = "zhipu/embedding-3";
/// The name stored with locally produced vectors (the model directory).
pub const LOCAL_EMBED_MODEL_NAME: &str = "bge-local";

/// A semantic hit: the file and how close its description is to the query.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptionHit {
    pub file_id: i64,
    pub score: f32,
}

/// The embedding source selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedSource {
    Local,
    Cloud,
}

impl EmbedSource {
    pub fn from_config(config: &crate::t_vl::VlGatewayConfig) -> Self {
        let value = std::env::var(VL_EMBED_SOURCE_ENV)
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| {
                if config.embed_source.trim().is_empty() {
                    VL_EMBED_DEFAULT_SOURCE.to_string()
                } else {
                    config.embed_source.trim().to_string()
                }
            });
        if value.eq_ignore_ascii_case("cloud") {
            EmbedSource::Cloud
        } else {
            EmbedSource::Local
        }
    }
}

/// The cloud embedding model, independent of the vision and rerank models.
pub fn cloud_embed_model(config: &crate::t_vl::VlGatewayConfig) -> String {
    std::env::var(VL_EMBED_MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            if config.embed_model.trim().is_empty() {
                VL_EMBED_DEFAULT_CLOUD_MODEL.to_string()
            } else {
                config.embed_model.trim().to_string()
            }
        })
}

/// Anything that can turn texts into vectors of a shared space.
pub trait Embedder {
    /// Identifies the vector space; stored per row so a model switch cannot
    /// silently mix incompatible vectors.
    fn name(&self) -> String;
    fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

// ----------------------------------------------------------------------------
// Storage
// ----------------------------------------------------------------------------

pub fn upsert_row(
    conn: &Connection,
    file_id: i64,
    model: &str,
    dims: usize,
    embeds: &[f32],
) -> Result<(), String> {
    let mut blob = Vec::with_capacity(embeds.len() * 4);
    for value in embeds {
        blob.extend_from_slice(&value.to_le_bytes());
    }
    conn.execute(
        "INSERT INTO caption_embeds(file_id, model, dims, embeds, updated_at)
         VALUES(?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(file_id) DO UPDATE SET model = excluded.model, dims = excluded.dims,
                       embeds = excluded.embeds, updated_at = excluded.updated_at",
        params![
            file_id,
            model,
            dims as i64,
            blob,
            chrono::Utc::now().timestamp()
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn remove_row(conn: &Connection, file_id: i64) -> Result<(), String> {
    conn.execute(
        "DELETE FROM caption_embeds WHERE file_id = ?1",
        params![file_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Every stored row of one model: `(file_id, dims, embeds)`.
pub fn rows_of_model(conn: &Connection, model: &str) -> Result<Vec<(i64, usize, Vec<u8>)>, String> {
    let mut stmt = conn
        .prepare("SELECT file_id, dims, embeds FROM caption_embeds WHERE model = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![model], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// The description texts of a file, exactly what the FTS caption column holds.
pub fn caption_texts(conn: &Connection, file_id: i64) -> Result<Vec<String>, String> {
    crate::t_fts::caption_of(conn, file_id)
}

/// Refresh one file after its captions changed. No caption means the row has
/// to go. Failures are the caller's to log: embedding is derived data.
pub fn refresh_file(
    conn: &Connection,
    file_id: i64,
    embedder: &mut dyn Embedder,
) -> Result<bool, String> {
    let text = caption_texts(conn, file_id)?.join(" ");
    if text.trim().is_empty() {
        remove_row(conn, file_id)?;
        return Ok(false);
    }
    let vectors = embedder.embed_batch(&[text])?;
    let Some(vector) = vectors.first() else {
        return Err("the embedder returned no vector".to_string());
    };
    upsert_row(conn, file_id, &embedder.name(), vector.len(), vector)?;
    Ok(true)
}

/// Best-effort hook refresh used next to the FTS caption hooks: local source
/// only, so a synchronous hook never waits on the network.
pub fn refresh_file_hook(conn: &Connection, file_id: i64) {
    let Ok(config) = crate::t_vl::resolve_gateway_config() else {
        return;
    };
    if EmbedSource::from_config(&config) != EmbedSource::Local || !local_model_available() {
        return;
    }
    let embedder = local_embedder();
    if let Err(error) = embedder.as_ref().map_err(Clone::clone).and_then(|model| {
        let mut guard = model.lock().map_err(|e| e.to_string())?;
        refresh_file(conn, file_id, &mut *guard)
    }) {
        eprintln!("caption embed refresh failed for file {file_id}: {error}");
    }
}

/// Files that carry an annotation but no vector of `model` yet.
fn files_missing_embedding(conn: &Connection, model: &str) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT v.file_id FROM vl_annotations v
             WHERE NOT EXISTS (
                SELECT 1 FROM caption_embeds c
                WHERE c.file_id = v.file_id AND c.model = ?1
             )",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![model], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

/// Embed every description that has no vector of the active model yet.
/// Returns `(stored, failed)`; a batch failure never aborts the rest.
pub fn ensure_fresh() -> Result<(usize, usize), String> {
    let model = active_model_name()?;
    let mut embedder = make_embedder()?;
    let missing: Vec<(i64, String)> = {
        let conn = crate::t_sqlite::open_conn()?;
        files_missing_embedding(&conn, &model)?
            .into_iter()
            .filter_map(|file_id| {
                caption_texts(&conn, file_id)
                    .ok()
                    .map(|texts| (file_id, texts.join(" ")))
            })
            .filter(|(_, text)| !text.trim().is_empty())
            .collect()
    };
    let mut ok = 0usize;
    let mut failed = 0usize;
    for chunk in missing.chunks(CLOUD_BATCH) {
        let texts: Vec<String> = chunk.iter().map(|(_, text)| text.clone()).collect();
        match embedder.embed_batch(&texts) {
            Ok(vectors) => {
                let conn = crate::t_sqlite::open_conn()?;
                for ((file_id, _), vector) in chunk.iter().zip(vectors.iter()) {
                    match upsert_row(&conn, *file_id, &model, vector.len(), vector) {
                        Ok(()) => ok += 1,
                        Err(error) => {
                            failed += 1;
                            eprintln!("caption embed store failed for file {file_id}: {error}");
                        }
                    }
                }
            }
            Err(error) => {
                failed += chunk.len();
                eprintln!("caption embed batch failed: {error}");
            }
        }
    }
    Ok((ok, failed))
}

// ----------------------------------------------------------------------------
// Ranking
// ----------------------------------------------------------------------------

/// Cosine similarity; zero for empty or length-mismatched vectors.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

fn vector_from_blob(blob: &[u8]) -> Vec<f32> {
    blob.chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Rank stored caption vectors against a query vector.
/// Rows of another model or with another dimensionality are skipped — mixing
/// vector spaces would produce meaningless scores.
pub fn rank_rows(
    query: &[f32],
    rows: &[(i64, usize, Vec<u8>)],
    threshold: f32,
    limit: usize,
) -> Vec<CaptionHit> {
    let mut hits: Vec<CaptionHit> = rows
        .iter()
        .filter(|(_, dims, blob)| *dims == query.len() && blob.len() / 4 == query.len())
        .map(|(file_id, _, blob)| CaptionHit {
            file_id: *file_id,
            score: cosine(query, &vector_from_blob(blob)),
        })
        .filter(|hit| hit.score > threshold)
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    hits
}

// ----------------------------------------------------------------------------
// Local BGE model
// ----------------------------------------------------------------------------

fn local_model_files_present(dir: &Path) -> bool {
    dir.join(LOCAL_EMBED_MODEL_FILE).is_file() && dir.join(crate::t_common::AI_TOKENIZER).is_file()
}

/// Where the local embedding model is read from: the app-data copy wins, the
/// bundled resource copy (a `models` directory beside the binary, e.g. a
/// self-contained portable layout) is the fallback. Both roots are the
/// `models` directories; `None` when no complete copy exists.
fn resolve_local_model_dir(
    appdata_models: Option<&Path>,
    resource_models: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = appdata_models
        .map(|root| root.join(LOCAL_EMBED_MODEL_NAME))
        .filter(|dir| local_model_files_present(dir))
    {
        return Some(dir);
    }
    resource_models
        .map(|root| root.join(LOCAL_EMBED_MODEL_NAME))
        .filter(|dir| local_model_files_present(dir))
}

/// The active copy of the model, resolved from app-data and the resource dir.
fn resolved_local_model_dir() -> Option<PathBuf> {
    resolve_local_model_dir(
        crate::t_config::get_app_data_dir()
            .ok()
            .map(|dir| dir.join("models"))
            .as_deref(),
        crate::t_config::get_resource_models_dir().as_deref(),
    )
}

/// The directory a missing model is reported against: the app-data install
/// target, where a download or manual copy is expected to land.
fn active_local_model_dir() -> Result<PathBuf, String> {
    resolved_local_model_dir().ok_or_else(|| {
        "本地嵌入模型未安装：请在 设置→图片理解 把嵌入源切到云端，或把 BGE 模型\
         （model.onnx + tokenizer.json）放入 用户数据目录或程序目录下的 models/bge-local/"
            .to_string()
    })
}

/// The identity of the installed local model: its file size. The directory
/// accepts any BGE-family ONNX, so the size tells the stored vectors apart —
/// swapping the model file changes the fingerprint and forces a re-embed
/// instead of silently mixing incompatible vector spaces.
fn local_model_fingerprint() -> Result<u64, String> {
    let path = active_local_model_dir()?.join(LOCAL_EMBED_MODEL_FILE);
    let meta = std::fs::metadata(&path).map_err(|e| format!("{path:?}: {e}"))?;
    Ok(meta.len())
}

/// The name stored with vectors of the installed local model.
fn local_model_vector_name() -> Result<String, String> {
    Ok(format!(
        "{}-{}",
        LOCAL_EMBED_MODEL_NAME,
        local_model_fingerprint()?
    ))
}

pub fn local_model_available() -> bool {
    resolved_local_model_dir().is_some()
}

struct LocalModel {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    vector_name: String,
}

static LOCAL_MODEL: OnceLock<Mutex<Option<Arc<Mutex<LocalModel>>>>> = OnceLock::new();

/// The cached local model, loading it on first use.
fn local_embedder() -> Result<Arc<Mutex<LocalModel>>, String> {
    let cell = LOCAL_MODEL.get_or_init(|| Mutex::new(None));
    {
        let guard = cell.lock().map_err(|e| e.to_string())?;
        if let Some(model) = guard.as_ref() {
            return Ok(model.clone());
        }
    }
    let dir = active_local_model_dir()?;
    let tokenizer = tokenizers::Tokenizer::from_file(dir.join(crate::t_common::AI_TOKENIZER))
        .map_err(|e| format!("Failed to load the embedding tokenizer: {e}"))?;
    let session = ort::session::Session::builder()
        .and_then(|builder| builder.commit_from_file(dir.join(LOCAL_EMBED_MODEL_FILE)))
        .map_err(|e| format!("Failed to load the embedding model: {e}"))?;
    let vector_name = local_model_vector_name()?;
    let model = Arc::new(Mutex::new(LocalModel {
        session,
        tokenizer,
        vector_name,
    }));
    {
        let mut guard = cell.lock().map_err(|e| e.to_string())?;
        *guard = Some(model.clone());
    }
    Ok(model)
}

impl LocalModel {
    fn encode(&mut self, text: &str) -> Result<Vec<f32>, String> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| format!("Tokenization error: {e}"))?;
        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|v| *v as i64).collect();
        let mut mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|v| *v as i64)
            .collect();
        ids.truncate(LOCAL_MAX_TOKENS);
        mask.truncate(ids.len());
        let seq = mask.len();

        // BERT-family exports may also require token_type_ids; feeds are
        // assembled from what the model actually declares.
        let has_mask = self
            .session
            .inputs
            .iter()
            .any(|input| input.name == "attention_mask");
        let has_token_types = self
            .session
            .inputs
            .iter()
            .any(|input| input.name == "token_type_ids");
        let ids_value = Value::from_array(
            ndarray::Array2::from_shape_vec((1, ids.len()), ids).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mask_value = Value::from_array(
            ndarray::Array2::from_shape_vec((1, mask.len()), mask).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let token_types_value = Value::from_array(
            ndarray::Array2::from_shape_vec((1, seq), vec![0i64; seq])
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

        let outputs = match (has_mask, has_token_types) {
            (true, true) => self.session.run(inputs![
                "input_ids" => ids_value,
                "attention_mask" => mask_value,
                "token_type_ids" => token_types_value,
            ]),
            (true, false) => self.session.run(inputs![
                "input_ids" => ids_value,
                "attention_mask" => mask_value,
            ]),
            _ => self.session.run(inputs![
                "input_ids" => ids_value,
            ]),
        }
        .map_err(|e| e.to_string())?;
        // BGE exports `last_hidden_state` [1, seq, hidden]; the sentence
        // embedding is the CLS token, L2-normalized.
        let embedding = if let Some(values) = outputs.get("last_hidden_state") {
            values
        } else {
            &outputs[0]
        };
        let (shape, data) = embedding
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let hidden = shape[shape.len() - 1] as usize;
        let head: Vec<f32> = data[..hidden.min(data.len())].to_vec();
        let norm = head.iter().map(|v| v * v).sum::<f32>().sqrt();
        Ok(if norm > 0.0 {
            head.iter().map(|v| v / norm).collect()
        } else {
            head
        })
    }
}

impl Embedder for LocalModel {
    fn name(&self) -> String {
        self.vector_name.clone()
    }

    fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        texts.iter().map(|text| self.encode(text)).collect()
    }
}

// ----------------------------------------------------------------------------
// Cloud gateway
// ----------------------------------------------------------------------------

/// Parse a `/v1/embeddings` response body into vectors.
pub fn parse_embeddings_response(body: &str) -> Result<Vec<Vec<f32>>, String> {
    let value: JsonValue = serde_json::from_str(body).map_err(|e| format!("invalid JSON: {e}"))?;
    let Some(data) = value.get("data").and_then(|d| d.as_array()) else {
        return Err("the response has no data array".to_string());
    };
    let mut vectors = Vec::with_capacity(data.len());
    for item in data {
        let embedding = item
            .get("embedding")
            .and_then(|e| e.as_array())
            .ok_or_else(|| "an item has no embedding array".to_string())?;
        let vector = embedding
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0) as f32)
            .collect::<Vec<f32>>();
        if vector.is_empty() {
            return Err("an embedding is empty".to_string());
        }
        vectors.push(vector);
    }
    if vectors.is_empty() {
        return Err("the response has no embeddings".to_string());
    }
    Ok(vectors)
}

fn call_cloud_embeddings(
    client: &reqwest::blocking::Client,
    config: &crate::t_vl::VlGatewayConfig,
    model: &str,
    texts: &[String],
) -> Result<Vec<Vec<f32>>, String> {
    let endpoint = crate::t_vl_protocol::join_endpoint(&config.gateway_url, "/v1/embeddings")?;
    let body = json!({ "model": model, "input": texts });
    let mut request = client.post(endpoint);
    // The auth follows the configured protocol: Anthropic gateways take the
    // key header, OpenAI gateways take Bearer (and only when a key is set).
    for (name, value) in crate::t_vl_protocol::auth_headers(config.protocol, &config.api_key) {
        request = request.header(name, value);
    }
    let response = request
        .json(&body)
        .send()
        .map_err(|e| format!("embedding request failed: {e}"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|e| format!("embedding response read failed: {e}"))?;
    if !status.is_success() {
        let excerpt: String = text.chars().take(160).collect();
        return Err(format!("embedding gateway returned {status}: {excerpt}"));
    }
    parse_embeddings_response(&text)
}

// ----------------------------------------------------------------------------
// Source resolution
// ----------------------------------------------------------------------------

enum ActiveEmbedder {
    Local(Arc<Mutex<LocalModel>>),
    Cloud {
        config: crate::t_vl::VlGatewayConfig,
        model: String,
    },
}

impl Embedder for ActiveEmbedder {
    fn name(&self) -> String {
        match self {
            ActiveEmbedder::Local(_) => LOCAL_EMBED_MODEL_NAME.to_string(),
            ActiveEmbedder::Cloud { model, .. } => model.clone(),
        }
    }

    fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        match self {
            ActiveEmbedder::Local(model) => {
                let mut guard = model.lock().map_err(|e| e.to_string())?;
                texts.iter().map(|text| guard.encode(text)).collect()
            }
            ActiveEmbedder::Cloud { config, model } => {
                // The blocking client owns a tokio runtime that must never be
                // created or dropped inside an async context: a dedicated OS
                // thread keeps the command handlers safe either way (a sync
                // command may run on a tokio worker).
                let texts: Vec<String> = texts.to_vec();
                let config = config.clone();
                let model = model.clone();
                std::thread::spawn(move || {
                    let client = reqwest::blocking::Client::new();
                    let mut vectors = Vec::with_capacity(texts.len());
                    for chunk in texts.chunks(CLOUD_BATCH) {
                        let mut part = call_cloud_embeddings(&client, &config, &model, chunk)?;
                        vectors.append(&mut part);
                    }
                    Ok(vectors)
                })
                .join()
                .map_err(|_| "the embedding request thread panicked".to_string())?
            }
        }
    }
}

/// Build the embedder for the configured source.
pub fn make_embedder() -> Result<Box<dyn Embedder>, String> {
    let config = crate::t_vl::resolve_gateway_config()?;
    match EmbedSource::from_config(&config) {
        EmbedSource::Local => {
            if !local_model_available() {
                return Err(active_local_model_dir().unwrap_err());
            }
            let model = local_embedder()?;
            Ok(Box::new(ActiveEmbedder::Local(model)))
        }
        EmbedSource::Cloud => {
            let model = cloud_embed_model(&config);
            if config.gateway_url.trim().is_empty() {
                return Err("the embedding gateway URL is not configured".to_string());
            }
            Ok(Box::new(ActiveEmbedder::Cloud { config, model }))
        }
    }
}

/// The model name the configured source would produce.
pub fn active_model_name() -> Result<String, String> {
    let config = crate::t_vl::resolve_gateway_config()?;
    Ok(match EmbedSource::from_config(&config) {
        EmbedSource::Local => local_model_vector_name()?,
        EmbedSource::Cloud => cloud_embed_model(&config),
    })
}

// ----------------------------------------------------------------------------
// Commands
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbedStatusView {
    pub source: String,
    pub local_available: bool,
    pub model: String,
    pub embedded: i64,
    pub captions: i64,
}

#[tauri::command]
pub fn embed_status() -> Result<EmbedStatusView, String> {
    let conn = crate::t_sqlite::open_conn()?;
    let model = active_model_name()?;
    let embedded: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM caption_embeds WHERE model = ?1",
            params![model],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let captions: i64 = conn
        .query_row(
            "SELECT COUNT(DISTINCT file_id) FROM vl_annotations",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    let source = crate::t_vl::resolve_gateway_config()
        .ok()
        .map(|config| match EmbedSource::from_config(&config) {
            EmbedSource::Local => "local".to_string(),
            EmbedSource::Cloud => "cloud".to_string(),
        })
        .unwrap_or_else(|| VL_EMBED_DEFAULT_SOURCE.to_string());
    Ok(EmbedStatusView {
        source,
        local_available: local_model_available(),
        model,
        embedded,
        captions,
    })
}

/// Rebuild every caption embedding for the active model from scratch.
#[tauri::command]
pub fn rebuild_caption_embeds() -> Result<(usize, usize), String> {
    let model = active_model_name()?;
    {
        let conn = crate::t_sqlite::open_conn()?;
        conn.execute(
            "DELETE FROM caption_embeds WHERE model = ?1",
            params![model],
        )
        .map_err(|e| e.to_string())?;
    }
    ensure_fresh()
}

/// Semantic search: query embedding vs stored caption embeddings.
#[tauri::command]
pub fn search_captions(
    query: String,
    threshold: Option<f32>,
    limit: Option<i64>,
) -> Result<Vec<CaptionHit>, String> {
    let query = query.trim().to_string();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let threshold = threshold
        .map(|value| value.clamp(0.0, 1.0))
        .unwrap_or(CAPTION_HIT_THRESHOLD);
    let limit = limit
        .unwrap_or(crate::t_fts::FTS_DEFAULT_LIMIT)
        .clamp(1, 1000) as usize;

    ensure_fresh()?;
    let mut embedder = make_embedder()?;
    let query_vector = embedder
        .embed_batch(&[query])?
        .into_iter()
        .next()
        .ok_or_else(|| "the embedder returned no query vector".to_string())?;

    let conn = crate::t_sqlite::open_conn()?;
    let model = active_model_name()?;
    let rows = rows_of_model(&conn, &model)?;
    // The search-exclusion rule applies before the ranking cut: a hidden
    // file must not spend a Top-N slot.
    let rows = filter_excluded_rows(&conn, rows);
    Ok(rank_rows(&query_vector, &rows, threshold, limit))
}

/// Drop the rows whose file sits in a folder excluded from search.
///
/// Schemas that cannot answer (no afolders, partial fixtures) keep every
/// row: the filter degrades to a no-op instead of failing the search.
pub fn filter_excluded_rows(
    conn: &Connection,
    rows: Vec<(i64, usize, Vec<u8>)>,
) -> Vec<(i64, usize, Vec<u8>)> {
    let Ok(excluded) = excluded_file_ids(conn) else {
        return rows;
    };
    if excluded.is_empty() {
        return rows;
    }
    rows.into_iter()
        .filter(|(file_id, _, _)| !excluded.contains(file_id))
        .collect()
}

/// The file ids whose folder is excluded from search.
fn excluded_file_ids(conn: &Connection) -> Result<std::collections::HashSet<i64>, String> {
    let sql = format!(
        "SELECT a.id FROM afiles a JOIN afolders b ON b.id = a.folder_id
         WHERE NOT ({})",
        crate::t_sqlite::AFile::search_exclusion_condition("b")
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    Ok(rows.flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t_fts::FTS_SCHEMA_SQL;
    use crate::t_migration::VL_SCHEMA_SQL;

    struct FakeEmbedder {
        model: String,
    }

    impl Embedder for FakeEmbedder {
        fn name(&self) -> String {
            self.model.clone()
        }

        fn embed_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
            Ok(texts
                .iter()
                .map(|text| {
                    let mut vector = vec![0.0f32; 4];
                    for (index, ch) in text.chars().enumerate() {
                        vector[index % 4] += (ch as u32 & 0xF) as f32;
                    }
                    vector
                })
                .collect())
        }
    }

    pub(crate) fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(VL_SCHEMA_SQL).unwrap();
        conn.execute_batch(FTS_SCHEMA_SQL).unwrap();
        conn.execute_batch(EMBED_SCHEMA_SQL).unwrap();
        // The foreign keys of vl_annotations point at these parents.
        conn.execute_batch("CREATE TABLE IF NOT EXISTS afiles (id INTEGER PRIMARY KEY);")
            .unwrap();
        conn
    }

    fn seed_annotation(conn: &Connection, file_id: i64, text: &str) {
        conn.execute(
            "INSERT OR IGNORE INTO afiles(id) VALUES(?1)",
            params![file_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO vl_calls(id, file_id, template_id, template_version, prompt_actual, system_actual, status, created_at)
             VALUES(1, ?1, 1, 1, 'p', 's', 'ok', 0)",
            params![file_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO vl_annotations(file_id, template_id, template_version, model, result_json, raw_text, call_id, created_at)
             VALUES(?1, 1, 1, 'm', ?2, ?2, 1, 0)",
            params![file_id, format!("{{\"caption\":\"{text}\"}}")],
        )
        .unwrap();
    }

    #[test]
    fn cosine_orders_by_direction_and_ignores_length() {
        let a = [1.0, 0.0];
        assert!((cosine(&a, &[2.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&a, &[0.0, 5.0]).abs() < 1e-6);
        assert!(cosine(&a, &[-1.0, 0.0]) < 0.0);
        assert!(cosine(&a, &[0.0, 0.0]) == 0.0);
        assert!(cosine(&a, &[1.0]) == 0.0, "length mismatch must be 0");
    }

    #[test]
    fn rank_rows_filters_by_threshold_and_skips_foreign_shapes() {
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let blob = |vector: &[f32]| {
            vector
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<u8>>()
        };
        let rows = vec![
            (1, 4, blob(&[1.0, 0.0, 0.0, 0.0])), // perfect match
            (2, 4, blob(&[0.0, 1.0, 0.0, 0.0])), // orthogonal, filtered
            (3, 4, blob(&[0.9, 0.1, 0.0, 0.0])), // close, kept
            (4, 2, blob(&[1.0, 0.0])),           // wrong dims, skipped
        ];
        let hits = rank_rows(&query, &rows, 0.5, 10);
        assert_eq!(
            hits.iter().map(|h| h.file_id).collect::<Vec<_>>(),
            vec![1, 3]
        );
        let limited = rank_rows(&query, &rows, 0.0, 1);
        assert_eq!(limited.len(), 1);
        assert_eq!(limited[0].file_id, 1);
    }

    #[test]
    fn refresh_file_stores_then_removes_with_the_caption() {
        let conn = fixture();
        seed_annotation(&conn, 7, "雪地里的红衣男孩");
        let mut embedder = FakeEmbedder {
            model: "fake".into(),
        };
        assert!(refresh_file(&conn, 7, &mut embedder).unwrap());
        let rows = rows_of_model(&conn, "fake").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 7);

        conn.execute("DELETE FROM vl_annotations WHERE file_id = 7", [])
            .unwrap();
        assert!(!refresh_file(&conn, 7, &mut embedder).unwrap());
        assert!(rows_of_model(&conn, "fake").unwrap().is_empty());
    }

    #[test]
    fn upsert_row_replaces_the_previous_vector() {
        let conn = fixture();
        upsert_row(&conn, 1, "m", 2, &[1.0, 0.0]).unwrap();
        upsert_row(&conn, 1, "other", 2, &[0.0, 1.0]).unwrap();
        let rows = rows_of_model(&conn, "other").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 1);
        assert!(rows_of_model(&conn, "m").unwrap().is_empty());
    }

    #[test]
    fn parse_embeddings_response_accepts_the_gateway_shape() {
        let body = r#"{"data":[{"embedding":[0.1,0.2]},{"embedding":[0.3]}],"model":"x"}"#;
        let vectors = parse_embeddings_response(body).unwrap();
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[1], vec![0.3]);

        assert!(parse_embeddings_response("{}").is_err());
        assert!(parse_embeddings_response(r#"{"data":[]}"#).is_err());
        assert!(parse_embeddings_response(r#"{"data":[{"nope":1}]}"#).is_err());
    }

    #[test]
    fn the_default_threshold_matches_the_measured_separation() {
        // Measured on the eval set: every target scored 0.486+, so 0.45 keeps
        // all of them while cutting most partial matches.
        assert!(CAPTION_HIT_THRESHOLD >= 0.44 && CAPTION_HIT_THRESHOLD < 0.486);
    }
}

#[cfg(test)]
mod search_exclusion_tests {
    use super::tests::fixture;
    use super::*;

    #[test]
    fn rows_of_excluded_folders_are_dropped_before_ranking() {
        let conn = fixture();
        conn.execute_batch(
            "CREATE TABLE albums(id INTEGER PRIMARY KEY, path TEXT, file_types INTEGER, small_image_filter INTEGER, excluded_folders TEXT);
             INSERT INTO albums VALUES(1, '/', 7, NULL, '[]');
             CREATE TABLE afolders(id INTEGER PRIMARY KEY, album_id INTEGER, path TEXT, is_excluded_from_search INTEGER);
             INSERT INTO afolders VALUES(1, 1, '/photos', 0), (2, 1, '/private', 1);
             ALTER TABLE afiles ADD COLUMN folder_id INTEGER;
             INSERT INTO afiles(id, folder_id) VALUES(1, 1), (2, 2);",
        )
        .unwrap();
        let rows = vec![
            (1_i64, 4_usize, vec![0_u8; 16]),
            (2_i64, 4_usize, vec![0_u8; 16]),
        ];
        let kept = filter_excluded_rows(&conn, rows);
        let kept_ids: Vec<i64> = kept.iter().map(|(id, _, _)| *id).collect();
        assert_eq!(kept_ids, vec![1], "the excluded folder's row is dropped");
    }

    #[test]
    fn schemas_without_folders_keep_every_row() {
        let conn = fixture();
        let rows = vec![(7_i64, 4_usize, vec![0_u8; 16])];
        let kept = filter_excluded_rows(&conn, rows);
        assert_eq!(kept.len(), 1);
    }
}

#[cfg(test)]
mod local_model_resolution_tests {
    use super::*;

    fn models_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lap-embed-model-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn seed_model(models: &Path) {
        let dir = models.join(LOCAL_EMBED_MODEL_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(LOCAL_EMBED_MODEL_FILE), b"model").unwrap();
        std::fs::write(dir.join(crate::t_common::AI_TOKENIZER), b"tokenizer").unwrap();
    }

    #[test]
    fn the_app_data_copy_wins_when_both_roots_carry_a_model() {
        let appdata = models_root("both-appdata");
        let resource = models_root("both-resource");
        seed_model(&appdata);
        seed_model(&resource);
        assert_eq!(
            resolve_local_model_dir(Some(&appdata), Some(&resource)),
            Some(appdata.join(LOCAL_EMBED_MODEL_NAME))
        );
    }

    #[test]
    fn the_resource_copy_serves_when_app_data_has_no_model() {
        let appdata = models_root("empty-appdata");
        let resource = models_root("seeded-resource");
        seed_model(&resource);
        assert_eq!(
            resolve_local_model_dir(Some(&appdata), Some(&resource)),
            Some(resource.join(LOCAL_EMBED_MODEL_NAME))
        );
    }

    #[test]
    fn an_incomplete_app_data_copy_falls_through_to_the_resource_copy() {
        let appdata = models_root("partial-appdata");
        let resource = models_root("full-resource");
        let partial = appdata.join(LOCAL_EMBED_MODEL_NAME);
        std::fs::create_dir_all(&partial).unwrap();
        std::fs::write(partial.join(LOCAL_EMBED_MODEL_FILE), b"model").unwrap();
        seed_model(&resource);
        assert_eq!(
            resolve_local_model_dir(Some(&appdata), Some(&resource)),
            Some(resource.join(LOCAL_EMBED_MODEL_NAME))
        );
    }

    #[test]
    fn no_copy_anywhere_means_no_local_model() {
        let appdata = models_root("none-appdata");
        let resource = models_root("none-resource");
        assert_eq!(
            resolve_local_model_dir(Some(&appdata), Some(&resource)),
            None
        );
        assert_eq!(resolve_local_model_dir(Some(&appdata), None), None);
    }
}
