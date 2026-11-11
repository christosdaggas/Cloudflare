//! Tunnels, and the two different kinds of "is it up" a tunnel has.
//!
//! Cloudflare reports a tunnel's health from the edge: how many connectors are
//! registered and whether all of their connections are established. A
//! connector's *service* state — whether `cloudflared` is running on some
//! machine — is a separate fact that only the remote module can see, and only
//! for hosts the user has registered. The two are kept in separate types on
//! purpose so that no screen can accidentally present one as the other.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What Cloudflare says about a tunnel, from the edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TunnelStatus {
    /// Every registered connector has all four of its connections up.
    Healthy,
    /// Connectors are registered but not all connections are established.
    Degraded,
    /// Connectors were registered and are now unreachable.
    Down,
    /// No connector has ever registered, or none is registered now.
    Inactive,
    /// A value Cloudflare added after this build. Shown verbatim rather than
    /// mapped on to a guess.
    Other(String),
}

impl TunnelStatus {
    pub fn from_api(value: &str) -> Self {
        match value {
            "healthy" => Self::Healthy,
            "degraded" => Self::Degraded,
            "down" => Self::Down,
            "inactive" => Self::Inactive,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Down => "Down",
            Self::Inactive => "Inactive",
            Self::Other(value) => value,
        }
    }

    /// A symbolic icon so the status is legible without colour. Every status
    /// gets a distinct *shape*, not a distinct hue: an application that says
    /// "down" only in red says nothing at all to a colour-blind user.
    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Healthy => "object-select-symbolic",
            Self::Degraded => "dialog-warning-symbolic",
            Self::Down => "dialog-error-symbolic",
            Self::Inactive => "media-playback-pause-symbolic",
            Self::Other(_) => "dialog-question-symbolic",
        }
    }

    /// The class that tints the badge. Always paired with the icon and the
    /// label above, never used alone.
    ///
    /// Inactive gets `.status-neutral` rather than `.dimmed`: `.dimmed` drops
    /// the whole subtree to 55% opacity, and a *status* is not secondary
    /// information even when the status is "nothing is happening". The
    /// neutral class gives the pill a quiet background and leaves the text at
    /// full contrast.
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Healthy => "success",
            Self::Degraded => "warning",
            Self::Down => "error",
            Self::Inactive | Self::Other(_) => "status-neutral",
        }
    }

    /// One sentence explaining what the status actually means, for a tooltip
    /// or a detail row. "Degraded" is not self-explanatory.
    pub fn explanation(&self) -> &'static str {
        match self {
            Self::Healthy => "All registered connectors have every connection established.",
            Self::Degraded => {
                "At least one connector is registered but is missing some of its \
                 connections to the Cloudflare edge."
            }
            Self::Down => "Connectors were registered for this tunnel and are now unreachable.",
            Self::Inactive => "No connector is registered. Nothing is serving this tunnel yet.",
            Self::Other(_) => "Cloudflare reported a status this version does not recognise.",
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Healthy | Self::Degraded)
    }
}

/// Where the tunnel's ingress configuration lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ConfigSource {
    /// Held by Cloudflare and editable through the API. This is the only kind
    /// this application creates, and the only kind whose configuration it will
    /// write.
    Cloudflare,
    /// Held in a `config.yml` on the machine running the connector. The API
    /// will accept a configuration write for one of these and the connector
    /// will ignore it, so the application refuses instead.
    Local,
    Other(String),
}

impl ConfigSource {
    pub fn from_api(value: &str) -> Self {
        match value {
            "cloudflare" => Self::Cloudflare,
            "local" => Self::Local,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_api(&self) -> &str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::Local => "local",
            Self::Other(value) => value,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Cloudflare => "Remotely managed",
            Self::Local => "Locally configured",
            Self::Other(value) => value,
        }
    }

    /// Whether this application may write the tunnel's ingress configuration.
    pub fn is_remotely_managed(&self) -> bool {
        matches!(self, Self::Cloudflare)
    }

