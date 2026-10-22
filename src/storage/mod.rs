//! Persistence, split by sensitivity.
//!
//! There are two stores and the split between them is the point:
//!
//! * [`db`] is a SQLite file holding settings, host profiles and the activity
//!   log. Its schema has no column that could hold a credential, and a test
//!   in that module enforces it.
//! * [`secrets`] is the desktop's own credential store, reached through
//!   `keyring`. It holds the Cloudflare API token and, optionally, an SSH key
//!   passphrase. Nothing else.
//!
//! Both are blocking. Nothing in this module may be called from the GTK main
//! thread — go through [`crate::services::runtime::blocking`].

pub mod activity;
pub mod db;
pub mod disabled;
pub mod hosts;
pub mod secrets;
pub mod settings;

pub use db::{StorageError, SCHEMA_VERSION};
pub use settings::Settings;
