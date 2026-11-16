//! Process-level state registry (serverization plan §3.2).
//!
//! Tauri injects `State<T>` into command handlers, which ties those handlers
//! to the desktop shell. The state values themselves are plain process state,
//! so the core keeps them here and reads them directly; the shell no longer
//! has to manage them.
//!
//! Each entry is a `OnceLock` holding the value for the life of the process,
//! mirroring the existing pattern in `t_config` and `t_events`. Commands that
//! still take `State<T>` for values registered elsewhere keep working; a value
//! must never live in both places, or the two copies would drift.

use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

/// Per-album cancellation flags for indexing: `true` means "stop".
///
/// Shared by `index_album` (which clears the flag), `cancel_indexing` (which
/// sets it) and the album mutation commands (which set it to wait for a
/// running scan to notice), so all four must read this one instance.
pub struct IndexCancellation(pub Arc<Mutex<HashMap<i64, bool>>>);

static INDEX_CANCELLATION: OnceLock<IndexCancellation> = OnceLock::new();

/// The process-wide index cancellation state.
pub fn index_cancellation() -> &'static IndexCancellation {
    INDEX_CANCELLATION.get_or_init(|| IndexCancellation(Arc::new(Mutex::new(HashMap::new()))))
}

/// Startup timing and AI warm-up progress, moved out of the desktop shell's
/// `t_startup`: the AI engine and the background indexing pipeline need the
/// same status the startup screen reads, so this is process state rather than
/// something behind a Tauri handle.
pub struct StartupState {
    pub started: Instant,
    pub stages: Mutex<HashSet<String>>,
    pub background_started: AtomicBool,
    pub ai_status: Mutex<String>,
}

impl Default for StartupState {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            stages: Mutex::new(HashSet::new()),
            background_started: AtomicBool::new(false),
            ai_status: Mutex::new("not-initialized".into()),
        }
    }
}

impl StartupState {
    /// Record a one-time startup stage; the first insertion logs the time it
    /// took to reach it.
    pub fn mark(&self, stage: &str) {
        if let Ok(mut stages) = self.stages.lock() {
            if stages.insert(stage.into()) {
                println!("[startup] {}: {} ms", stage, self.started.elapsed().as_millis());
            }
        }
    }
}

static STARTUP: OnceLock<StartupState> = OnceLock::new();

/// The process-wide startup/AI-warm-up state.
pub fn startup() -> &'static StartupState {
    STARTUP.get_or_init(StartupState::default)
}

static AI: OnceLock<crate::t_ai::AiState> = OnceLock::new();

/// The process-wide AI engine. Its mutex shares initialization between early
/// searches, indexing and warm-up so the models load exactly once.
pub fn ai() -> &'static crate::t_ai::AiState {
    AI.get_or_init(|| crate::t_ai::AiState(Mutex::new(crate::t_ai::AiEngine::new())))
}

/// The shared face engine (`Arc<Mutex<FaceEngine>>`): one instance for the
/// index command and every face query that loads models through it.
static FACE: OnceLock<crate::t_face::FaceState> = OnceLock::new();

pub fn face() -> &'static crate::t_face::FaceState {
    FACE.get_or_init(|| {
        crate::t_face::FaceState(Arc::new(Mutex::new(crate::t_face::FaceEngine::new())))
    })
}

/// Face indexing cancellation flag: `true` means "stop".
static FACE_INDEX_CANCELLATION: OnceLock<crate::t_face::FaceIndexCancellation> = OnceLock::new();

pub fn face_index_cancellation() -> &'static crate::t_face::FaceIndexCancellation {
    FACE_INDEX_CANCELLATION
        .get_or_init(|| crate::t_face::FaceIndexCancellation(Arc::new(Mutex::new(false))))
}

/// Face indexing running flag; storage migration refuses to run while it is
/// set, so both sides must read one instance.
static FACE_INDEXING_STATUS: OnceLock<crate::t_face::FaceIndexingStatus> = OnceLock::new();

pub fn face_indexing_status() -> &'static crate::t_face::FaceIndexingStatus {
    FACE_INDEXING_STATUS.get_or_init(|| crate::t_face::FaceIndexingStatus(Arc::new(Mutex::new(false))))
}

/// Face indexing progress snapshot the status command reads while a scan runs.
static FACE_INDEX_PROGRESS: OnceLock<crate::t_face::FaceIndexProgressState> = OnceLock::new();

pub fn face_index_progress() -> &'static crate::t_face::FaceIndexProgressState {
    FACE_INDEX_PROGRESS.get_or_init(|| {
        crate::t_face::FaceIndexProgressState(Arc::new(Mutex::new(
            crate::t_face::FaceIndexProgress {
                current: 0,
                total: 0,
                faces_found: 0,
                phase: "indexing".to_string(),
            },
        )))
    })
}

