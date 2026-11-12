//! Exact search over the library.
//!
//! The index is an FTS5 virtual table holding one row per file. Because the
//! FTS5 tokenizer treats a run of CJK characters as a single token, the text
//! is cut into bigrams before it is written: a Chinese run of two or more
//! characters becomes every adjacent pair, a run of one character stays whole.
//! Latin words are lowercased and kept as one token.
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;

/// The columns of the index, in display order.
const COLUMNS: [&str; 5] = ["name", "people", "meta", "tags", "caption"];

/// The schema of the full text index (migration 24).
///
/// A virtual table cannot be altered, so the columns are fixed here. The
/// `aux` column carries the file id as JSON for debugging and for showing
/// where a hit came from; it is not searched.
pub(crate) const FTS_SCHEMA_SQL: &str = "
    CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
        name,
        people,
        meta,
        tags,
        caption,
        aux UNINDEXED
    );
";

/// The default number of hits returned by a search.
pub const FTS_DEFAULT_LIMIT: i64 = 200;

/// The largest number of hits a search may return.
pub const FTS_MAX_LIMIT: i64 = 1000;

/// One hit of a search.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsHit {
    pub file_id: i64,
    pub matched_columns: Vec<String>,
}

/// The row counts of the index and of the library.
///
/// The two numbers tell whether the index is in step with the library: a
/// rebuild is offered when they drift apart.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FtsStatus {
    pub documents: i64,
    pub files: i64,
}

/// Split the text into the tokens the index stores.
///
/// CJK runs become bigrams, latin runs become lowercased words and everything
/// else separates tokens.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cjk: Vec<char> = Vec::new();
    let mut word: Vec<char> = Vec::new();

    for ch in text.chars() {
        if is_cjk(ch) {
            flush_word(&mut word, &mut tokens);
            cjk.push(ch);
        } else if ch.is_alphabetic() || ch.is_numeric() {
            flush_cjk(&mut cjk, &mut tokens);
            word.push(ch.to_lowercase().next().unwrap_or(ch));
        } else {
            flush_cjk(&mut cjk, &mut tokens);
            flush_word(&mut word, &mut tokens);
        }
    }
    flush_cjk(&mut cjk, &mut tokens);
    flush_word(&mut word, &mut tokens);

    tokens
}

/// The text written into one index column.
pub fn index_text(text: &str) -> String {
    let mut tokens = tokenize(text);
    // The index also carries the single CJK characters: the strict query
    // matches bigrams, but a query whose wording differs from the document
    // ("摘葡萄的小男孩" against "一个小男孩…采摘葡萄") cannot match every
    // bigram. The relaxed second pass in `search` needs the characters.
    for token in relaxed_tokens(text) {
        if !tokens.contains(&token) {
            tokens.push(token);
        }
    }
    tokens.join(" ")
}

/// The tokens of the relaxed second pass: every CJK character on its own and
/// every latin word. It matches a document as long as all of the characters
/// (and words) of the query appear in it, even when the phrasing differs.
pub fn relaxed_tokens(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut word: Vec<char> = Vec::new();
    for ch in text.chars() {
        if is_cjk(ch) {
            if !word.is_empty() {
                push_unique_word(&mut tokens, &mut word);
            }
            push_unique_char(&mut tokens, ch);
        } else if ch.is_alphabetic() || ch.is_numeric() {
            word.push(ch.to_lowercase().next().unwrap_or(ch));
        } else if !word.is_empty() {
            push_unique_word(&mut tokens, &mut word);
        }
    }
    if !word.is_empty() {
        push_unique_word(&mut tokens, &mut word);
    }
    tokens
}

/// Push a latin word unless the same token is already present.
fn push_unique_word(tokens: &mut Vec<String>, word: &mut Vec<char>) {
    let token: String = word.iter().collect();
    word.clear();
    if !tokens.contains(&token) {
        tokens.push(token);
    }
}

/// Push one CJK character unless it is already present.
fn push_unique_char(tokens: &mut Vec<String>, ch: char) {
    let token = ch.to_string();
    if !tokens.contains(&token) {
        tokens.push(token);
    }
}

/// True when the character belongs to a CJK script.
///
/// The standard library has no Unicode script categories, so the Han,
/// Hiragana, Katakana and Hangul blocks are matched by range. The ranges
/// cover the blocks a photo library realistically holds.
fn is_cjk(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x11FF    // Hangul jamo
        | 0x3005           // CJK iteration mark (々)
        | 0x3040..=0x309F  // Hiragana
        | 0x30A0..=0x30FF  // Katakana
        | 0x3105..=0x312F  // Bopomofo
        | 0x3130..=0x318F  // Hangul compatibility jamo
        | 0x31C0..=0x31EF  // CJK strokes
        | 0x31F0..=0x31FF  // Katakana phonetic extensions
        | 0x3400..=0x4DBF  // CJK unified ideographs extension A
        | 0x4E00..=0x9FFF  // CJK unified ideographs
        | 0xA960..=0xA97F  // Hangul jamo extended-A
        | 0xAC00..=0xD7A3  // Hangul syllables
        | 0xD7B0..=0xD7FF  // Hangul jamo extended-B
        | 0xF900..=0xFAFF  // CJK compatibility ideographs
        | 0xFF66..=0xFF9F) // Halfwidth katakana
}

