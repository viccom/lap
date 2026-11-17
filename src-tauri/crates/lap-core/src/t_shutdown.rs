//! Graceful shutdown primitives (plan §3.7): cancel the background work a
//! service process owns, then flush the WAL before the process exits.
//!
//! The desktop app dies with its window and the OS reaps the threads; a
//! service (`lapd`) has to do this deliberately: a half-written thumbnail
//! cache, an orphaned ffmpeg or an unflushed WAL are exactly the states a
//! restart has to survive.
//!
//! The cancellation flags live in `t_state` (process-global), so this module
//! is the one place that knows which of them mean "stop" — the server
//! crate only asks for "cancel everything" and "flush the WAL".

use std::path::Path;
use std::sync::atomic::Ordering;

use serde::Serialize;

/// What the shutdown actually cancelled — reported to the operator so a
/// stuck background job is visible instead of silent.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CancellationReport {
    /// Indexing runs asked to stop (album ids).
    pub index_albums: Vec<i64>,
    /// Image-understanding runs asked to stop (run ids).
    pub vl_runs: Vec<String>,
    /// Video preparation processes terminated.
    pub video_processes: Vec<String>,
    /// True when an import-and-organize run was interrupted.
    pub import_interrupted: bool,
    /// True when face indexing was interrupted.
    pub face_index_interrupted: bool,
    /// True when a deduplication scan was interrupted.
    pub dedup_interrupted: bool,
    /// True when a similar-photos scan was interrupted.
    pub similar_interrupted: bool,
}

/// Raise every cancellation flag the background workers poll.
///
/// Synchronous by design: the flags are plain atomics and mutexes, and the
/// callers (index worker, import worker, face indexer, dedup/similar
/// scanners, VL run workers) observe them on their next poll.
pub fn cancel_background_tasks() -> CancellationReport {
    let mut report = CancellationReport::default();

    // Indexing: one flag per album.
    {
        let index = crate::t_state::index_cancellation();
        if let Ok(mut map) = index.0.lock() {
            report.index_albums = map.keys().copied().collect();
            for flag in map.values_mut() {
                *flag = true;
            }
            map.clear();
        }
    }

    // Import-and-organize.
    {
        let import = crate::t_state::import_cancellation();
        if let Ok(mut state) = import.0.lock() {
            report.import_interrupted = state.running;
            state.cancelled = true;
            state.running = false;
        }
    }

    // Face indexing.
    {
        let face = crate::t_state::face_index_cancellation();
        if let Ok(mut flag) = face.0.lock() {
            report.face_index_interrupted = *flag;
            *flag = true;
        }
    }

    // Deduplication scan.
    {
        let dedup = crate::t_state::dedup();
        report.dedup_interrupted = dedup.is_scanning.load(Ordering::SeqCst);
        dedup.cancel_flag.store(true, Ordering::SeqCst);
    }

    // Similar-photos scan.
    {
        let similar = crate::t_state::similar();
        report.similar_interrupted = similar.is_scanning.load(Ordering::SeqCst);
        similar.cancel_flag.store(true, Ordering::SeqCst);
    }

    // Image-understanding runs: one flag per run id.
    {
        let vl = crate::t_state::vl();
        if let Ok(flags) = vl.cancel_flags.lock() {
            report.vl_runs = flags.keys().cloned().collect();
            for flag in flags.values() {
                flag.store(true, Ordering::SeqCst);
            }
        }
    }

    report
}

/// Cancel the background work and terminate the video preparation
/// processes (ffmpeg children) that would otherwise survive the parent.
pub async fn terminate_background_tasks() -> CancellationReport {
    let mut report = cancel_background_tasks();

    // Video preparation children: kill them so no ffmpeg keeps writing into
    // the cache directory while we flush the databases.
    let manager = crate::t_state::video_manager();
    let mut processes = manager.active_processes.lock().await;
    for (player_id, (_request_id, child)) in processes.iter_mut() {
        let _ = child.kill().await;
        report.video_processes.push(player_id.clone());
    }
    processes.clear();
    drop(processes);

    let mut requests = manager.active_requests.lock().await;
    requests.clear();

    report
}

/// Whether the known background work has wound down. Used to wait a bounded
/// moment before flushing, so the checkpoint does not race a writer.
pub fn background_tasks_idle() -> bool {
    if crate::t_sqlite::has_active_thumb_background_tasks() {
        return false;
    }
    let dedup = crate::t_state::dedup();
    if dedup.is_scanning.load(Ordering::SeqCst) {
        return false;
    }
    let similar = crate::t_state::similar();
    if similar.is_scanning.load(Ordering::SeqCst) {
        return false;
    }
    let vl = crate::t_state::vl();
    if let Ok(flags) = vl.cancel_flags.lock() {
        // A run still holding a cancel flag may still be winding down.
        if !flags.is_empty() {
            return false;
        }
    }
    true
}

