//! Process-level event sink (serverization plan §3.2).
//!
//! The core modules must not know about Tauri. Today the only thing they need
//! from it is emitting progress events to the frontend, so an `EventSink`
//! trait stands in for `Emitter::emit`: the desktop shell installs one that
//! forwards to Tauri, and a future `lapd` will install one that fans out over
//! SSE. Call sites keep their current "fire and forget" semantics — emission
//! never fails observably (the old code was `let _ = app.emit(...)`).
//!
//! The sink is a `OnceLock`, matching the existing process-global pattern
//! (`t_config::APP_IDENTIFIER`, `t_video::SIDE_CAR_DIR`): set once during
//! startup, read everywhere. Until it is set, emissions go nowhere, so a
//! module can be unit-tested without a Tauri runtime.

use std::sync::{Arc, OnceLock};

use serde::Serialize;

/// Something that can deliver a named event with a JSON payload.
pub trait EventSink: Send + Sync {
    fn emit(&self, name: &str, payload: serde_json::Value);
}

/// Drops every event. Used before the shell installs the real sink and in tests.
struct NoopSink;

impl EventSink for NoopSink {
    fn emit(&self, _name: &str, _payload: serde_json::Value) {}
}

static SINK: OnceLock<Arc<dyn EventSink>> = OnceLock::new();

/// Install the process-wide sink. Called once from the desktop `setup()`; a
/// later call is ignored (first writer wins) so startup ordering stays simple.
pub fn set_sink(sink: Arc<dyn EventSink>) {
    let _ = SINK.set(sink);
}

/// The installed sink, or a no-op one before startup installs the real one.
pub fn sink() -> &'static Arc<dyn EventSink> {
    SINK.get_or_init(|| Arc::new(NoopSink))
}

/// Emit a typed payload, serializing it to JSON first.
///
/// Payloads are three shapes at the call sites — `()`, a `Serialize` struct
/// (by ref or by value), and inline `serde_json::json!` — all covered here.
/// A serialization failure degenerates to JSON `null` rather than panicking,
/// consistent with the old emission that ignored its result.
pub fn emit<T: Serialize>(name: &str, payload: T) {
    let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
    sink().emit(name, value);
}

/// Event names, kept verbatim from the historical inline literals so the
/// frontend `listen(...)` registrations keep matching. Grouping them here is
/// the first step of §3.2's "23 event names" inventory.
pub mod names {
    pub const NATIVE_FILE_DRAG_ACTIVE: &str = "native-file-drag-active";
    pub const DEDUP_SCAN_PROGRESS: &str = "dedup-scan-progress";
    pub const IMAGE_SEARCH_MODEL_DOWNLOAD_PROGRESS: &str =
        "image_search_model_download_progress";
    pub const FACE_INDEX_FINISHED: &str = "face_index_finished";
    pub const FACE_INDEX_PROGRESS: &str = "face_index_progress";
    pub const CLUSTER_PROGRESS: &str = "cluster_progress";
    pub const ALBUMS_REFRESHED: &str = "albums-refreshed";
    pub const THUMBNAIL_READY: &str = "thumbnail_ready";
    pub const VL_TASKS_CHANGED: &str = "vl-tasks-changed";
    pub const ALBUM_FOLDER_PATHS_MIGRATED: &str = "album-folder-paths-migrated";
    pub const IMPORT_ORGANIZE_PROGRESS: &str = "import-organize-progress";
    pub const IMPORT_ORGANIZE_FINISHED: &str = "import-organize-finished";
    pub const SIMILAR_SCAN_PROGRESS: &str = "similar-scan-progress";
    pub const LIBRARY_FOLDER_SYNC_FINISHED: &str = "library-folder-sync-finished";
    pub const LIBRARY_TOTAL_REFRESHED: &str = "library-total-refreshed";
    pub const INDEX_PROGRESS: &str = "index_progress";
    pub const INDEX_FINISHED: &str = "index_finished";
    pub const VL_ANALYSIS_PROGRESS: &str = "vl-analysis-progress";
    pub const TAGS_CHANGED: &str = "tags-changed";
    pub const VL_ANALYSIS_FINISHED: &str = "vl-analysis-finished";
    // The three menu events (menu-select-all / app-open-about /
    // app-open-preferences) are emitted on a WebviewWindow by the shell
    // (t_menu.rs) and never travel through this sink, so they are not listed.
}

/// A sink that records emissions for assertions. Test-only.
#[cfg(test)]
pub mod testing {
    use super::EventSink;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct RecordingSink {
        events: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl RecordingSink {
        pub fn events(&self) -> Vec<(String, serde_json::Value)> {
            self.events.lock().unwrap().clone()
        }

        pub fn names(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap()
                .iter()
                .map(|(name, _)| name.clone())
                .collect()
        }
    }

    impl EventSink for RecordingSink {
        fn emit(&self, name: &str, payload: serde_json::Value) {
            self.events
                .lock()
                .unwrap()
                .push((name.to_string(), payload));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_serializes_typed_and_unit_payloads() {
        // The sink is process-global and set-once, so this test exercises the
        // serialization path directly rather than racing on installation.
        let value = serde_json::to_value(()).unwrap();
        assert_eq!(value, serde_json::Value::Null);

        #[derive(Serialize)]
        struct P {
            current: u32,
        }
        let typed = P { current: 3 };
        let value = serde_json::to_value(typed).unwrap();
        assert_eq!(value["current"], 3);
    }

    #[test]
    fn recording_sink_captures_names_and_payloads() {
        use super::testing::RecordingSink;
        let sink = RecordingSink::default();
        sink.emit("a", serde_json::json!({ "n": 1 }));
        sink.emit("b", serde_json::Value::Null);
        assert_eq!(sink.names(), vec!["a".to_string(), "b".to_string()]);
        let events = sink.events();
        assert_eq!(events[0].1["n"], 1);
        assert_eq!(events[1].1, serde_json::Value::Null);
    }

    #[test]
    fn default_sink_is_a_no_op() {
        // No sink installed in this test binary unless a startup path did so;
        // emitting must not panic either way.
        emit(names::INDEX_PROGRESS, serde_json::json!({ "current": 1 }));
        emit::<()>(names::VL_TASKS_CHANGED, ());
    }
}
