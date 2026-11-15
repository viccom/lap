//! Queued image-understanding tasks: the scheduled counterpart of a manual
//! run.
//!
//! A task is exactly what the user picked in the browser — a file set plus a
//! template — with the local time it should fire at ("2026-11-16 02:30",
//! single shot). Nothing is recomputed from the library: a 50k library stays
//! untouched unless photos were explicitly selected. Tasks live in each
//! library database (v28, `vl_tasks`); the scheduler moves them through:
//! - `pending` → `fired` (a run exists, tagged `source = 'scheduled'`)
//! - `pending` → `expired` (the date passed without the app running)
//! - `pending` → `failed` (the fire itself failed, e.g. gateway not set up)
//! The scheduler only runs while the app lives; no OS-level wake-up.
use chrono::Timelike;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use crate::t_config::load_app_config;
use crate::t_vl::{start_run_for_library, LibraryContext, VlState, VlStore};

/// How often the scheduler looks at the clock. A task missed while the app
/// was closed still fires the same day (a `once` task expires at midnight).
const TICK_SECS: u64 = 30;

/// One queued task as the browser created it and the UI manages it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlTaskView {
    pub id: String,
    pub library_id: String,
    pub library_name: String,
    pub template_id: i64,
    pub template_name: String,
    /// How many files the task carries (the ids themselves stay in the db).
    pub file_count: usize,
    pub force: bool,
    /// Local fire time, "HH:MM".
    pub time: String,
    /// Fire date, "YYYY-MM-DD".
    pub date: String,
    /// pending / fired / expired / failed.
    pub state: String,
    /// The run a fired task produced.
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: i64,
}

/// Parse "HH:MM" into minutes since local midnight.
pub fn parse_hhmm(value: &str) -> Option<u32> {
    let (hours, minutes) = value.split_once(':')?;
    let hours: u32 = hours.trim().parse().ok()?;
    let minutes: u32 = minutes.trim().parse().ok()?;
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/// A pending task fires once its date is today and its time has come; a task
/// whose date lies in the past can never fire anymore.
pub fn task_is_due(time: &str, date: &str, now_date: &str, now_minutes: u32) -> bool {
    date == now_date && parse_hhmm(time).is_some_and(|fire| now_minutes >= fire)
}

pub fn task_is_expired(date: &str, now_date: &str) -> bool {
    date < now_date
}

// ----------------------------------------------------------------------------
// Scheduler
// ----------------------------------------------------------------------------

/// Start the background clock. One task per app; it dies with the runtime.
pub fn spawn_scheduler(app_handle: tauri::AppHandle) {
    let state: tauri::State<'_, VlState> = app_handle.state();
    let state = state.inner().clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(TICK_SECS)).await;
            if let Err(e) = run_due_tasks(&app_handle, &state) {
                eprintln!("VL task scheduler tick failed: {e}");
            }
        }
    });
}

/// Fire and expire every due task across the configured libraries.
fn run_due_tasks(app_handle: &tauri::AppHandle, state: &VlState) -> Result<(), String> {
    let now = chrono::Local::now();
    let now_date = now.format("%Y-%m-%d").to_string();
    let now_minutes = now.hour() * 60 + now.minute();
    let config = load_app_config()?;
    let mut changed = false;
    for library in &config.libraries {
        let Ok(db_path) = crate::t_storage::get_library_db_path(&library.id) else {
            continue;
        };
        if !std::path::Path::new(&db_path).exists() {
            continue;
        }
        let Ok(conn) = crate::t_sqlite::open_readwrite_at(&db_path) else {
            continue;
        };
        // A library from before v28 has no task table yet.
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if version < 28 {
            if let Err(e) = crate::t_migration::check_and_migrate(&conn) {
                eprintln!("VL task scheduler: migrating {} failed: {e}", library.id);
                continue;
            }
        }
        changed |= expire_past_tasks(&conn, &now_date);
        let context = LibraryContext {
            library_id: library.id.clone(),
            library_name: library.name.clone(),
            db_path: db_path.clone(),
        };
        for (task_id, template_id, file_ids, force, time) in pending_tasks(&conn)? {
            if !task_is_due(&time, &pending_date(&conn, &task_id)?, &now_date, now_minutes) {
                continue;
            }
            let outcome = fire_task(app_handle, state, &context, template_id, &file_ids, force);
            match outcome {
                Ok(run_id) => {
                    eprintln!("VL task {task_id} fired run {run_id}");
                    mark_task(&conn, &task_id, "fired", Some(run_id), None);
                }
                Err(e) => {
                    eprintln!("VL task {task_id} failed: {e}");
                    mark_task(&conn, &task_id, "failed", None, Some(e));
                }
            }
            changed = true;
        }
    }
    if changed {
        let _ = app_handle.emit("vl-tasks-changed", ());
    }
    Ok(())
}

