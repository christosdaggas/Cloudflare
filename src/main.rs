//! The binary entry point.
//!
//! Everything else lives in the library (`src/lib.rs`) so that the integration
//! tests in `tests/` can link against it. This file holds only what genuinely
//! belongs to a running process: starting the Tauri application.

fn main() {
    tunnel_manager::bridge::run();
}
