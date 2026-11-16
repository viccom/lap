//! Process-level async runtime (serverization plan §3.2).
//!
//! The core used `tauri::async_runtime` for `spawn` / `spawn_blocking` /
//! `block_on`. Tauri backs those with a **dedicated** multi-thread tokio
//! runtime (not the caller's ambient one) — that is what lets `spawn` work
//! from a plain `std::thread`, and `block_on` run from a blocking thread. A
//! naive `tokio::spawn` / `Handle::block_on` swap would instead panic outside
//! an async context, so mirror Tauri's design here verbatim for an
//! equivalent-behaviour replacement.
//!
//! Desktop and `lapd` share this module: the app owns one runtime for its
//! whole life, exactly as before.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("failed to start the async runtime"))
}

/// Spawn a future onto the shared runtime (callable from any thread).
pub fn spawn<F>(task: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().handle().spawn(task)
}

/// Run a blocking closure on the runtime's blocking pool.
pub fn spawn_blocking<F, R>(func: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().handle().spawn_blocking(func)
}

/// Run a future to completion on the shared runtime.
///
/// Mirrors `tauri::async_runtime::block_on`: intended for synchronous
/// boundaries (tests, blocking threads), not for use inside an async context.
pub fn block_on<F: Future>(task: F) -> F::Output {
    runtime().block_on(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_board_and_block_on_share_one_runtime() {
        // spawn_blocking from a synchronous test thread must not panic.
        let value = block_on(spawn_blocking(|| 21 + 21));
        assert_eq!(value.unwrap(), 42);

        // spawn (async) from the same synchronous context, then join.
        let joined = block_on(spawn(async { "ok" }));
        assert_eq!(joined.unwrap(), "ok");
    }
}
