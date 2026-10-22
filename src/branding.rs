//! Product identity, in one place.
//!
//! Everything that names the application — the D-Bus name, the resource
//! prefix, the window title, the strings in the About dialog — is derived from
//! the constants here. Renaming the product means editing this file, the two
//! `data/` filenames that must match [`APP_ID`], and the resource path in
//! `build.rs`; nothing else in `src/` spells the name out.
//!
//! `Tunnel Manager` is a neutral placeholder. This application is not
//! affiliated with, endorsed by, or produced by Cloudflare, Inc.; it is a
//! third-party client for their public API. Keep any replacement name clear of
//! Cloudflare's trademarks for the same reason.

/// The reverse-DNS application ID. Also the D-Bus name, the icon name and the
/// basename of the `.desktop` and metainfo files.
pub const APP_ID: &str = "com.chrisdaggas.tunnelmanager";

/// The name shown to the user.
pub const APP_NAME: &str = "Tunnel Manager";

/// A one-line description, used by the About dialog and the status bar.
pub const APP_TAGLINE: &str = "Manage remotely configured Cloudflare Tunnels";

/// The GResource prefix. Must match the `prefix` attribute in
/// `data/<APP_ID>.gresource.xml`.
pub const RESOURCE_PREFIX: &str = "/com/chrisdaggas/tunnelmanager";

/// The version, from `Cargo.toml`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The disclaimer that has to appear anywhere the product is named alongside
/// Cloudflare's — the About dialog and the onboarding page.
pub const AFFILIATION_NOTICE: &str = "An unofficial client for the Cloudflare API. \
    Not affiliated with or endorsed by Cloudflare, Inc.";

/// The subdirectory used under `$XDG_DATA_HOME` and `$XDG_CONFIG_HOME`.
pub const DATA_DIR_NAME: &str = "tunnel-manager";

/// The service name secrets are filed under in the OS credential store.
pub const KEYRING_SERVICE: &str = "com.chrisdaggas.tunnelmanager";

/// Builds a path inside the compiled-in GResource bundle.
pub fn resource(path: &str) -> String {
    format!("{RESOURCE_PREFIX}/{path}")
}