/// Write a CJK run as one bigram per adjacent pair.
fn flush_cjk(cjk: &mut Vec<char>, tokens: &mut Vec<String>) {
    if cjk.len() == 1 {
        tokens.push(cjk[0].to_string());
    } else {
        for pair in cjk.windows(2) {
            tokens.push(pair.iter().collect());
        }
    }
    cjk.clear();
}

/// Write a latin run as one lowercased word.
fn flush_word(word: &mut Vec<char>, tokens: &mut Vec<String>) {
    if !word.is_empty() {
        tokens.push(word.iter().collect());
        word.clear();
    }
}

/// The text of one file that goes into the index.
struct FtsDoc {
    name: String,
    people: String,
    meta: String,
    tags: String,
    caption: String,
}

/// The file columns that go into the `meta` column of the index.
const META_COLUMNS: [&str; 10] = [
    "comments",
    "e_title",
    "e_artist",
    "e_make",
    "e_model",
    "e_lens_make",
    "e_lens_model",
    "geo_name",
    "geo_admin1",
    "geo_cc",
];

/// The file columns the index reads.
///
/// A write to one of them changes the document of the file, so the write path
/// has to index the file again.
pub(crate) const INDEXED_FILE_COLUMNS: [&str; 11] = [
    "name",
    "comments",
    "e_title",
    "e_artist",
    "e_make",
    "e_model",
    "e_lens_make",
    "e_lens_model",
    "geo_name",
    "geo_admin1",
    "geo_cc",
];

/// Write the document of one file into the index.
pub fn upsert_doc(conn: &Connection, file_id: i64) -> Result<(), String> {
    let Some(doc) = build_doc(conn, file_id)? else {
        return Ok(());
    };
    write_doc(conn, file_id, &doc)
}

/// Drop the document of one file from the index.
pub fn remove_doc(conn: &Connection, file_id: i64) -> Result<(), String> {
    if !table_exists(conn, "files_fts")? {
        return Ok(());
    }
    conn.execute("DELETE FROM files_fts WHERE rowid = ?1", params![file_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Search the index.
///
/// The query is split into the same tokens the index stores (CJK bigrams and
/// characters, latin words). Every matching document is a candidate, ranked
/// by FTS5's BM25: a document containing more and rarer query tokens scores
/// higher. Ranking instead of filtering is what makes a natural sentence work
/// — the old all-token rule dropped a photo as soon as one word of the query
/// was phrased differently ("小男孩" against "孩子").
///
/// A query of one CJK character cannot match the bigrams of the index, so it
/// keeps the plain text fallback.
pub fn search(conn: &Connection, query: &str, limit: i64) -> Result<Vec<FtsHit>, String> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(0, FTS_MAX_LIMIT);
    if tokens.len() == 1 && is_single_cjk(&tokens[0]) {
        return like_search(conn, &tokens[0], limit);
    }
    // The query carries the same token shapes as the index: bigrams give the
    // phrase precision, the single characters keep a synonym-heavy query
    // ("小男孩躺在草地上" against "孩子仰躺…草地上") matched by the words it
    // does share. BM25 weighs the rare tokens up and the common ones down.
    // Short queries stay bigram-only: adding the characters of 小明 would
    // also match 小红 through the shared 小.
    let mut query_tokens = tokens;
    let cjk_chars = query.chars().filter(|ch| is_cjk(*ch)).count();
    if cjk_chars >= MIN_CJK_FOR_CHAR_TOKENS {
        for token in relaxed_tokens(query) {
            if !query_tokens.contains(&token) {
                query_tokens.push(token);
            }
        }
    }
    search_ranked(conn, &query_tokens, limit)
}

/// Below this many CJK characters a query is a short keyword (a name, a place)
/// and stays bigram-only.
const MIN_CJK_FOR_CHAR_TOKENS: usize = 3;

/// Rank the documents that match any query token by BM25.
fn search_ranked(
    conn: &Connection,
    tokens: &[String],
    limit: i64,
) -> Result<Vec<FtsHit>, String> {
    let quoted: Vec<String> = tokens
        .iter()
        .filter_map(|token| quote_token(token))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let quoted: Vec<String> = quoted
        .into_iter()
        .filter(|token| seen.insert(token.clone()))
        .collect();
    if quoted.is_empty() {
        return Ok(Vec::new());
    }
    let expression = quoted.join(" OR ");

    // The column order of the index is name, people, meta, tags, caption.
    // Weighting the caption matches human intent best: measured on a 40-photo
    // annotation set (113 queries, 2026-11-12), caption×5 lifted Recall@1
    // from 0.909 to 0.936; heavier name weights changed nothing.
    const BM25: &str = "bm25(files_fts, 1.0, 1.0, 1.0, 1.0, 5.0)";

    // FTS5's bm25() returns a negative score; ascending order is best first.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT rowid FROM files_fts WHERE files_fts MATCH ?1
             ORDER BY {BM25} LIMIT ?2"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![expression, limit], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    let ranked: Vec<i64> = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if ranked.is_empty() {
        return Ok(Vec::new());
    }

    // The hit badges still name the columns each returned document matched.
    let wanted: std::collections::HashSet<i64> = ranked.iter().copied().collect();
    let mut matched: HashMap<i64, Vec<String>> = HashMap::new();
    for column in COLUMNS {
        let column_query = format!("{}:({})", column, expression);
        let mut stmt = conn
            .prepare("SELECT rowid FROM files_fts WHERE files_fts MATCH ?1 LIMIT ?2")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![column_query, FTS_MAX_LIMIT], |row| row.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let file_id = row.map_err(|e| e.to_string())?;
            if !wanted.contains(&file_id) {
                continue;
            }
            let columns = matched.entry(file_id).or_default();
            if !columns.iter().any(|name| name == column) {
                columns.push(column.to_string());
            }
        }
    }

    Ok(ranked
        .into_iter()
        .map(|file_id| FtsHit {
            matched_columns: matched.remove(&file_id).unwrap_or_default(),
            file_id,
        })
        .collect())
}

