//! The pages, the dialogs, and the shell they live in.
//!
//! Two conventions run through this module.
//!
//! **Every page is a `GObject` subclass of `AdwBin`.** A product page owns
//! state — widgets it updates later, requests in flight, a subscription to
//! [`state::AppState`] — and a subclass is what gives it an identity to hang
//! those on. `dashboard_page.rs` is the shortest complete example.
//!
//! **A page never holds a reference to its window.** Anything that needs the
//! window goes through a `win.` action, and anything that needs a toast finds
//! the overlay by walking up the widget tree. That is what lets a page be
//! built before it is added to anything, and lets a dialog outlive the widget
//! that raised it.

mod dashboard_page;
mod onboarding_page;
mod tunnel_detail_page;
mod tunnels_page;
mod window;

pub mod activity_page;
pub mod create_tunnel_dialog;
pub mod deployment_dialog;
pub mod host_dialog;
pub mod hostname_dialog;
pub mod hostnames_page;
pub mod hosts_page;
pub mod models;
pub mod page;
pub mod reinstall_dialog;
pub mod settings_page;
pub mod state;
pub mod widgets;
pub mod zones_page;

pub use activity_page::ActivityPage;
pub use dashboard_page::DashboardPage;
pub use hostnames_page::HostnamesPage;
pub use hosts_page::HostsPage;
pub use onboarding_page::{refresh_tunnels, restore_session, OnboardingPage};
pub use settings_page::SettingsPage;
pub use tunnel_detail_page::TunnelDetailPage;
pub use tunnels_page::TunnelsPage;
pub use window::MainWindow;
pub use zones_page::ZonesPage;

pub use state::{AppState, Event, Store};
pub use widgets as w;
