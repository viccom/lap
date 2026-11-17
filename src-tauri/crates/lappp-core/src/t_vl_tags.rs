//! Write the tags a vision-language result carries into the library tag tree.
//!
//! Every AI-produced tag lives under one root tag ("AI理解"). The root is the
//! provenance marker: a re-run replaces the file's tags that live under it and
//! never touches a tag the user wrote by hand. Writing goes through the
//! connection the caller passes in, so a run pinned to a library writes into
//! that library and not into whatever is active right now.
//!
//! The extraction half is pure and unit tested without a database.

use rusqlite::{Connection, params};

/// The root tag every AI-produced tag is nested under.
pub const VL_TAG_ROOT: &str = "AI理解";

/// The result fields that may carry the tags, in priority order: the first one
/// the response spec declares wins, otherwise the first one the value carries.
pub const VL_TAG_FIELDS: [&str; 3] = ["tags", "keywords", "labels"];

/// The most tags one result may contribute; a runaway model cannot flood the
/// tag tree.
pub const VL_TAG_LIMIT: usize = 50;

/// The name of the result field that carries the tags, if any.
///
/// The spec is the template's declared shape: a `tags` field declared there is
/// authoritative even when the answer omits it. Without a declaration the
/// first known field present in the value is used.
pub fn tag_field(spec: &serde_json::Value, value: &serde_json::Value) -> Option<String> {
    if let Some(object) = spec.as_object() {
        for field in VL_TAG_FIELDS {
            if object.contains_key(field) {
                return Some(field.to_string());
            }
        }
    }
    if let Some(object) = value.as_object() {
        for field in VL_TAG_FIELDS {
            if object.contains_key(field) {
                return Some(field.to_string());
            }
        }
    }
    None
}

/// Clean one tag from a result, or `None` when it is unusable.
///
/// A tag is flat and cannot carry the characters the tag tree reserves as
/// separators; such a value is dropped rather than silently rewritten.
pub fn normalize_tag(raw: &str) -> Option<String> {
    let mut cleaned = String::new();
    for part in raw.split_whitespace() {
        if !cleaned.is_empty() {
            cleaned.push(' ');
        }
        cleaned.push_str(part);
    }
    if cleaned.is_empty()
        || cleaned.chars().count() > 255
        || cleaned.contains([',', ';', '|'])
        || cleaned.contains('\0')
    {
        return None;
    }
    Some(cleaned)
}

/// The tags a result contributes: the field's string entries, cleaned,
/// de-duplicated case-insensitively and capped.
pub fn tags_from_result(value: &serde_json::Value, spec: &serde_json::Value) -> Vec<String> {
    let Some(field) = tag_field(spec, value) else {
        return Vec::new();
    };
    let Some(items) = value.get(&field).and_then(|entry| entry.as_array()) else {
        return Vec::new();
    };
    let mut tags: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for item in items {
        let Some(text) = item.as_str() else {
            continue;
        };
        let Some(tag) = normalize_tag(text) else {
            continue;
        };
        let key = tag.to_lowercase();
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        tags.push(tag);
        if tags.len() >= VL_TAG_LIMIT {
            break;
        }
    }
    tags
}

/// The id of the AI root tag, created on first use. The unique root index
/// makes a concurrent first write a no-op instead of a failed transaction
/// (REVIEW-2026-11-14 P1-14).
fn ensure_root(conn: &Connection) -> Result<i64, String> {
    if let Some(id) = crate::t_tags::find(conn, VL_TAG_ROOT, None)? {
        return Ok(id);
    }
    conn.execute(
        "INSERT OR IGNORE INTO atags(name, parent_id) VALUES(?1, NULL)",
        params![VL_TAG_ROOT],
    )
    .map_err(|e| e.to_string())?;
    crate::t_tags::find(conn, VL_TAG_ROOT, None)?
        .ok_or_else(|| "failed to create the AI tag root".to_string())
}

/// The id of one child of the root, created on first use; the unique sibling
/// index keeps a racing first write from failing the transaction (P1-14).
fn ensure_child(conn: &Connection, root: i64, name: &str) -> Result<i64, String> {
    if let Some(id) = crate::t_tags::find(conn, name, Some(root))? {
        return Ok(id);
    }
    conn.execute(
        "INSERT OR IGNORE INTO atags(name, parent_id) VALUES(?1, ?2)",
        params![name, root],
    )
    .map_err(|e| e.to_string())?;
    crate::t_tags::find(conn, name, Some(root))?
        .ok_or_else(|| format!("failed to create the AI tag '{name}'"))
}

fn tables_present(conn: &Connection) -> bool {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('atags','afile_tags','afiles')",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    count == 3
}