    /// Why an edit is refused, when it is.
    pub fn edit_refusal(&self) -> Option<&'static str> {
        match self {
            Self::Cloudflare => None,
            Self::Local => Some(
                "This tunnel is configured by a config.yml on the machine running its \
                 connector. Writing a remote configuration would be ignored by that \
                 connector, so hostname and ingress editing is disabled here.",
            ),
            Self::Other(_) => Some(
                "This tunnel reports a configuration source this version does not \
                 recognise, so its configuration is not edited from here.",
            ),
        }
    }
}

/// How the tunnel was created. Only `cfd_tunnel` is a cloudflared tunnel;
/// the others are different products sharing the endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum TunnelKind {
    Cfd,
    Warp,
    Magic,
    Other(String),
}

impl TunnelKind {
    pub fn from_api(value: &str) -> Self {
        match value {
            "cfd_tunnel" => Self::Cfd,
            "warp_connector" => Self::Warp,
            "magic" | "ip_sec" | "gre" | "cni" => Self::Magic,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Cfd => "Cloudflare Tunnel",
            Self::Warp => "WARP connector",
            Self::Magic => "Magic WAN",
            Self::Other(value) => value,
        }
    }
}

/// One `cloudflared` process registered against a tunnel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connector {
    /// The client UUID. This is what a `DELETE .../connections` call targets,
    /// and what identifies one machine among several serving one tunnel.
    pub id: String,
    pub version: String,
    pub arch: String,
    pub run_at: Option<DateTime<Utc>>,
    pub features: Vec<String>,
    pub config_version: Option<i64>,
    pub connections: Vec<Connection>,
}

impl Connector {
    /// Whether any of this connector's connections is still established.
    pub fn is_connected(&self) -> bool {
        self.connections
            .iter()
            .any(|connection| !connection.is_pending_reconnect)
    }

    pub fn established_count(&self) -> usize {
        self.connections
            .iter()
            .filter(|connection| !connection.is_pending_reconnect)
            .count()
    }

    /// The distinct Cloudflare colocations this connector reaches, in a stable
    /// order, for the detail page.
    pub fn colos(&self) -> Vec<String> {
        let mut colos: Vec<String> = self
            .connections
            .iter()
            .map(|connection| connection.colo_name.clone())
            .filter(|colo| !colo.is_empty())
            .collect();
        colos.sort();
        colos.dedup();
        colos
    }

    /// A short label for the machine, when one can be inferred. Cloudflare
    /// does not report a hostname, so this is the origin IP if there is one.
    pub fn origin_ip(&self) -> Option<&str> {
        self.connections
            .iter()
            .find_map(|connection| connection.origin_ip.as_deref())
    }
}

/// One of the four connections a connector opens to the edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub colo_name: String,
    pub is_pending_reconnect: bool,
    pub origin_ip: Option<String>,
    pub opened_at: Option<DateTime<Utc>>,
    pub client_id: Option<String>,
    pub client_version: Option<String>,
}

/// A tunnel, as the application models it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tunnel {
    pub id: String,
    pub account_tag: String,
    pub name: String,
    pub status: TunnelStatus,
    pub config_source: ConfigSource,
    pub kind: TunnelKind,
    pub remote_config: bool,
    pub created_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub conns_active_at: Option<DateTime<Utc>>,
    pub conns_inactive_at: Option<DateTime<Utc>>,
    /// Connectors as reported inline by the list endpoint. The detail page
    /// refetches these from `/connections`, which reports more.
    pub connector_count: usize,
    /// Filled in from the tunnel's configuration, which the list endpoint does
    /// not include. `None` means "not fetched", which is different from zero.
    pub hostname_count: Option<usize>,
}

impl Tunnel {
    /// The CNAME target a public hostname for this tunnel must point at.
    pub fn cname_target(&self) -> String {
        format!("{}.cfargotunnel.com", self.id)
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }

    /// When the tunnel last had a connector attached, whichever way round the
    /// two timestamps are.
    pub fn last_active_at(&self) -> Option<DateTime<Utc>> {
        match (self.conns_active_at, self.conns_inactive_at) {
            (Some(active), Some(inactive)) => Some(active.max(inactive)),
            (Some(active), None) => Some(active),
            (None, Some(inactive)) => Some(inactive),
            (None, None) => None,
        }
    }