/// `pending` rows whose date is over become `expired` (the app was closed on
/// their day). Returns whether anything changed.
fn expire_past_tasks(conn: &rusqlite::Connection, now_date: &str) -> bool {
    conn.execute(
        "UPDATE vl_tasks SET state = 'expired', error = '未在计划日期执行'
         WHERE state = 'pending' AND date < ?1",
        [now_date],
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// The pending tasks of a library, oldest first.
fn pending_tasks(
    conn: &rusqlite::Connection,
) -> Result<Vec<(String, i64, Vec<i64>, bool, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, template_id, file_ids, force, date, time FROM vl_tasks
                  WHERE state = 'pending' ORDER BY date, time, created_at")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut tasks = Vec::new();
    for row in rows {
        let (id, template_id, file_ids_json, force, date, time) =
            row.map_err(|e| e.to_string())?;
        let file_ids: Vec<i64> = serde_json::from_str(&file_ids_json)
            .map_err(|e| format!("task {id} carries a broken file list: {e}"))?;
        // The date travels with the row; due checks read it from here.
        let _ = date;
        tasks.push((id, template_id, file_ids, force, time));
    }
    Ok(tasks)
}

/// The date of a pending task (kept separate so `pending_tasks` stays flat).
fn pending_date(conn: &rusqlite::Connection, task_id: &str) -> Result<String, String> {
    conn.query_row(
        "SELECT date FROM vl_tasks WHERE id = ?1",
        [task_id],
        |row| row.get(0),
    )
    .map_err(|e| e.to_string())
}

/// Move a task to a terminal state, remembering the run or the error.
fn mark_task(
    conn: &rusqlite::Connection,
    task_id: &str,
    state: &str,
    run_id: Option<String>,
    error: Option<String>,
) {
    let _ = conn.execute(
        "UPDATE vl_tasks SET state = ?2, run_id = ?3, error = ?4 WHERE id = ?1",
        rusqlite::params![task_id, state, run_id, error],
    );
}

/// Start the run of one due task. Files that left the library since queueing
/// drop out silently; an empty remainder fails the task instead of the run.
fn fire_task(
    app_handle: &tauri::AppHandle,
    state: &VlState,
    context: &LibraryContext,
    template_id: i64,
    file_ids: &[i64],
    force: bool,
) -> Result<String, String> {
    let conn = crate::t_sqlite::open_conn_for_library(&context.db_path)?;
    let live: std::collections::HashSet<i64> = conn
        .prepare("SELECT id FROM afiles")
        .map_err(|e| e.to_string())?
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|e| e.to_string())?
        .filter_map(|id| id.ok())
        .collect();
    let files: Vec<i64> = file_ids
        .iter()
        .copied()
        .filter(|id| live.contains(id))
        .collect();
    if files.is_empty() {
        return Err("任务中的文件已不在图库中".to_string());
    }
    start_run_for_library(
        app_handle,
        state,
        context,
        files,
        template_id,
        force,
        "scheduled",
    )
}

// ----------------------------------------------------------------------------
// Commands
// ----------------------------------------------------------------------------

/// Queue a task: the picked files plus the template, firing at `date time`.
#[tauri::command]
pub fn queue_vl_task(
    file_ids: Vec<i64>,
    template_id: i64,
    force: bool,
    time: String,
    date: String,
) -> Result<VlTaskView, String> {
    if file_ids.is_empty() {
        return Err("未选择要理解的文件".to_string());
    }
    if parse_hhmm(&time).is_none() {
        return Err("时间格式应为 HH:MM".to_string());
    }
    if date.len() != 10 || !date.contains('-') {
        return Err("日期格式应为 YYYY-MM-DD".to_string());
    }
    let library = crate::t_vl::current_library_context()?;
    let conn = crate::t_sqlite::open_conn_for_library(&library.db_path)?;
    crate::t_vl::validate_run_files(&conn, &file_ids)?;
    let store = crate::t_vl::LibraryBoundStore {
        db_path: library.db_path.clone(),
    };
    let template = store
        .template(template_id)?
        .ok_or_else(|| "模板不存在".to_string())?;
    if !template.enabled {
        return Err("模板已停用".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let file_ids_json = serde_json::to_string(&file_ids).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO vl_tasks(id, template_id, file_ids, force, time, date, state, created_at)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)",
        rusqlite::params![id, template_id, file_ids_json, i64::from(force), time, date, now_ts()],
    )
    .map_err(|e| e.to_string())?;
    Ok(VlTaskView {
        id,
        library_id: library.library_id,
        library_name: library.library_name,
        template_id,
        template_name: template.name,
        file_count: file_ids.len(),
        force,
        time,
        date,
        state: "pending".to_string(),
        run_id: None,
        error: None,
        created_at: now_ts(),
    })
}

/// The queued tasks of every configured library, newest first. Pending tasks
/// whose date passed are lazily expired on the way out.
#[tauri::command]
pub fn list_vl_tasks() -> Result<Vec<VlTaskView>, String> {
    let now_date = chrono::Local::now().format("%Y-%m-%d").to_string();
    let config = load_app_config()?;
    let mut tasks = Vec::new();
    for library in &config.libraries {
        let Ok(db_path) = crate::t_storage::get_library_db_path(&library.id) else {
            continue;
        };
        if !std::path::Path::new(&db_path).exists() {
            continue;
        }
        let Ok(conn) = crate::t_sqlite::open_readwrite_at(&db_path) else {
            continue;
        };
        let version: i32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap_or(0);
        if version < 28 {
            if let Err(e) = crate::t_migration::check_and_migrate(&conn) {
                eprintln!("VL task list: migrating {} failed: {e}", library.id);
                continue;
            }
        }
        expire_past_tasks(&conn, &now_date);
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, template_id, file_ids, force, time, date, state, run_id, error, created_at
             FROM vl_tasks ORDER BY created_at DESC",
        ) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)? != 0,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, i64>(9)?,
            ))
        }) else {
            continue;
        };
        for row in rows {
            let (id, template_id, file_ids_json, force, time, date, state, run_id, error, created_at) =
                row.map_err(|e| e.to_string())?;
            let file_count: usize = serde_json::from_str::<Vec<i64>>(&file_ids_json)
                .map(|ids| ids.len())
                .unwrap_or(0);
            let template_name = crate::t_vl::LibraryBoundStore {
                db_path: db_path.clone(),
            }
            .template(template_id)
            .ok()
            .flatten()
            .map(|template| template.name)
            .unwrap_or_else(|| format!("#{template_id}"));
            tasks.push(VlTaskView {
                id,
                library_id: library.id.clone(),
                library_name: library.name.clone(),
                template_id,
                template_name,
                file_count,
                force,
                time,
                date,
                state,
                run_id,
                error,
                created_at,
            });
        }
    }
    Ok(tasks)
}