/// Checkpoint every database the service owns so the WAL is folded into the
/// main database file before the process exits.
///
/// `data_dir` is the lapd data root: the service database (`lapd.db`) lives
/// there, the libraries live in the configured storage directory.
///
/// Returns the warnings instead of failing the exit: a busy checkpoint is
/// worth reporting but must not keep the server from exiting.
pub fn flush_wal(data_dir: &Path) -> Vec<String> {
    let mut warnings = Vec::new();

    // The service database (device tokens, audit log).
    let service_db = data_dir.join("lapd.db");
    if service_db.exists() {
        if let Err(e) = crate::t_storage::checkpoint_db(&service_db) {
            warnings.push(format!("service database checkpoint failed: {e}"));
        }
    }

    // Every configured library database.
    match crate::t_config::load_app_config() {
        Ok(config) => {
            for library in &config.libraries {
                match crate::t_storage::get_library_db_path(&library.id) {
                    Ok(path) => {
                        let path = Path::new(&path);
                        if path.exists() {
                            if let Err(e) = crate::t_storage::checkpoint_db(path) {
                                warnings.push(format!(
                                    "library {} checkpoint failed: {e}",
                                    library.id
                                ));
                            }
                        }
                    }
                    Err(e) => warnings.push(format!("library {} path failed: {e}", library.id)),
                }
            }
        }
        Err(e) => warnings.push(format!("reading app config for checkpoint failed: {e}")),
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::t_state;

    #[test]
    fn cancelling_raises_every_background_flag() {
        // The cancellation flags are process-global, so the assertions live
        // in one test: parallel tests would race each other through the
        // shared state.
        t_state::init_process_states();

        let index = t_state::index_cancellation();
        index.0.lock().unwrap().insert(7, false);
        index.0.lock().unwrap().insert(9, false);
        {
            let import = t_state::import_cancellation();
            let mut state = import.0.lock().unwrap();
            state.running = true;
            state.cancelled = false;
        }
        t_state::dedup().is_scanning.store(true, Ordering::SeqCst);
        t_state::similar().is_scanning.store(true, Ordering::SeqCst);
        let vl = t_state::vl();
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        vl.cancel_flags.lock().unwrap().insert("run-1".to_string(), flag.clone());

        let report = cancel_background_tasks();

        assert!(report.index_albums.contains(&7));
        assert!(report.index_albums.contains(&9));
        assert!(report.import_interrupted);
        assert!(report.dedup_interrupted);
        assert!(report.similar_interrupted);
        assert!(report.vl_runs.contains(&"run-1".to_string()));

        // The flags are cleared so a restarted scan is not born cancelled.
        assert!(index.0.lock().unwrap().is_empty());
        assert!(t_state::import_cancellation().0.lock().unwrap().cancelled);
        assert!(t_state::dedup().cancel_flag.load(Ordering::SeqCst));
        assert!(t_state::similar().cancel_flag.load(Ordering::SeqCst));
        assert!(flag.load(Ordering::SeqCst));

        // Leave the shared state clean for the other tests.
        t_state::dedup().is_scanning.store(false, Ordering::SeqCst);
        t_state::dedup().cancel_flag.store(false, Ordering::SeqCst);
        t_state::similar().is_scanning.store(false, Ordering::SeqCst);
        t_state::similar().cancel_flag.store(false, Ordering::SeqCst);
        t_state::import_cancellation().0.lock().unwrap().running = false;
        vl.cancel_flags.lock().unwrap().clear();
    }

    #[test]
    fn flushing_a_missing_database_directory_is_harmless() {
        let dir = std::env::temp_dir().join(format!("lapd-flush-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let warnings = flush_wal(&dir);
        // The property under test is that a directory with no `lapd.db` is safe
        // to flush. That is asserted on OUR directory: any warning naming it
        // would mean flush_wal invented work for a path that does not exist.
        //
        // We deliberately do NOT assert "no warnings at all": `flush_wal` also
        // walks the libraries of the process-global app config, which on a
        // developer machine is the real user config — and `get_library_db_path`
        // legitimately errors while a sibling test holds the process-global
        // migration guard (`DB_MIGRATION_IN_PROGRESS`). Asserting global
        // emptiness made this test fail on any machine with a real library and
        // race the migration test; both are outside what this test is about.
        let dir_text = dir.to_string_lossy().to_string();
        assert!(
            !warnings.iter().any(|w| w.contains(&dir_text)),
            "flush_wal reported trouble for the empty directory itself: {warnings:?}",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn flushing_folds_the_wal_into_the_main_database() {
        // A real WAL database: write without checkpointing, then flush and
        // verify the -wal file is gone and the data survived.
        let dir = std::env::temp_dir().join(format!("lapd-flush-wal-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("probe.db");
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
            .unwrap();
        conn.execute("INSERT INTO t (v) VALUES ('hello')", []).unwrap();
        let wal_path = dir.join("probe.db-wal");
        assert!(wal_path.exists(), "WAL mode must leave a -wal file behind");

        crate::t_storage::checkpoint_db(&db_path).unwrap();

        // The checkpoint truncates the WAL (or folds it away entirely).
        let wal_size = std::fs::metadata(&wal_path).map(|m| m.len()).unwrap_or(0);
        assert_eq!(wal_size, 0, "the WAL must be folded into the db file");
        // The data survived the checkpoint.
        let value: String = conn
            .query_row("SELECT v FROM t WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "hello");
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