/// Quote one token for the match expression.
///
/// The tokens come from `tokenize` and hold letters and digits only. The
/// check keeps a token that slips through from changing the query.
fn quote_token(token: &str) -> Option<String> {
    if token.is_empty() || !token.chars().all(|ch| ch.is_alphanumeric()) {
        return None;
    }
    Some(format!("\"{}\"", token.replace('"', "\"\"")))
}

/// True when the token is one CJK character.
fn is_single_cjk(token: &str) -> bool {
    let mut chars = token.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => is_cjk(ch),
        _ => false,
    }
}

/// The plain text search used for a query of one CJK character.
fn like_search(conn: &Connection, needle: &str, limit: i64) -> Result<Vec<FtsHit>, String> {
    let pattern = format!("%{}%", escape_like(needle));
    let mut clauses: Vec<String> = Vec::new();
    for column in existing_columns(conn, "afiles", &INDEXED_FILE_COLUMNS)? {
        clauses.push(format!("a.{column} LIKE ?1 ESCAPE '\\'"));
    }
    if table_exists(conn, "faces")? && table_exists(conn, "persons")? {
        clauses.push(
            "EXISTS (SELECT 1 FROM faces f JOIN persons p ON p.id = f.person_id
                WHERE f.file_id = a.id AND p.name LIKE ?1 ESCAPE '\\')"
                .to_string(),
        );
    }
    if table_exists(conn, "afile_tags")? && table_exists(conn, "atags")? {
        clauses.push(
            "EXISTS (SELECT 1 FROM afile_tags ft JOIN atags t ON t.id = ft.tag_id
                WHERE ft.file_id = a.id AND t.name LIKE ?1 ESCAPE '\\')"
                .to_string(),
        );
    }
    if table_exists(conn, "vl_annotations")? {
        clauses.push(
            "EXISTS (SELECT 1 FROM vl_annotations v WHERE v.file_id = a.id
                AND (v.raw_text LIKE ?1 ESCAPE '\\' OR v.result_json LIKE ?1 ESCAPE '\\'))"
                .to_string(),
        );
    }
    if clauses.is_empty() {
        return Ok(Vec::new());
    }

    let sql = format!(
        "SELECT DISTINCT a.id FROM afiles a WHERE {} ORDER BY a.id LIMIT ?2",
        clauses.join(" OR ")
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![pattern, limit], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?;
    let mut hits = Vec::new();
    for row in rows {
        hits.push(FtsHit {
            file_id: row.map_err(|e| e.to_string())?,
            matched_columns: vec!["like".to_string()],
        });
    }
    Ok(hits)
}

