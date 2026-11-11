//! The bridge between the async world and everything that blocks.
//!
//! `rusqlite`, `keyring` and `std::process` are all synchronous, and the first
//! two can take tens of milliseconds — on the UI thread that is a visible
//! stutter, and when the keyring is locked it is a frozen window until the user
//! notices the prompt behind it. Every one of them therefore runs on Tokio's
//! blocking pool through [`blocking`].
//!
//! Network and SSH work is already async and is awaited directly by the Tauri
//! command that owns it; there is no callback hop in this build.

#[cfg(test)]
use std::future::Future;
#[cfg(test)]
use std::sync::OnceLock;

#[cfg(test)]
use tokio::runtime::Runtime;

/// A private runtime, used only by tests.
#[cfg(test)]
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

#[cfg(test)]
fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            // A desktop client makes a handful of concurrent requests, not
            // thousands. Four threads is more than enough and keeps the
            // process's thread count reasonable.
            .worker_threads(4)
            .thread_name("tunnel-manager-io")
            .enable_all()
            .build()
            .unwrap_or_else(|error| {
                eprintln!("tunnel-manager: could not start the I/O runtime: {error}");
                std::process::exit(1);
            })
    })
}

/// Awaits blocking work from inside an async context.
///
/// Returns `None` only when the blocking task panicked; a panic in a database
/// call is reported rather than allowed to take the whole process down.
pub async fn blocking<T, F>(work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.ok()
}

/// Runs a future to completion on the private runtime.
///
/// Only for tests and for `main`'s shutdown path. Calling it from inside a
/// Tauri command would stall that command, which is why nothing in
/// `bridge` may use it.
#[cfg(test)]
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn the_runtime_is_a_singleton() {
        let first = runtime() as *const Runtime;
        let second = runtime() as *const Runtime;
        assert_eq!(first, second, "a second runtime would double the threads");
    }

    #[test]
    fn work_runs_on_the_runtime_and_returns_its_value() {
        let value = block_on(async { 21 * 2 });
        assert_eq!(value, 42);
    }

    #[test]
    fn blocking_work_runs_on_the_blocking_pool() {
        let counter = Arc::new(AtomicU32::new(0));
        let clone = Arc::clone(&counter);

        let result = block_on(async move {
            blocking(move || {
                clone.fetch_add(1, Ordering::SeqCst);
                "done"
            })
            .await
        });

        assert_eq!(result, Some("done"));
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_panicking_blocking_task_returns_none_rather_than_unwinding() {
        let result: Option<()> = block_on(async { blocking(|| panic!("deliberate")).await });
        assert!(result.is_none(), "a panic must not reach the caller");
    }
}
