//! The application's own model of the things it manages.
//!
//! Nothing in here knows about HTTP, GTK or SSH. That is the point: the rules
//! about what a valid hostname is, what an ingress list must end with, and
//! when a DNS record counts as a conflict are the parts most worth testing,
//! and they are all reachable from a unit test with no network and no
//! display.
//!
//! Types here are deliberately *not* the wire format. [`crate::cloudflare`]
//! owns the DTOs and converts; that keeps a change in Cloudflare's JSON from
//! rippling into the UI.

pub mod account;
pub mod activity;
pub mod config;
pub mod host;
pub mod hostname;
pub mod tunnel;
pub mod validation;
pub mod zone;

pub use account::Account;
pub use activity::{ActivityCategory, ActivityEvent, ActivitySeverity};
pub use config::{IngressRule, OriginRequest, TunnelConfig, WarpRouting, CATCH_ALL_SERVICE};
pub use host::{RemoteAuth, RemoteHost, RemoteHostDraft, RemoteServiceState, RemoteStatus};
pub use hostname::{DnsRecord, DnsState, OriginScheme, OriginService, PublicHostname};
pub use tunnel::{ConfigSource, Connection, Connector, Tunnel, TunnelKind, TunnelStatus};
pub use validation::{ValidationError, ValidationResult};
pub use zone::{Zone, ZoneStatus};
