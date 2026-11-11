//! Content for the "What's New" dialog.
//!
//! Kept as plain data so the dialog needs no per-release code. Newest first.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Release {
    pub version: &'static str,
    pub date: &'static str,
    pub changes: &'static [&'static str],
}

pub const RELEASES: &[Release] = &[Release {
    version: "1.0.0",
    date: "2026-10-22",
    changes: &[
        "A native GNOME client for Fedora: connect with a scoped Cloudflare API token \
             kept in the desktop credential store.",
        "Create, inspect, rename and delete remotely managed Cloudflare Tunnels, and \
             see their health and connector counts at a glance.",
        "Publish, repair and safely unpublish public hostnames. Every ingress rule and \
             originRequest field the application did not set is preserved exactly.",
        "Read Cloudflare's connector health separately from systemd state on the machine. \
             The two are never merged into one misleading switch.",
        "Optionally control cloudflared over SSH with pinned host keys and a fixed \
             allowlist of operations, or control the local service with no SSH at all.",
        "Pin QUIC or HTTP/2 for a connector, and use the adaptive GNOME interface in \
             light, dark or system theme down to 360px wide.",
    ],
}];
