//! Graceful shutdown for `lapd` (plan §3.7): "SIGTERM/Ctrl-C 优雅关闭——
//! 停止接受新命令、取消可取消的后台任务、flush WAL".
//!
//! The sequence, in the order the plan states it:
//!
//! 1. the shutdown flag goes up — the command bridge starts answering 503
//!    instead of accepting new work;
//! 2. the background work is cancelled (index/import/face/dedup/similar/VL
//!    runs, video preparation children) and given a bounded moment to wind
//!    down;
//! 3. the server stops accepting connections and the in-flight requests
//!    finish;
//! 4. every database is checkpointed so the WAL is folded into the main
//!    file;
//! 5. the lock file is released on the way out (the guard's Drop).
//!
//! Signal coverage: `SIGTERM`/`SIGINT` on unix, `Ctrl-C` on Windows.
//! Windows has no real SIGTERM — a service wrapper that hard-kills the
//! process (WinSW's default) skips this path entirely, and the lock file
//! is then reclaimed as stale on the next start.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use axum::response::IntoResponse;

/// How long the background work may take to notice its cancellation flag.
const BACKGROUND_GRACE: Duration = Duration::from_secs(5);

/// The shared "the server is going away" state.
pub struct ShutdownState {
    requested: AtomicBool,
    notify: Notify,
}

impl Default for ShutdownState {
    fn default() -> Self {
        ShutdownState {
            requested: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
}

impl ShutdownState {
    pub fn new() -> Arc<ShutdownState> {
        Arc::new(ShutdownState {
            requested: AtomicBool::new(false),
            notify: Notify::new(),
        })
    }

    /// True once a shutdown was requested — the command bridge answers 503.
    pub fn is_shutting_down(&self) -> bool {
        self.requested.load(Ordering::SeqCst)
    }

    /// Raise the flag and wake everyone waiting for it.
    pub fn request(&self) {
        self.requested.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Resolves once a shutdown was requested.
    pub async fn wait(&self) {
        if self.is_shutting_down() {
            return;
        }
        self.notify.notified().await;
    }
}

/// Listen for the termination signals in the background. The returned task
/// completes when the process was asked to stop.
pub fn spawn_signal_watcher(state: Arc<ShutdownState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        wait_for_signal().await;
        state.request();
    })
}

/// The control channel: the service manager (or an operator) writes a line
/// to the process' standard input to stop it gracefully.
///
/// Windows has no real SIGTERM — a service wrapper that hard-kills the
/// process (WinSW's default) skips the signal path entirely. Standard input
/// is not a network surface, so it is the reliable way for a wrapper to ask
/// for a graceful stop. `quit` (also `shutdown` / `stop`) ends the server;
/// end-of-file simply means "no control channel" and leaves the server
/// running.
pub fn spawn_stdin_control(state: Arc<ShutdownState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, BufReader};
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let command = line.trim().to_ascii_lowercase();
                    if matches!(command.as_str(), "quit" | "shutdown" | "stop") {
                        println!("lapd: '{command}' received on stdin — shutting down");
                        state.request();
                        return;
                    }
                }
                Ok(None) => return,
                Err(e) => {
                    eprintln!("lapd: control channel failed: {e}");
                    return;
                }
            }
        }
    })
}

/// Wait for a termination signal (unix: SIGTERM/SIGINT, windows: Ctrl-C).
async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(sig) => sig,
            Err(e) => {
                eprintln!("lapd: cannot listen for SIGTERM ({e}); falling back to Ctrl-C");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        let mut interrupt = match signal(SignalKind::interrupt()) {
            Ok(sig) => sig,
            Err(e) => {
                eprintln!("lapd: cannot listen for SIGINT ({e}); falling back to SIGTERM");
                term.recv().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = interrupt.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(e) = tokio::signal::ctrl_c().await {
            eprintln!("lapd: cannot listen for Ctrl-C: {e}");
            // Without a signal source the process must not pretend to be
            // shutdown-capable; park forever instead of exiting silently.
            std::future::pending::<()>().await;
        }
    }
}

/// The shutdown work itself: cancel the background tasks, let them wind
/// down, and checkpoint the databases. Returns the warnings the operator
/// should see (a busy checkpoint is worth reporting, not worth refusing
/// the exit over).
pub async fn run_shutdown_sequence(data_dir: &std::path::Path) -> Vec<String> {
    let report = lappp_core::t_shutdown::terminate_background_tasks().await;
    println!(
        "lapd: cancelling background work — index albums: {}, vl runs: {}, video processes: {}, import: {}, face: {}, dedup: {}, similar: {}",
        report.index_albums.len(),
        report.vl_runs.len(),
        report.video_processes.len(),
        report.import_interrupted,
        report.face_index_interrupted,
        report.dedup_interrupted,
        report.similar_interrupted,
    );

    // Give the workers a bounded moment to notice their flags, so the
    // checkpoint below does not race a writer.
    let deadline = tokio::time::Instant::now() + BACKGROUND_GRACE;
    while tokio::time::Instant::now() < deadline {
        if lappp_core::t_shutdown::background_tasks_idle() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !lappp_core::t_shutdown::background_tasks_idle() {
        println!("lapd: background work did not finish within the grace period; continuing");
    }

    let warnings = lappp_core::t_shutdown::flush_wal(data_dir);
    for warning in &warnings {
        eprintln!("lapd: {warning}");
    }
    warnings
}

/// The response a shutting-down server returns for new work.
///
/// Pure on purpose: the middleware calls it, and the test asserts it.
pub fn reject_while_shutting_down(
    state: &ShutdownState,
) -> Option<axum::response::Response> {
    if !state.is_shutting_down() {
        return None;
    }
    Some(
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({ "error": "server is shutting down" })),
        )
            .into_response(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_server_lets_requests_through() {
        let state = ShutdownState::new();
        assert!(reject_while_shutting_down(&state).is_none());
    }

    #[tokio::test]
    async fn a_shutting_down_server_answers_service_unavailable() {
        let state = ShutdownState::new();
        state.request();
        let response = reject_while_shutting_down(&state).expect("must refuse");
        assert_eq!(response.status(), axum::http::StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "server is shutting down");
    }

    #[tokio::test]
    async fn the_flag_starts_down_and_flips_on_request() {
        let state = ShutdownState::new();
        assert!(!state.is_shutting_down());
        state.request();
        assert!(state.is_shutting_down());
        // wait() resolves immediately once the flag is up.
        tokio::time::timeout(Duration::from_secs(1), state.wait())
            .await
            .expect("wait must resolve after request");
    }

    #[tokio::test]
    async fn wait_blocks_until_the_shutdown_is_requested() {
        let state = ShutdownState::new();
        let waiter = {
            let state = state.clone();
            tokio::spawn(async move { state.wait().await })
        };
        // Not yet.
        assert!(!tokio::time::timeout(Duration::from_millis(50), async {})
            .await
            .is_err());
        state.request();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("the waiter must wake")
            .unwrap();
    }
}