/// Escape the wildcards of the plain text search.
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Build the document of one file from the library.
fn build_doc(conn: &Connection, file_id: i64) -> Result<Option<FtsDoc>, String> {
    if !table_exists(conn, "afiles")? {
        return Ok(None);
    }
    let exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM afiles WHERE id = ?1",
            params![file_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Ok(None);
    }

    let name = index_text(&file_texts(conn, &["name"], file_id)?.join(" "));
    let meta = index_text(&file_texts(conn, &META_COLUMNS, file_id)?.join(" "));
    let people = index_text(&people_of(conn, file_id)?.join(" "));
    let tags = index_text(&tags_of(conn, file_id)?.join(" "));
    let caption = index_text(&caption_of(conn, file_id)?.join(" "));

    Ok(Some(FtsDoc {
        name,
        people,
        meta,
        tags,
        caption,
    }))
}

/// Write one document into the index, replacing the previous one.
fn write_doc(conn: &Connection, file_id: i64, doc: &FtsDoc) -> Result<(), String> {
    conn.execute("DELETE FROM files_fts WHERE rowid = ?1", params![file_id])
        .map_err(|e| e.to_string())?;
    let aux = serde_json::json!({ "fileId": file_id }).to_string();
    conn.execute(
        "INSERT INTO files_fts(rowid, name, people, meta, tags, caption, aux)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            file_id,
            doc.name,
            doc.people,
            doc.meta,
            doc.tags,
            doc.caption,
            aux
        ],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The non empty texts of the given columns of one file.
fn file_texts(conn: &Connection, columns: &[&str], file_id: i64) -> Result<Vec<String>, String> {
    let available = existing_columns(conn, "afiles", columns)?;
    if available.is_empty() {
        return Ok(Vec::new());
    }
    let select = available
        .iter()
        .map(|column| format!("COALESCE({}, '')", column))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT {} FROM afiles WHERE id = ?1", select);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt.query(params![file_id]).map_err(|e| e.to_string())?;
    let mut texts = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        for index in 0..available.len() {
            let value: String = row.get(index).map_err(|e| e.to_string())?;
            if !value.trim().is_empty() {
                texts.push(value);
            }
        }
    }
    Ok(texts)
}

/// The names of the people a file shows.
fn people_of(conn: &Connection, file_id: i64) -> Result<Vec<String>, String> {
    if existing_columns(conn, "faces", &["file_id", "person_id"])?.is_empty()
        || existing_columns(conn, "persons", &["id", "name"])?.is_empty()
    {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT p.name FROM faces f JOIN persons p ON p.id = f.person_id
             WHERE f.file_id = ?1 AND p.name IS NOT NULL AND TRIM(p.name) <> ''",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![file_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row.map_err(|e| e.to_string())?);
    }
    names.sort();
    Ok(names)
}

/// The tags of a file, with the path of the tag tree when there is one.
fn tags_of(conn: &Connection, file_id: i64) -> Result<Vec<String>, String> {
    if existing_columns(conn, "afile_tags", &["file_id", "tag_id"])?.is_empty()
        || existing_columns(conn, "atags", &["name"])?.is_empty()
    {
        return Ok(Vec::new());
    }
    let has_parents = !existing_columns(conn, "atags", &["parent_id"])?.is_empty();
    let sql = if has_parents {
        "WITH RECURSIVE ancestors(tag_id, id, parent_id, path, depth) AS (
             SELECT t.id, t.id, t.parent_id, t.name, 0
               FROM atags t JOIN afile_tags ft ON ft.tag_id = t.id WHERE ft.file_id = ?1
             UNION ALL
             SELECT a.tag_id, t.id, t.parent_id, a.path || '|' || t.name, a.depth + 1
               FROM atags t JOIN ancestors a ON t.id = a.parent_id WHERE a.depth < 64
         )
         SELECT path FROM ancestors WHERE parent_id IS NULL"
    } else {
        "SELECT t.name FROM afile_tags ft JOIN atags t ON t.id = ft.tag_id WHERE ft.file_id = ?1"
    };
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![file_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut tags = Vec::new();
    for row in rows {
        tags.push(row.map_err(|e| e.to_string())?);
    }
    tags.sort();
    Ok(tags)
}

/// The annotation texts of a file.
fn caption_of(conn: &Connection, file_id: i64) -> Result<Vec<String>, String> {
    if existing_columns(conn, "vl_annotations", &["file_id"])?.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT result_json, raw_text FROM vl_annotations WHERE file_id = ?1 ORDER BY id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![file_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| e.to_string())?;
    let mut texts = Vec::new();
    for row in rows {
        let (result_json, raw_text) = row.map_err(|e| e.to_string())?;
        let values = json_values(&result_json);
        if values.is_empty() {
            if !raw_text.trim().is_empty() {
                texts.push(raw_text);
            }
        } else {
            texts.extend(values);
        }
    }
    Ok(texts)
}

/// The string values of a JSON document, without the keys.
fn json_values(text: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let mut values = Vec::new();
    collect_json_values(&value, &mut values);
    values
}

