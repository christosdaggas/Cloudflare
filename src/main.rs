//! The binary entry point.
//!
//! Everything else lives in the library (`src/lib.rs`) so that the integration
//! tests in `tests/` can link against it. This file holds only what genuinely
//! belongs to a running process: initialising libadwaita, registering the
//! compiled-in resources, and starting the application.

use gtk4::prelude::*;
use gtk4::{gdk, gio, glib};

use tunnel_manager::application::TunnelManagerApplication;
use tunnel_manager::{branding, services};

fn main() -> glib::ExitCode {
    glib::set_prgname(Some(branding::APP_ID));
    glib::set_application_name(branding::APP_NAME);

    services::logging::init();

    if let Err(error) = libadwaita::init() {
        eprintln!("Failed to initialise libadwaita: {error}");
        return glib::ExitCode::FAILURE;
    }

    register_resources();
    register_icon_search_paths();

    TunnelManagerApplication::new().run()
}

/// The stylesheet and the application icons are compiled into the binary by
/// `build.rs`, so a `cargo run` from a source checkout looks exactly like an
/// installed copy.
///
/// This stays in the binary rather than the library because `OUT_DIR` is a
/// property of the build that produced the executable.
fn register_resources() {
    let bytes = glib::Bytes::from_static(include_bytes!(concat!(
        env!("OUT_DIR"),
        "/tunnel-manager.gresource"
    )));

    match gio::Resource::from_data(&bytes) {
        Ok(resource) => gio::resources_register(&resource),
        Err(error) => eprintln!("Failed to register resources: {error}"),
    }
}

/// Lets the icon theme find the bundled application icon by name.
fn register_icon_search_paths() {
    let Some(display) = gdk::Display::default() else {
        return;
    };

    gtk4::IconTheme::for_display(&display).add_resource_path(&branding::resource("icons"));
}