    /// Whether the application may write this tunnel's ingress configuration.
    pub fn is_editable(&self) -> bool {
        self.config_source.is_remotely_managed() && !self.is_deleted()
    }
}

/// The counts the dashboard shows. Kept as a type so the dashboard cannot
/// silently disagree with the tunnel list about what "degraded" means.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelSummary {
    pub healthy: usize,
    pub degraded: usize,
    pub down: usize,
    pub inactive: usize,
    pub other: usize,
}

impl TunnelSummary {
    pub fn of(tunnels: &[Tunnel]) -> Self {
        let mut summary = Self::default();
        for tunnel in tunnels.iter().filter(|tunnel| !tunnel.is_deleted()) {
            match tunnel.status {
                TunnelStatus::Healthy => summary.healthy += 1,
                TunnelStatus::Degraded => summary.degraded += 1,
                TunnelStatus::Down => summary.down += 1,
                TunnelStatus::Inactive => summary.inactive += 1,
                TunnelStatus::Other(_) => summary.other += 1,
            }
        }
        summary
    }

    pub fn total(&self) -> usize {
        self.healthy + self.degraded + self.down + self.inactive + self.other
    }

    /// Whether anything on the account needs looking at.
    pub fn needs_attention(&self) -> bool {
        self.degraded > 0 || self.down > 0
    }
}

/// The filter chips above the tunnel list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelFilter {
    All,
    Healthy,
    Degraded,
    Down,
    Inactive,
}