fn collect_json_values(value: &serde_json::Value, values: &mut Vec<String>) {
    match value {
        serde_json::Value::String(text) => {
            if !text.trim().is_empty() {
                values.push(text.clone());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json_values(item, values);
            }
        }
        serde_json::Value::Object(entries) => {
            for (_, item) in entries {
                collect_json_values(item, values);
            }
        }
        _ => {}
    }
}

/// True when a write to the column changes the indexed document.
pub(crate) fn column_is_indexed(column: &str) -> bool {
    INDEXED_FILE_COLUMNS.contains(&column)
}

/// True when the table exists.
fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?1",
            params![table],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(count > 0)
}

/// The wanted columns a table really has.
///
/// A table that does not exist has no columns, so the pragma alone tells
/// whether the table is there.
fn existing_columns(
    conn: &Connection,
    table: &str,
    wanted: &[&str],
) -> Result<Vec<String>, String> {
    let sql = format!("PRAGMA table_info({})", table);
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let mut rows = stmt.query([]).map_err(|e| e.to_string())?;
    let mut available: Vec<String> = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        available.push(row.get::<_, String>(1).map_err(|e| e.to_string())?);
    }
    Ok(wanted
        .iter()
        .filter(|column| available.iter().any(|name| name == *column))
        .map(|column| column.to_string())
        .collect())
}

/// Rebuild the whole index.
pub fn reindex_all() -> Result<usize, String> {
    let conn = crate::t_sqlite::open_conn()?;
    reindex_all_with_conn(&conn)
}

/// Rebuild the whole index on an existing connection.
pub fn reindex_all_with_conn(conn: &Connection) -> Result<usize, String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM files_fts", [])
        .map_err(|e| e.to_string())?;
    let ids: Vec<i64> = {
        let mut stmt = tx
            .prepare("SELECT id FROM afiles ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<i64>, _>>()
            .map_err(|e| e.to_string())?
    };
    for id in &ids {
        upsert_doc(&tx, *id)?;
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(ids.len())
}

/// Search the index from the command layer.
#[tauri::command]
pub fn search_fts(query: String, limit: Option<i64>) -> Result<Vec<FtsHit>, String> {
    let conn = crate::t_sqlite::open_conn()?;
    search(&conn, &query, limit.unwrap_or(FTS_DEFAULT_LIMIT))
}

/// Rebuild the whole index from the command layer. The rebuild walks every
/// file of the library, so it runs on the blocking thread pool instead of the
/// UI thread.
#[tauri::command]
pub async fn fts_reindex() -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(reindex_all)
        .await
        .map_err(|e| format!("FTS reindex task failed: {e}"))?
}

/// The row counts of the index and the library on an existing connection.
///
/// A database without the migration counts as empty rather than an error, so
/// the status can be shown before the first scan.
pub fn fts_status_with_conn(conn: &Connection) -> Result<FtsStatus, String> {
    let documents = if table_exists(conn, "files_fts")? {
        conn.query_row("SELECT COUNT(*) FROM files_fts", [], |row| row.get(0))
            .map_err(|e| e.to_string())?
    } else {
        0
    };
    let files = if table_exists(conn, "afiles")? {
        conn.query_row("SELECT COUNT(*) FROM afiles", [], |row| row.get(0))
            .map_err(|e| e.to_string())?
    } else {
        0
    };
    Ok(FtsStatus { documents, files })
}

