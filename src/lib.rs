//! Tunnel Manager — a GNOME client for the Cloudflare API.
//!
//! An unofficial, third-party application: it talks to Cloudflare's public v4
//! API with a scoped API token the user supplies, and to registered Linux
//! hosts over SSH when — and only when — the user has set one up.
//!
//! The layering is deliberate and worth keeping:
//!
//! * [`domain`] holds the application's own model of a tunnel, a hostname, a
//!   zone and a host. It knows nothing about HTTP, GTK or SSH.
//! * [`cloudflare`] is a typed client for the v4 API and the DTOs that map on
//!   to it. It knows nothing about GTK or SSH.
//! * [`remote`] is the optional SSH/systemd adapter, behind a trait so the
//!   default test suite can run against a mock and the application can run
//!   with no remote host configured at all.
//! * [`storage`] persists non-secret state in SQLite and secrets in the
//!   operating system's credential store — never both in the same place.
//! * [`services`] orchestrates the three above into the use cases the UI
//!   invokes, and owns the background runtime.
//! * [`ui`] is the only module that may mention GTK.
//!
//! Nothing below `ui` blocks the GTK main thread: every call into the network,
//! the database or SSH goes through [`services::runtime`], which runs it on a
//! Tokio runtime and hands the result back to the GLib main context.
//!
//! This is a library target as well as a binary so that the integration tests
//! in `tests/` can link against it. `main.rs` is only the entry point.

pub mod application;
pub mod branding;
pub mod cloudflare;
pub mod domain;
pub mod release_notes;
pub mod remote;
pub mod services;
pub mod storage;
pub mod theme;
pub mod ui;
pub mod version_check;
