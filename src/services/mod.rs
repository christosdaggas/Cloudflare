//! Use cases, and the machinery that runs them off the GTK main thread.
//!
//! A service here composes [`crate::cloudflare`], [`crate::storage`] and
//! [`crate::remote`] into one operation the UI can invoke and one result it
//! can render. The multi-step ones — publishing a hostname, creating a tunnel,
//! deploying a connector — return a list of step outcomes rather than a
//! `Result`, because every one of them can half succeed and the user has to be
//! told which half.

pub mod hostnames;
pub mod hosts;
pub mod local;
pub mod logging;
pub mod runtime;
pub mod session;
pub mod tunnels;