impl TunnelFilter {
    pub const ALL: &'static [TunnelFilter] = &[
        Self::All,
        Self::Healthy,
        Self::Degraded,
        Self::Down,
        Self::Inactive,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Down => "down",
            Self::Inactive => "inactive",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Healthy => "Healthy",
            Self::Degraded => "Degraded",
            Self::Down => "Down",
            Self::Inactive => "Inactive",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|item| item.name() == name)
    }

    pub fn matches(self, tunnel: &Tunnel) -> bool {
        match self {
            Self::All => true,
            Self::Healthy => tunnel.status == TunnelStatus::Healthy,
            Self::Degraded => tunnel.status == TunnelStatus::Degraded,
            Self::Down => tunnel.status == TunnelStatus::Down,
            Self::Inactive => matches!(
                tunnel.status,
                TunnelStatus::Inactive | TunnelStatus::Other(_)
            ),
        }
    }

    pub fn count(self, summary: &TunnelSummary) -> usize {
        match self {
            Self::All => summary.total(),
            Self::Healthy => summary.healthy,
            Self::Degraded => summary.degraded,
            Self::Down => summary.down,
            Self::Inactive => summary.inactive + summary.other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tunnel(name: &str, status: TunnelStatus) -> Tunnel {
        Tunnel {
            id: "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415".into(),
            account_tag: "a".repeat(32),
            name: name.into(),
            status,
            config_source: ConfigSource::Cloudflare,
            kind: TunnelKind::Cfd,
            remote_config: true,
            created_at: None,
            deleted_at: None,
            conns_active_at: None,
            conns_inactive_at: None,
            connector_count: 0,
            hostname_count: None,
        }
    }

    #[test]
    fn status_maps_the_documented_values_and_keeps_the_rest() {
        assert_eq!(TunnelStatus::from_api("healthy"), TunnelStatus::Healthy);
        assert_eq!(TunnelStatus::from_api("degraded"), TunnelStatus::Degraded);
        assert_eq!(TunnelStatus::from_api("down"), TunnelStatus::Down);
        assert_eq!(TunnelStatus::from_api("inactive"), TunnelStatus::Inactive);
        assert_eq!(
            TunnelStatus::from_api("brand-new"),
            TunnelStatus::Other("brand-new".into())
        );
    }

    #[test]
    fn every_status_has_a_distinct_icon() {
        let statuses = [
            TunnelStatus::Healthy,
            TunnelStatus::Degraded,
            TunnelStatus::Down,
            TunnelStatus::Inactive,
        ];
        let mut icons: Vec<&str> = statuses.iter().map(TunnelStatus::icon_name).collect();
        icons.sort_unstable();
        let before = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), before, "statuses must not share an icon");
    }

    #[test]
    fn a_locally_configured_tunnel_is_not_editable() {
        let mut local = tunnel("local", TunnelStatus::Healthy);
        local.config_source = ConfigSource::Local;
        assert!(!local.is_editable());
        assert!(local.config_source.edit_refusal().is_some());

        let remote = tunnel("remote", TunnelStatus::Healthy);
        assert!(remote.is_editable());
        assert!(remote.config_source.edit_refusal().is_none());
    }

    #[test]
    fn a_deleted_tunnel_is_not_editable() {
        let mut deleted = tunnel("gone", TunnelStatus::Inactive);
        deleted.deleted_at = Some(Utc::now());
        assert!(!deleted.is_editable());
    }

    #[test]
    fn cname_target_is_the_tunnel_id() {
        let tunnel = tunnel("t", TunnelStatus::Healthy);
        assert_eq!(
            tunnel.cname_target(),
            "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415.cfargotunnel.com"
        );
    }

    #[test]
    fn summary_counts_by_status_and_skips_deleted() {
        let mut gone = tunnel("gone", TunnelStatus::Healthy);
        gone.deleted_at = Some(Utc::now());

        let tunnels = vec![
            tunnel("a", TunnelStatus::Healthy),
            tunnel("b", TunnelStatus::Healthy),
            tunnel("c", TunnelStatus::Degraded),
            tunnel("d", TunnelStatus::Down),
            tunnel("e", TunnelStatus::Inactive),
            gone,
        ];

        let summary = TunnelSummary::of(&tunnels);
        assert_eq!(summary.healthy, 2);
        assert_eq!(summary.degraded, 1);
        assert_eq!(summary.down, 1);
        assert_eq!(summary.inactive, 1);
        assert_eq!(summary.total(), 5);
        assert!(summary.needs_attention());
    }

    #[test]
    fn filters_partition_the_list() {
        let tunnels = [
            tunnel("a", TunnelStatus::Healthy),
            tunnel("b", TunnelStatus::Degraded),
            tunnel("c", TunnelStatus::Down),
            tunnel("d", TunnelStatus::Inactive),
            tunnel("e", TunnelStatus::Other("weird".into())),
        ];

        let counted: usize = [
            TunnelFilter::Healthy,
            TunnelFilter::Degraded,
            TunnelFilter::Down,
            TunnelFilter::Inactive,
        ]
        .iter()
        .map(|filter| tunnels.iter().filter(|t| filter.matches(t)).count())
        .sum();

        assert_eq!(counted, tunnels.len());
        assert_eq!(
            tunnels
                .iter()
                .filter(|t| TunnelFilter::All.matches(t))
                .count(),
            tunnels.len()
        );
    }

    #[test]
    fn connector_reports_established_connections() {
        let connector = Connector {
            id: "c".into(),
            version: "2026.1.0".into(),
            arch: "linux_amd64".into(),
            run_at: None,
            features: vec![],
            config_version: Some(3),
            connections: vec![
                Connection {
                    id: "1".into(),
                    colo_name: "ATH".into(),
                    is_pending_reconnect: false,
                    origin_ip: Some("203.0.113.5".into()),
                    opened_at: None,
                    client_id: None,
                    client_version: None,
                },
                Connection {
                    id: "2".into(),
                    colo_name: "ATH".into(),
                    is_pending_reconnect: true,
                    origin_ip: None,
                    opened_at: None,
                    client_id: None,
                    client_version: None,
                },
            ],
        };

        assert!(connector.is_connected());
        assert_eq!(connector.established_count(), 1);
        assert_eq!(connector.colos(), vec!["ATH".to_string()]);
        assert_eq!(connector.origin_ip(), Some("203.0.113.5"));
    }
}
