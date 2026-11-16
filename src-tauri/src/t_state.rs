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

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

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
}