/// The row counts of the index and the library from the command layer.
#[tauri::command]
pub fn fts_status() -> Result<FtsStatus, String> {
    let conn = crate::t_sqlite::open_conn()?;
    fts_status_with_conn(&conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library with the tables the index reads from.
    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE afiles(
                 id INTEGER PRIMARY KEY,
                 name TEXT,
                 comments TEXT,
                 e_title TEXT,
                 e_artist TEXT,
                 e_make TEXT,
                 e_model TEXT,
                 e_lens_make TEXT,
                 e_lens_model TEXT,
                 geo_name TEXT,
                 geo_admin1 TEXT,
                 geo_cc TEXT
             );
             CREATE TABLE atags(
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL,
                 parent_id INTEGER REFERENCES atags(id) ON DELETE CASCADE
             );
             CREATE TABLE afile_tags(file_id INTEGER, tag_id INTEGER, PRIMARY KEY(file_id, tag_id));
             CREATE TABLE persons(id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE faces(id INTEGER PRIMARY KEY, file_id INTEGER, person_id INTEGER);
             CREATE TABLE vl_annotations(
                 id INTEGER PRIMARY KEY,
                 file_id INTEGER,
                 result_json TEXT,
                 raw_text TEXT
             );",
        )
        .unwrap();
        conn.execute_batch(FTS_SCHEMA_SQL).unwrap();
        conn
    }

    /// One file with a person, a tag tree, metadata and an annotation.
    fn seed_file(conn: &Connection, id: i64, name: &str) {
        conn.execute(
            "INSERT INTO afiles(id, name, comments, e_title, e_artist, e_make, e_model,
                 e_lens_make, e_lens_model, geo_name, geo_admin1, geo_cc)
             VALUES (?1, ?2, '春节团聚', '家庭聚会', 'Some Artist', 'Canon', 'EOS R5',
                 'Canon', 'RF 24-70', '杭州西湖', '浙江', 'CN')",
            params![id, name],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO atags(id, name, parent_id) VALUES (1, '旅行', NULL), (2, '春节', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO afile_tags(file_id, tag_id) VALUES (?1, 2)",
            params![id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO persons(id, name) VALUES (1, '小明')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO faces(id, file_id, person_id) VALUES (?1, ?1, 1)",
            params![id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO vl_annotations(id, file_id, result_json, raw_text)
             VALUES (?1, ?1, '{\"caption\":\"a cat\"}', '{\"caption\":\"a cat\"}')",
            params![id],
        )
        .unwrap();
    }

    /// The indexed text of one column of one document.
    fn column(conn: &Connection, name: &str, file_id: i64) -> String {
        conn.query_row(
            &format!("SELECT {} FROM files_fts WHERE rowid = ?1", name),
            params![file_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// The file ids of a search.
    fn hits(conn: &Connection, query: &str) -> Vec<i64> {
        search(conn, query, 100)
            .unwrap()
            .into_iter()
            .map(|hit| hit.file_id)
            .collect()
    }

    #[test]
    fn a_cjk_run_becomes_bigrams() {
        assert_eq!(tokenize("过年回家"), vec!["过年", "年回", "回家"]);
    }

    #[test]
    fn a_single_cjk_character_stays_whole() {
        assert_eq!(tokenize("过"), vec!["过"]);
    }

    #[test]
    fn latin_runs_become_lowercase_words() {
        assert_eq!(tokenize("Chinese New Year"), vec!["chinese", "new", "year"]);
    }

    #[test]
    fn latin_and_cjk_mix_without_a_separator() {
        assert_eq!(tokenize("照片Photo2"), vec!["照片", "photo2"]);
    }

    #[test]
    fn punctuation_and_emoji_separate_tokens() {
        assert_eq!(tokenize("你好，世界！🎉"), vec!["你好", "世界"]);
    }

    #[test]
    fn iteration_marks_bopomofo_and_strokes_are_cjk() {
        // 々 (U+3005), ㄅ (U+3105) and ㇀ (U+31C0) belong to the CJK runs.
        assert_eq!(tokenize("人々"), vec!["人々"]);
        assert_eq!(tokenize("ㄅㄆ"), vec!["ㄅㄆ"]);
        assert_eq!(tokenize("㇀㇁"), vec!["㇀㇁"]);
    }

    #[test]
    fn an_empty_text_produces_no_tokens() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("  \n\t ").is_empty());
        assert!(tokenize("，。!").is_empty());
    }

    #[test]
    fn index_text_joins_the_tokens_with_spaces() {
        // Bigrams first, then the single characters the relaxed pass needs.
        assert_eq!(index_text("过年回家"), "过年 年回 回家 过 年 回 家");
        assert_eq!(index_text("Chinese New Year"), "chinese new year");
        assert_eq!(index_text(""), "");
    }

    #[test]
    fn the_relaxed_tokens_are_characters_and_words() {
        assert_eq!(
            relaxed_tokens("摘葡萄的小男孩"),
            vec!["摘", "葡", "萄", "的", "小", "男", "孩"]
        );
        assert_eq!(relaxed_tokens("葡萄 Photo2"), vec!["葡", "萄", "photo2"]);
        assert_eq!(relaxed_tokens(""), Vec::<String>::new());
    }

    #[test]
    fn a_document_is_written_column_by_column() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();

        assert_eq!(
            column(&conn, "name", 1),
            "过年 年回 回家 jpg 过 年 回 家"
        );
        assert_eq!(column(&conn, "people", 1), "小明 小 明");
        assert_eq!(column(&conn, "tags", 1), "春节 旅行 春 节 旅 行");
        assert_eq!(column(&conn, "caption", 1), "a cat");
        assert_eq!(
            column(&conn, "meta", 1),
            "春节 节团 团聚 家庭 庭聚 聚会 some artist canon eos r5 canon rf 24 70 杭州 州西 西湖 浙江 cn 春 节 团 聚 家 庭 会 杭 州 西 湖 浙 江"
        );
    }

    /// Run queries against a real library database for manual acceptance.
    ///
    /// `LAP_FTS_DB` points at the library file (read only), `LAP_FTS_QUERIES`
    /// holds semicolon separated queries (defaults to the reported cases).
    #[test]
    #[ignore = "reads a real library database, set LAP_FTS_DB"]
    fn real_library_search_probe() {
        let path = std::env::var("LAP_FTS_DB").expect("set LAP_FTS_DB");
        let queries = std::env::var("LAP_FTS_QUERIES")
            .unwrap_or_else(|_| "摘葡萄的小男孩;摘葡萄的小女孩".to_string());
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .expect("open the library read only");
        for query in queries.split(';').map(str::trim).filter(|q| !q.is_empty()) {
            let hits = search(&conn, query, 50).expect("search");
            let names: Vec<String> = hits
                .iter()
                .map(|hit| {
                    conn.query_row(
                        "SELECT name FROM afiles WHERE id = ?1",
                        params![hit.file_id],
                        |row| row.get::<_, String>(0),
                    )
                    .unwrap_or_else(|_| format!("#{}", hit.file_id))
                })
                .collect();
            if names.is_empty() {
                println!("{query} => (no hits)");
            } else {
                println!("{query} => {} hit(s): {}", names.len(), names.join(", "));
            }
        }
    }

    #[test]
    fn a_synonym_query_still_finds_the_document() {
        let conn = fixture();
        seed_file(&conn, 1, "A (1).jpg");
        // The caption calls the child 孩子 and uses 仰躺; the query says
        // 小男孩 / 躺在. The old all-character rule missed 小 and 男, so the
        // document disappeared; a ranked lexical search keeps it.
        conn.execute(
            "UPDATE vl_annotations SET result_json = ?1, raw_text = ?1 WHERE file_id = 1",
            params![
                "{\"caption\":\"阳光下的草地上，一个穿蓝色羽绒服的孩子仰躺着，旁边停着一辆橙色滑板车。\"}"
            ],
        )
        .unwrap();
        upsert_doc(&conn, 1).unwrap();

        assert_eq!(hits(&conn, "一个小男孩躺在草地上"), vec![1]);
    }

    #[test]
    fn a_reworded_query_is_ranked_not_dropped() {
        let conn = fixture();
        seed_file(&conn, 1, "C (16).jpg");
        // The wording of the query does not appear verbatim in the caption.
        // The bigrams spanning word boundaries ("萄的", "的小") are absent, but
        // the overlapping tokens still rank the photo first.
        conn.execute(
            "UPDATE vl_annotations SET result_json = ?1, raw_text = ?1 WHERE file_id = 1",
            params![
                "{\"caption\":\"一个小男孩在葡萄园里采摘葡萄，他穿着紫白格子衬衫，一手举起一串刚采摘的绿葡萄。\"}"
            ],
        )
        .unwrap();
        upsert_doc(&conn, 1).unwrap();

        assert_eq!(hits(&conn, "摘葡萄"), vec![1], "the exact phrase matches");
        assert_eq!(
            hits(&conn, "摘葡萄的小男孩"),
            vec![1],
            "a reworded query still ranks the photo"
        );
        assert_eq!(
            hits(&conn, "摘葡萄的小女孩"),
            vec![1],
            "a partly wrong description ranks the photo instead of hiding it"
        );
        assert!(
            hits(&conn, "航空发动机维修").is_empty(),
            "a query without a single shared token stays empty"
        );
    }

    #[test]
    fn a_document_is_replaced_instead_of_duplicated() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();
        upsert_doc(&conn, 1).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM files_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    #[test]
    fn removing_a_document_takes_it_out_of_the_index() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();
        remove_doc(&conn, 1).unwrap();
        assert!(hits(&conn, "过年").is_empty());
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM files_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_search_matches_every_column() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();

        assert_eq!(hits(&conn, "回家"), vec![1], "the file name must match");
        assert_eq!(hits(&conn, "小明"), vec![1], "the person must match");
        assert_eq!(hits(&conn, "Canon"), vec![1], "the camera must match");
        assert_eq!(hits(&conn, "西湖"), vec![1], "the place must match");
        assert_eq!(hits(&conn, "cat"), vec![1], "the annotation must match");
    }

    #[test]
    fn more_matching_tokens_rank_higher() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        seed_file(&conn, 2, "回家路上.jpg");
        upsert_doc(&conn, 1).unwrap();
        upsert_doc(&conn, 2).unwrap();

        assert_eq!(hits(&conn, "过年"), vec![1]);
        // Both documents share 回家; the one that also carries 过年 ranks first.
        assert_eq!(hits(&conn, "过年 回家"), vec![1, 2]);
    }

    #[test]
    fn a_single_cjk_character_falls_back_to_a_like_search() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();

        let result = search(&conn, "过", 100).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_id, 1);
        assert_eq!(result[0].matched_columns, vec!["like".to_string()]);
    }

    #[test]
    fn the_matched_columns_name_every_column_that_matched() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();

        let result = search(&conn, "春节", 100).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].matched_columns, vec!["meta", "tags"]);
    }

    #[test]
    fn a_search_is_limited() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        seed_file(&conn, 2, "过年回家2.jpg");
        upsert_doc(&conn, 1).unwrap();
        upsert_doc(&conn, 2).unwrap();

        assert_eq!(search(&conn, "过年", 1).unwrap().len(), 1);
        assert_eq!(search(&conn, "过年", 100).unwrap().len(), 2);
    }

    #[test]
    fn a_table_that_is_not_there_has_no_columns() {
        let conn = fixture();
        assert!(existing_columns(&conn, "afiles", &["name"])
            .unwrap()
            .contains(&"name".to_string()));
        assert!(existing_columns(&conn, "no_such_table", &["name"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn an_empty_query_returns_nothing() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        upsert_doc(&conn, 1).unwrap();

        assert!(search(&conn, "", 100).unwrap().is_empty());
        assert!(search(&conn, "  ,。!", 100).unwrap().is_empty());
    }

    /// The whole index as one string, so two runs can be compared.
    fn snapshot(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT rowid, name, people, meta, tags, caption FROM files_fts ORDER BY rowid",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}|{}|{}|{}",
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .unwrap();
        rows.map(|row| row.unwrap()).collect()
    }

    #[test]
    fn a_reindex_builds_the_same_index_twice() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        seed_file(&conn, 2, "春节团聚.jpg");
        // A document of a file that is not in the library any more.
        conn.execute(
            "INSERT INTO files_fts(rowid, name, people, meta, tags, caption, aux)
             VALUES (99, '旧文档', '', '', '', '', '{}')",
            [],
        )
        .unwrap();

        assert_eq!(reindex_all_with_conn(&conn).unwrap(), 2);
        let first = snapshot(&conn);
        assert_eq!(reindex_all_with_conn(&conn).unwrap(), 2);
        let second = snapshot(&conn);

        assert_eq!(first, second, "a reindex has to be idempotent");
        assert_eq!(first.len(), 2);
        assert!(!first.iter().any(|row| row.contains("旧文档")));
    }

    #[test]
    fn the_status_counts_the_index_and_the_library() {
        let conn = fixture();
        seed_file(&conn, 1, "过年回家.jpg");
        seed_file(&conn, 2, "春节团聚.jpg");
        upsert_doc(&conn, 1).unwrap();
        upsert_doc(&conn, 2).unwrap();

        let status = fts_status_with_conn(&conn).unwrap();
        assert_eq!(status.documents, 2, "two indexed documents");
        assert_eq!(status.files, 2, "two library files");

        conn.execute("DELETE FROM afiles WHERE id = 1", []).unwrap();
        remove_doc(&conn, 1).unwrap();
        let status = fts_status_with_conn(&conn).unwrap();
        assert_eq!(status.documents, 1);
        assert_eq!(status.files, 1);
    }

    #[test]
    fn the_status_counts_an_empty_library_as_zero() {
        let conn = Connection::open_in_memory().unwrap();
        // A database without the migration still answers, with zeros.
        let status = fts_status_with_conn(&conn).unwrap();
        assert_eq!(status.documents, 0);
        assert_eq!(status.files, 0);
    }

    // ----- command registration -----

    /// The commands this module contributes to the Tauri handler.
    const FTS_COMMANDS: [&str; 3] = ["search_fts", "fts_reindex", "fts_status"];

    #[test]
    fn every_command_is_registered_in_main() {
        let main = include_str!("main.rs");
        let handler = main
            .split("generate_handler![")
            .nth(1)
            .and_then(|rest| rest.split("])").next())
            .expect("main.rs must register a handler list");
        for command in FTS_COMMANDS {
            assert!(
                handler.contains(&format!("t_fts::{command}")),
                "command {command} is not registered in main.rs"
            );
        }
    }

    #[test]
    fn the_handler_does_not_register_unknown_fts_commands() {
        let main = include_str!("main.rs");
        let handler = main
            .split("generate_handler![")
            .nth(1)
            .and_then(|rest| rest.split("])").next())
            .expect("main.rs must register a handler list");
        let registered: Vec<&str> = handler
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix("t_fts::"))
            .map(|line| line.trim_end_matches(','))
            .collect();
        assert_eq!(
            registered.len(),
            FTS_COMMANDS.len(),
            "unexpected fts commands in main.rs: {registered:?}"
        );
        for command in registered {
            assert!(
                FTS_COMMANDS.contains(&command),
                "main.rs registers an unknown fts command: {command}"
            );
        }
    }
}
