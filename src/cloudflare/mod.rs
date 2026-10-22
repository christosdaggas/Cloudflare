//! A typed client for the Cloudflare v4 API.
//!
//! The layering rule for this module is that it knows about HTTP and about
//! [`crate::domain`], and about nothing else. It never touches GTK, SQLite or
//! SSH, which is what lets the whole of it be tested against a local mock
//! server with no credentials and no display.

pub mod client;
pub mod envelope;
pub mod error;
pub mod models;

pub use client::{Cancel, CloudflareClient, Timeouts, DEFAULT_BASE_URL};
pub use envelope::{ApiMessage, Envelope, ResultInfo};
pub use error::{Capability, CloudflareError, Operation, Result};
pub use models::TokenStatus;
