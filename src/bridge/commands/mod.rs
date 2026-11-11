//! The Tauri command surface. Every function here is called from the React
//! frontend; nothing below this module knows that a webview exists.

pub mod activity;
pub mod app;
pub mod connection;
pub mod hostnames;
pub mod hosts;
pub mod settings;
pub mod tunnels;
pub mod zones;