/// Replace the AI tags of a file with the tags of a fresh result.
///
/// Only the file's tags that live under [`VL_TAG_ROOT`] are replaced; tags the
/// user wrote by hand stay. An empty `tags` slice is a no-op: a result without
/// tags must not silently erase what a previous run produced.
pub fn apply_tags(conn: &Connection, file_id: i64, tags: &[String]) -> Result<usize, String> {
    if tags.is_empty() || !tables_present(conn) {
        return Ok(0);
    }
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    let root = ensure_root(&tx)?;

    // Drop this file's previous AI tags; everything outside the root stays.
    let remove_sql = format!(
        "{} DELETE FROM afile_tags WHERE file_id = ?2 AND tag_id IN (SELECT id FROM descendants)",
        crate::t_tags::DESCENDANTS
    );
    tx.execute(&remove_sql, params![root, file_id])
        .map_err(|e| e.to_string())?;

    let mut linked = 0usize;
    for tag in tags {
        let Some(name) = normalize_tag(tag) else {
            continue;
        };
        let tag_id = ensure_child(&tx, root, &name)?;
        let inserted = tx
            .execute(
                "INSERT OR IGNORE INTO afile_tags(file_id, tag_id) VALUES(?1, ?2)",
                params![file_id, tag_id],
            )
            .map_err(|e| e.to_string())?;
        if inserted > 0 {
            linked += 1;
        }
    }

    tx.execute(
        "UPDATE afiles SET has_tags = 1 WHERE id = ?1",
        params![file_id],
    )
    .map_err(|e| e.to_string())?;

    // The afile_tags triggers latch the Tags dirty bit on any change, and a
    // latched bit makes non-destructive sidecar imports skip this file's
    // tags forever. AI tags are machine-written: they must not borrow the
    // protection that exists for *user* edits, so the bit is cleared again
    // inside the same transaction (schemas without axmp_imports skip this).
    let has_imports: bool = tx
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='axmp_imports'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count == 1)
        .unwrap_or(false);
    if has_imports {
        let keep_mask = !(crate::metadata::schema::CatalogField::Tags.mask() as i64);
        tx.execute(
            "INSERT INTO axmp_imports(file_id, dirty) VALUES(?1, 0)
             ON CONFLICT(file_id) DO UPDATE SET dirty = dirty & ?2",
            params![file_id, keep_mask],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())?;

    // The new tags have to be findable right away; a failure here is logged,
    // never fatal, exactly like the catalog's own tag writes.
    if let Err(e) = crate::t_fts::upsert_doc(conn, file_id) {
        eprintln!("fts index update failed for file {file_id}: {e}");
    }
    Ok(linked)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             CREATE TABLE afiles(id INTEGER PRIMARY KEY, name TEXT, has_tags INTEGER DEFAULT 0);
             CREATE TABLE atags(id INTEGER PRIMARY KEY, name TEXT NOT NULL, parent_id INTEGER REFERENCES atags(id) ON DELETE CASCADE);
             CREATE TABLE afile_tags(file_id INTEGER REFERENCES afiles(id) ON DELETE CASCADE, tag_id INTEGER REFERENCES atags(id) ON DELETE CASCADE, PRIMARY KEY(file_id, tag_id));
             INSERT INTO afiles VALUES(1,'a.jpg',0),(2,'b.jpg',0);",
        )
        .unwrap();
        conn.execute_batch(crate::t_fts::FTS_SCHEMA_SQL).unwrap();
        conn
    }

    fn names_of(conn: &Connection, file_id: i64) -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT t.name FROM afile_tags ft JOIN atags t ON t.id = ft.tag_id
                 WHERE ft.file_id = ?1 ORDER BY t.name COLLATE NOCASE",
            )
            .unwrap();
        let rows = stmt
            .query_map(params![file_id], |row| row.get::<_, String>(0))
            .unwrap();
        rows.map(Result::unwrap).collect()
    }

    // ----- pure extraction -----

    #[test]
    fn the_spec_decides_the_tag_field_over_the_value() {
        let spec = serde_json::json!({ "caption": "描述", "tags": "标签数组" });
        let value = serde_json::json!({ "caption": "a cat", "tags": ["cat"], "keywords": ["x"] });
        assert_eq!(tag_field(&spec, &value).as_deref(), Some("tags"));
    }

    #[test]
    fn without_a_declaration_the_first_known_value_field_wins() {
        let value = serde_json::json!({ "caption": "a", "keywords": ["k"] });
        assert_eq!(
            tag_field(&serde_json::json!({}), &value).as_deref(),
            Some("keywords")
        );
    }

    #[test]
    fn an_unknown_shape_has_no_tag_field() {
        let value = serde_json::json!({ "caption": "a", "objects": ["x"] });
        assert_eq!(
            tag_field(&serde_json::json!({ "caption": "" }), &value),
            None
        );
        assert!(tags_from_result(&value, &serde_json::json!({})).is_empty());
    }

    #[test]
    fn tags_are_cleaned_deduplicated_and_capped() {
        let value = serde_json::json!({
            "tags": ["  葡萄 ", "水果", "水果", "苹果", "bad,name", "line|pipe", "", 7]
        });
        let tags = tags_from_result(&value, &serde_json::json!({ "tags": "" }));
        assert_eq!(tags, vec!["葡萄", "水果", "苹果"]);

        let many: Vec<String> = (0..80).map(|i| format!("t{i}")).collect();
        let value = serde_json::json!({ "tags": many });
        assert_eq!(
            tags_from_result(&value, &serde_json::json!({})).len(),
            VL_TAG_LIMIT
        );
    }

    #[test]
    fn internal_whitespace_is_collapsed_not_rejected() {
        assert_eq!(normalize_tag("  new  york ").as_deref(), Some("new york"));
        assert_eq!(normalize_tag("a;b"), None);
        assert_eq!(normalize_tag("   "), None);
    }

    // ----- database writes -----

    #[test]
    fn tags_land_under_the_ai_root_and_link_the_file() {
        let conn = fixture();
        let tags = vec!["葡萄".to_string(), "水果".to_string()];
        assert_eq!(apply_tags(&conn, 1, &tags).unwrap(), 2);
        assert_eq!(names_of(&conn, 1), vec!["水果", "葡萄"]);

        let root: i64 = conn
            .query_row(
                "SELECT id FROM atags WHERE name = ?1 AND parent_id IS NULL",
                params![VL_TAG_ROOT],
                |row| row.get(0),
            )
            .unwrap();
        let parents: Vec<Option<i64>> = conn
            .prepare("SELECT parent_id FROM atags WHERE name IN ('葡萄','水果')")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(parents.iter().all(|parent| *parent == Some(root)));

        let has_tags: i64 = conn
            .query_row("SELECT has_tags FROM afiles WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(has_tags, 1);
    }

    #[test]
    fn a_rerun_replaces_only_the_ai_tags() {
        let conn = fixture();
        // A hand-written tag the user owns, outside the AI root.
        conn.execute(
            "INSERT INTO atags(id, name, parent_id) VALUES(90, '手写', NULL)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO afile_tags(file_id, tag_id) VALUES(1, 90)", [])
            .unwrap();

        apply_tags(&conn, 1, &["葡萄".to_string()]).unwrap();
        // A fresh result no longer mentions 葡萄.
        apply_tags(&conn, 1, &["苹果".to_string()]).unwrap();

        assert_eq!(names_of(&conn, 1), vec!["手写", "苹果"]);
    }

    #[test]
    fn a_tag_kept_by_the_new_result_stays_linked() {
        let conn = fixture();
        apply_tags(&conn, 1, &["葡萄".to_string(), "水果".to_string()]).unwrap();
        apply_tags(&conn, 1, &["水果".to_string(), "绿色".to_string()]).unwrap();
        assert_eq!(names_of(&conn, 1), vec!["水果", "绿色"]);
    }

    #[test]
    fn an_empty_result_leaves_the_existing_tags_alone() {
        let conn = fixture();
        apply_tags(&conn, 1, &["葡萄".to_string()]).unwrap();
        assert_eq!(apply_tags(&conn, 1, &[]).unwrap(), 0);
        assert_eq!(names_of(&conn, 1), vec!["葡萄"]);
    }

    #[test]
    fn invalid_names_are_skipped_without_aborting_the_batch() {
        let conn = fixture();
        let tags = vec!["".to_string(), "a,b".to_string(), "ok".to_string()];
        assert_eq!(apply_tags(&conn, 1, &tags).unwrap(), 1);
        assert_eq!(names_of(&conn, 1), vec!["ok"]);
    }

    #[test]
    fn two_files_share_the_same_child_tags() {
        let conn = fixture();
        apply_tags(&conn, 1, &["葡萄".to_string()]).unwrap();
        apply_tags(&conn, 2, &["葡萄".to_string()]).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM atags WHERE name = '葡萄'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(names_of(&conn, 2), vec!["葡萄"]);
    }

    #[test]
    fn applying_tags_clears_the_sidecar_tags_dirty_bit_but_keeps_other_bits() {
        let conn = fixture();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS axmp_imports(
                 file_id INTEGER PRIMARY KEY REFERENCES afiles(id) ON DELETE CASCADE,
                 fingerprint TEXT, dirty INTEGER NOT NULL DEFAULT 0);
             INSERT OR REPLACE INTO axmp_imports(file_id, dirty) VALUES(1, 8 | 2);",
        )
        .unwrap();
        apply_tags(&conn, 1, &["天空".to_string()]).unwrap();
        let dirty: i64 = conn
            .query_row(
                "SELECT dirty FROM axmp_imports WHERE file_id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            dirty, 2,
            "the Tags bit (8) must be cleared by an AI write, other bits preserved"
        );
    }

    #[test]
    fn ensure_helpers_return_the_sibling_that_appeared_concurrently() {
        let conn = fixture();
        let root = ensure_root(&conn).unwrap();
        // Simulate the losing side of the first-write race: the child exists
        // although an earlier `find` would not have seen it. The creation
        // helpers must resolve to the existing row instead of failing the
        // whole apply_tags transaction (REVIEW-2026-11-14 P1-14).
        conn.execute(
            "INSERT INTO atags(name, parent_id) VALUES('天空', ?1)",
            [root],
        )
        .unwrap();
        let child = ensure_child(&conn, root, "天空").unwrap();
        let existing: i64 = conn
            .query_row(
                "SELECT id FROM atags WHERE name = '天空' AND parent_id = ?1",
                [root],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(child, existing);
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM atags WHERE name = '天空'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "no duplicate sibling may be created");
    }
}
