//! The bridge between the GTK main thread and everything that blocks.
//!
//! GTK is single-threaded and its main loop must never stop. Every HTTP
//! request, every SQLite query and every SSH connection in this application
//! therefore runs on a Tokio runtime, and the result crosses back over an
//! `async-channel` that the GLib main context awaits.
//!
//! Two rules follow, and breaking either one is the classic way to freeze a
//! GTK application:
//!
//! * **Never call [`Runtime::block_on`] from the main thread.** There is no
//!   API here that lets you. [`spawn`] is the only way in.
//! * **A future that touches a widget is not `Send`.** That is exactly why the
//!   callback runs on the GLib side of the channel rather than inside the
//!   Tokio task — and why it takes weak references to its widgets.
//!
//! A Tokio future cannot be polled by the GLib executor at all: `reqwest` and
//! `russh` need Tokio's reactor and timer in scope. The channel is not an
//! ergonomic nicety, it is the only way across.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::Runtime;

/// The one runtime, built on first use.
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            // A desktop client makes a handful of concurrent requests, not
            // thousands. Four threads is more than enough and keeps the
            // process's thread count reasonable.
            .worker_threads(4)
            .thread_name("tunnel-manager-io")
            // Both the reactor and the timer: `reqwest` needs the first,
            // `tokio::time::timeout` in the remote layer needs the second.
            .enable_all()
            .build()
            .unwrap_or_else(|error| {
                // Thread or resource exhaustion at first I/O. There is no
                // recovery inside this process; fail loudly rather than
                // limp on without a runtime.
                eprintln!("tunnel-manager: could not start the I/O runtime: {error}");
                std::process::exit(1);
            })
    })
}

/// Runs `work` off the main thread and calls `on_done` back on it.
///
/// `on_done` is deliberately **not** `Send`: it is expected to capture widgets,
/// and it runs on the GLib main context where touching them is legal. Capture
/// them weakly — a page can be gone by the time a slow request finishes, and a
/// strong capture would keep a destroyed window alive.
///
/// ```ignore
/// runtime::spawn(
///     async move { client.list_tunnels(&account, &cancel).await },
///     glib::clone!(
///         #[weak] page,
///         move |result| page.show_tunnels(result)
///     ),
/// );
/// ```
pub fn spawn<T, F, C>(work: F, on_done: C)
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
    C: FnOnce(T) + 'static,
{
    let (sender, receiver) = async_channel::bounded::<T>(1);

    runtime().spawn(async move {
        let value = work.await;
        // A closed channel means the page went away while the work was in
        // flight. That is normal, not an error.
        let _ = sender.send(value).await;
    });

    glib::spawn_future_local(async move {
        if let Ok(value) = receiver.recv().await {
            on_done(value);
        }
    });
}

/// Like [`spawn`], but the work reports progress as it goes.
///
/// Used by the multi-step operations — publishing a hostname, applying a
/// deployment — so the UI can show which step is running rather than a bare
/// spinner. The channel is bounded, so a producer that outruns the UI is
/// slowed down rather than allowed to queue without limit.
pub fn spawn_streaming<T, F, Fut, C>(producer: F, mut on_item: C)
where
    T: Send + 'static,
    F: FnOnce(async_channel::Sender<T>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
    C: FnMut(T) + 'static,
{
    let (sender, receiver) = async_channel::bounded::<T>(16);

    runtime().spawn(producer(sender));

    glib::spawn_future_local(async move {
        while let Ok(item) = receiver.recv().await {
            on_item(item);
        }
    });
}

/// Runs blocking work — SQLite, the credential store — on the runtime's
/// blocking pool and calls back on the main thread.
///
/// `rusqlite` and `keyring` are both synchronous and both can take tens of
/// milliseconds; on the main thread that is a visible stutter, and when the
/// keyring is locked it is a frozen window until the user notices the prompt
/// behind it.
pub fn spawn_blocking<T, F, C>(work: F, on_done: C)
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
    C: FnOnce(T) + 'static,
{
    spawn(
        async move {
            match tokio::task::spawn_blocking(work).await {
                Ok(value) => Some(value),
                // The only way this happens is a panic in `work`. Losing the
                // result is better than propagating the panic into the main
                // loop and taking the window down.
                Err(error) => {
                    tracing::error!(%error, "a blocking task panicked");
                    None
                }
            }
        },
        move |value| {
            if let Some(value) = value {
                on_done(value);
            }
        },
    )
}

/// Awaits blocking work from inside an async context.
pub async fn blocking<T, F>(work: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    tokio::task::spawn_blocking(work).await.ok()
}

/// Runs a future to completion on the runtime, from a *non-main* thread.
///
/// Only for tests and for `main`'s shutdown path. Calling it from the GTK main
/// thread would block the main loop, which is why nothing in `ui` may use it.
#[cfg(test)]
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}

/// A repeating timer on the GLib main context.
///
/// Used for the automatic refresh. `glib::timeout_add_local` rather than a
/// Tokio interval because the callback touches widgets, and because a timer
/// that stops when its widget dies is exactly what
/// [`glib::ControlFlow::Break`] gives for free.
pub fn every(
    interval: std::time::Duration,
    mut tick: impl FnMut() -> bool + 'static,
) -> glib::SourceId {
    glib::timeout_add_local(interval, move || {
        if tick() {
            glib::ControlFlow::Continue
        } else {
            glib::ControlFlow::Break
        }
    })
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
        assert!(result.is_none(), "a panic must not reach the main loop");
    }

    #[test]
    fn a_dropped_receiver_does_not_panic_the_producer() {
        // `spawn` sends into a channel whose receiver lives on the GLib main
        // context. If the page goes away first the send fails, and that has to
        // be a no-op rather than a panic on a worker thread.
        let (sender, receiver) = async_channel::bounded::<u32>(1);
        drop(receiver);
        let result = block_on(async move { sender.send(1).await });
        assert!(result.is_err(), "the send fails, and the caller ignores it");
    }
}