/// Import-and-organize state: `running` guards re-entry, `cancelled` is the
/// stop flag the worker polls.
pub struct ImportState {
    pub cancelled: bool,
    pub running: bool,
}

impl Default for ImportState {
    fn default() -> Self {
        Self {
            cancelled: false,
            running: false,
        }
    }
}

pub struct ImportCancellation(pub Arc<Mutex<ImportState>>);

static IMPORT_CANCELLATION: OnceLock<ImportCancellation> = OnceLock::new();

pub fn import_cancellation() -> &'static ImportCancellation {
    IMPORT_CANCELLATION.get_or_init(|| ImportCancellation(Arc::new(Mutex::new(ImportState::default()))))
}

/// The dedup scan runner: one instance so `dedup_start_scan`'s CAS re-entry
/// guard, the status command and the cancel command all see the same scan.
static DEDUP: OnceLock<crate::t_dedup::DedupState> = OnceLock::new();

pub fn dedup() -> &'static crate::t_dedup::DedupState {
    DEDUP.get_or_init(crate::t_dedup::DedupState::default)
}

/// Same pattern as [`dedup`] for the similar-photo scan.
static SIMILAR: OnceLock<crate::t_similar::SimilarState> = OnceLock::new();

pub fn similar() -> &'static crate::t_similar::SimilarState {
    SIMILAR.get_or_init(crate::t_similar::SimilarState::default)
}

/// Video prepare/preview bookkeeping: live ffmpeg children, per-player
/// request ids and the task counter shared by the three video commands.
static VIDEO_MANAGER: OnceLock<crate::t_video::VideoManager> = OnceLock::new();

pub fn video_manager() -> &'static crate::t_video::VideoManager {
    VIDEO_MANAGER.get_or_init(crate::t_video::VideoManager::default)
}

/// The VL analysis registry: run statuses, cancel flags, the queue and the
/// global LLM call permits. Shared by the VL commands, the run workers and
/// the v28 task scheduler, and read by `remove_library` to refuse deleting a
/// library with a live run.
static VL: OnceLock<crate::t_vl::VlState> = OnceLock::new();

pub fn vl() -> &'static crate::t_vl::VlState {
    VL.get_or_init(crate::t_vl::VlState::default)
}

/// Initialize every registered process state up front. The host entry point
/// calls this first so time-anchored state (like `StartupState::started`)
/// reflects process start rather than the first late reader; library-only
/// callers (tests) may skip it and rely on the lazy accessors.
pub fn init_process_states() {
    let _ = startup();
    let _ = ai();
    let _ = face();
    let _ = face_index_cancellation();
    let _ = face_indexing_status();
    let _ = face_index_progress();
    let _ = import_cancellation();
    let _ = dedup();
    let _ = similar();
    let _ = video_manager();
    let _ = vl();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cancellation_state_is_shared_across_callers() {
        // Two readers must see the same flags, otherwise a cancellation set by
        // one command would be invisible to the scan it is meant to stop.
        let first = index_cancellation();
        let second = index_cancellation();
        assert!(std::ptr::eq(first, second));

        first.0.lock().unwrap().insert(42, true);
        assert_eq!(*second.0.lock().unwrap().get(&42).unwrap(), true);
        second.0.lock().unwrap().remove(&42);
    }

    #[test]
    fn the_cancel_command_writes_to_the_registry() {
        // The command and the scan worker must share one token; if the command
        // wrote to a different copy, cancelling an index would silently do
        // nothing.
        crate::t_cmds::cancel_indexing(4242).expect("cancel_indexing must succeed");
        assert_eq!(
            *index_cancellation().0.lock().unwrap().get(&4242).unwrap(),
            true
        );
        index_cancellation().0.lock().unwrap().remove(&4242);
    }

    #[test]
    fn startup_and_ai_states_are_shared_across_callers() {
        // ensure_ai marks stages on one StartupState while the startup screen
        // reads another copy would show "not-initialized" forever.
        assert!(std::ptr::eq(startup(), startup()));
        assert!(std::ptr::eq(ai(), ai()));
    }

    #[test]
    fn background_warmup_runs_once_across_window_remounts() {
        let state = StartupState::default();
        assert!(!state.background_started.swap(true, Ordering::SeqCst));
        assert!(state.background_started.swap(true, Ordering::SeqCst));
    }

    #[test]
    fn face_status_is_shared_between_the_index_and_migration_guard() {
        // The storage-migration guard refuses to run while face indexing is
        // active; that only works if both read the same flag instance.
        assert!(std::ptr::eq(face_indexing_status(), face_indexing_status()));
        assert!(!*face_indexing_status().0.lock().unwrap());
    }

    #[test]
    fn a_stage_is_marked_once_and_logs_once() {
        let state = StartupState::default();
        state.mark("registry-test-stage");
        state.mark("registry-test-stage");
        let stages = state.stages.lock().unwrap();
        assert!(stages.contains("registry-test-stage"));
        assert_eq!(stages.len(), 1);
    }
}