/// Reschedule a pending/expired/failed task to a new date and time. Editing
/// revives the task: it goes back to `pending` with a clean error.
#[tauri::command]
pub fn update_vl_task_time(
    id: String,
    time: String,
    date: String,
) -> Result<(), String> {
    if parse_hhmm(&time).is_none() {
        return Err("时间格式应为 HH:MM".to_string());
    }
    if date.len() != 10 || !date.contains('-') {
        return Err("日期格式应为 YYYY-MM-DD".to_string());
    }
    let Some(db_path) = locate_task_db(&id) else {
        return Err(format!("未找到任务 {id}"));
    };
    let conn = crate::t_sqlite::open_readwrite_at(&db_path)?;
    let updated = conn
        .execute(
            "UPDATE vl_tasks SET time = ?2, date = ?3, state = 'pending',
                    run_id = NULL, error = NULL
             WHERE id = ?1 AND state != 'fired'",
            rusqlite::params![id, time, date],
        )
        .map_err(|e| e.to_string())?;
    if updated == 0 {
        return Err("任务已执行，不能改期".to_string());
    }
    Ok(())
}

/// Remove a task from the queue (any state except a live run it may own).
#[tauri::command]
pub fn delete_vl_task(id: String) -> Result<(), String> {
    let Some(db_path) = locate_task_db(&id) else {
        return Err(format!("未找到任务 {id}"));
    };
    let conn = crate::t_sqlite::open_readwrite_at(&db_path)?;
    conn.execute("DELETE FROM vl_tasks WHERE id = ?1", [id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The database of the library holding `task_id`.
fn locate_task_db(task_id: &str) -> Option<String> {
    let config = load_app_config().ok()?;
    for library in &config.libraries {
        let Ok(db_path) = crate::t_storage::get_library_db_path(&library.id) else {
            continue;
        };
        if !std::path::Path::new(&db_path).exists() {
            continue;
        }
        let Ok(conn) = crate::t_sqlite::open_readwrite_at(&db_path) else {
            continue;
        };
        let owned = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM vl_tasks WHERE id = ?1)",
            [task_id],
            |row| row.get::<_, bool>(0),
        );
        if matches!(owned, Ok(true)) {
            return Some(db_path);
        }
    }
    None
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hhmm_parsing_rejects_out_of_range_values() {
        assert_eq!(parse_hhmm("02:30"), Some(150));
        assert_eq!(parse_hhmm("23:59"), Some(23 * 60 + 59));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("02:60"), None);
        assert_eq!(parse_hhmm("0230"), None);
        assert_eq!(parse_hhmm(""), None);
    }

    #[test]
    fn a_task_is_due_only_on_its_date_from_its_time() {
        // Before the time of its day: not yet.
        assert!(!task_is_due("02:30", "2026-11-16", "2026-11-16", 149));
        // At and after the time, same day: due (late starts count too).
        assert!(task_is_due("02:30", "2026-11-16", "2026-11-16", 150));
        assert!(task_is_due("02:30", "2026-11-16", "2026-11-16", 22 * 60));
        // Another day is never due; a malformed time never fires.
        assert!(!task_is_due("02:30", "2026-11-16", "2026-11-17", 150));
        assert!(!task_is_due("99:00", "2026-11-16", "2026-11-16", 150));
    }

    #[test]
    fn a_task_expires_when_its_date_passes() {
        assert!(task_is_expired("2026-11-15", "2026-11-16"));
        assert!(!task_is_expired("2026-11-16", "2026-11-16"));
        assert!(!task_is_expired("2026-11-17", "2026-11-16"));
    }

    #[test]
    fn pending_tasks_expire_and_editing_revives_them() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::t_migration::ensure_tasks_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO vl_tasks(id, template_id, file_ids, force, time, date, state, created_at)
             VALUES('t1', 1, '[1,2]', 0, '02:30', '2026-11-14', 'pending', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO vl_tasks(id, template_id, file_ids, force, time, date, state, created_at)
             VALUES('t2', 1, '[3]', 0, '02:30', '2026-11-16', 'pending', 2)",
            [],
        )
        .unwrap();
        assert!(expire_past_tasks(&conn, "2026-11-15"));
        let states: Vec<(String, String)> = conn
            .prepare("SELECT id, state FROM vl_tasks ORDER BY id")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            states,
            vec![
                ("t1".to_string(), "expired".to_string()),
                ("t2".to_string(), "pending".to_string())
            ]
        );
        // Editing moves a task back to pending on the new date.
        conn.execute(
            "UPDATE vl_tasks SET time = ?2, date = ?3, state = 'pending', run_id = NULL, error = NULL
             WHERE id = ?1 AND state != 'fired'",
            rusqlite::params!["t1", "03:00", "2026-11-17"],
        )
        .unwrap();
        assert_eq!(pending_date(&conn, "t1").unwrap(), "2026-11-17");
        // The file list survives as JSON.
        let tasks = pending_tasks(&conn).unwrap();
        assert_eq!(tasks.len(), 2);
        assert!(tasks.iter().any(|(id, _, files, _, _)| id == "t1" && files == &vec![1, 2]));
    }
}
