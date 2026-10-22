//! Tracing setup.
//!
//! Two things make this more than three lines of boilerplate.
//!
//! **Everything written is redacted first.** The writer, not the call site, is
//! where redaction happens: a `tracing` macro somewhere that interpolates a
//! response body would otherwise be one careless line away from putting an API
//! token on stderr. Redacting at the sink means every present and future call
//! site is covered, including ones inside dependencies.
//!
//! **The last few hundred lines are kept in memory.** The diagnostic export on
//! the Settings page needs recent log output, and reading it back out of the
//! journal is neither portable nor available when the application was started
//! from a terminal. The buffer holds the same redacted text that went to
//! stderr — never anything more.

use std::io::{self, Write};
use std::sync::{Mutex, OnceLock, RwLock};

use tracing::level_filters::LevelFilter;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{reload, EnvFilter, Layer};

use crate::domain::activity::redact;

/// How many lines the in-memory buffer keeps. Enough to cover one failed
/// operation and the requests around it; small enough that a long-running
/// window does not grow without bound.
const BUFFER_LINES: usize = 500;

/// The levels offered in Settings, in order.
pub const LOG_LEVELS: &[(&str, &str)] = &[
    ("error", "Errors only"),
    ("warn", "Warnings and errors"),
    ("info", "Normal"),
    ("debug", "Verbose"),
    ("trace", "Everything"),
];

pub const DEFAULT_LOG_LEVEL: &str = "info";

type ReloadHandle = reload::Handle<EnvFilter, tracing_subscriber::Registry>;

static RELOAD: OnceLock<ReloadHandle> = OnceLock::new();
static BUFFER: OnceLock<Mutex<RingBuffer>> = OnceLock::new();
static CURRENT_LEVEL: RwLock<String> = RwLock::new(String::new());

/// Installs the subscriber. Safe to call once; later calls do nothing.
pub fn init() {
    let _ = BUFFER.set(Mutex::new(RingBuffer::new(BUFFER_LINES)));

    // RUST_LOG wins if it is set, so a developer can still turn something up
    // without going through the UI.
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .with_env_var("RUST_LOG")
        .from_env_lossy();

    if let Ok(mut current) = CURRENT_LEVEL.write() {
        *current = DEFAULT_LOG_LEVEL.to_string();
    }

    let (filter, handle) = reload::Layer::new(filter);
    let _ = RELOAD.set(handle);

    let fmt = tracing_subscriber::fmt::layer()
        .with_writer(RedactingWriter)
        .with_target(true)
        .with_ansi(std::io::IsTerminal::is_terminal(&io::stderr()))
        .with_filter(filter);

    // `try_init` rather than `init`: a test binary may already have installed
    // one, and a panic on startup because logging was set up twice would be an
    // absurd way to lose a session.
    let _ = tracing_subscriber::registry().with(fmt).try_init();
}

/// Changes the level at runtime, from the Settings page.
///
/// Returns false if the subscriber was never installed or the directive does
/// not parse — the caller reports that rather than pretending it took.
pub fn set_level(level: &str) -> bool {
    let Some(handle) = RELOAD.get() else {
        return false;
    };
    let Ok(filter) = EnvFilter::try_new(level) else {
        return false;
    };
    if handle.reload(filter).is_err() {
        return false;
    }
    if let Ok(mut current) = CURRENT_LEVEL.write() {
        *current = level.to_string();
    }
    true
}

pub fn current_level() -> String {
    CURRENT_LEVEL
        .read()
        .map(|level| level.clone())
        .unwrap_or_default()
}

/// The recent log output, already redacted, newest last.
pub fn recent_lines() -> Vec<String> {
    BUFFER
        .get()
        .and_then(|buffer| buffer.lock().ok())
        .map(|buffer| buffer.lines())
        .unwrap_or_default()
}

/// A fixed-capacity line buffer.
struct RingBuffer {
    capacity: usize,
    lines: std::collections::VecDeque<String>,
    /// Bytes seen since the last newline, so a write split mid-line does not
    /// produce two half lines.
    partial: String,
}

impl RingBuffer {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            lines: std::collections::VecDeque::with_capacity(capacity),
            partial: String::new(),
        }
    }

    fn push(&mut self, text: &str) {
        self.partial.push_str(text);
        while let Some(index) = self.partial.find('\n') {
            let line: String = self.partial.drain(..=index).collect();
            let line = line.trim_end_matches(['\n', '\r']).to_string();
            if self.lines.len() == self.capacity {
                self.lines.pop_front();
            }
            self.lines.push_back(line);
        }
        // A pathological writer that never emits a newline must not grow this
        // without bound.
        if self.partial.len() > 64 * 1024 {
            self.partial.clear();
        }
    }

    fn lines(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

/// Wraps stderr, redacting every write and teeing it into the ring buffer.
#[derive(Clone, Copy)]
struct RedactingWriter;

impl<'a> MakeWriter<'a> for RedactingWriter {
    type Writer = RedactingSink;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingSink
    }
}

struct RedactingSink;

impl Write for RedactingSink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let redacted = redact(&text);

        if let Some(buffer) = BUFFER.get() {
            if let Ok(mut buffer) = buffer.lock() {
                buffer.push(&redacted);
            }
        }

        io::stderr().write_all(redacted.as_bytes())?;
        // Report the caller's length, not the redacted one: a short write is
        // an error to `fmt`, and the redaction is not the caller's business.
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_buffer_keeps_only_the_last_lines() {
        let mut buffer = RingBuffer::new(3);
        for index in 0..10 {
            buffer.push(&format!("line {index}\n"));
        }
        assert_eq!(buffer.lines(), vec!["line 7", "line 8", "line 9"]);
    }

    #[test]
    fn the_ring_buffer_joins_a_split_line() {
        let mut buffer = RingBuffer::new(4);
        buffer.push("hello ");
        buffer.push("world\n");
        assert_eq!(buffer.lines(), vec!["hello world"]);
    }

    #[test]
    fn the_ring_buffer_does_not_grow_without_a_newline() {
        let mut buffer = RingBuffer::new(4);
        buffer.push(&"x".repeat(70 * 1024));
        assert!(buffer.partial.len() <= 64 * 1024);
        assert!(buffer.lines().is_empty());
    }

    #[test]
    fn the_sink_redacts_before_the_buffer_sees_it() {
        // Uses the real global buffer, so it only asserts on what it wrote.
        let _ = BUFFER.set(Mutex::new(RingBuffer::new(8)));
        let mut sink = RedactingSink;
        let secret = "abcdefghijklmnopqrstuvwxyz0123456789";
        let written = sink
            .write(format!("authorization: Bearer {secret}\n").as_bytes())
            .unwrap();

        assert_eq!(written, format!("authorization: Bearer {secret}\n").len());
        let lines = recent_lines();
        let last = lines.last().expect("a line was buffered");
        assert!(!last.contains(secret), "got {last}");
    }

    #[test]
    fn the_level_list_is_ordered_from_quiet_to_loud() {
        let names: Vec<&str> = LOG_LEVELS.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec!["error", "warn", "info", "debug", "trace"]);
        assert!(names.contains(&DEFAULT_LOG_LEVEL));
    }
}
