//! Zones — the domains on the account.

use serde::{Deserialize, Serialize};
/// A zone's activation state, as Cloudflare reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ZoneStatus {
    /// Serving traffic.
    Active,
    /// Added but not yet using Cloudflare's nameservers.
    Pending,
    /// Set up but never activated.
    Initializing,
    /// Deactivated by the account holder or by Cloudflare.
    Moved,
    Deactivated,
    Other(String),
}

impl ZoneStatus {
    pub fn from_api(value: &str) -> Self {
        match value {
            "active" => Self::Active,
            "pending" => Self::Pending,
            "initializing" => Self::Initializing,
            "moved" => Self::Moved,
            "deactivated" => Self::Deactivated,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Active => "Active",
            Self::Pending => "Pending nameservers",
            Self::Initializing => "Initialising",
            Self::Moved => "Moved",
            Self::Deactivated => "Deactivated",
            Self::Other(value) => value,
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Active => "object-select-symbolic",
            Self::Pending | Self::Initializing => "content-loading-symbolic",
            Self::Moved | Self::Deactivated => "action-unavailable-symbolic",
            Self::Other(_) => "dialog-question-symbolic",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Active => "success",
            Self::Pending | Self::Initializing => "warning",
            Self::Moved | Self::Deactivated => "error",
            Self::Other(_) => "status-neutral",
        }
    }

    /// Whether a proxied CNAME created in this zone will actually serve
    /// traffic. Creating one in a pending zone succeeds and does nothing,
    /// which is worth warning about before the user waits for it.
    pub fn can_serve(&self) -> bool {
        matches!(self, Self::Active)
    }

    pub fn caveat(&self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::Pending => Some(
                "This zone is not using Cloudflare's nameservers yet, so a record \
                 created here will not resolve until it is.",
            ),
            Self::Initializing => Some("This zone is still being set up."),
            Self::Moved => Some("This zone has been moved away from this account."),
            Self::Deactivated => Some("This zone is deactivated and will not serve traffic."),
            Self::Other(_) => Some("This zone reports a status this version does not recognise."),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub status: ZoneStatus,
    pub paused: bool,
    pub account_id: String,
    pub account_name: String,
    /// The permissions the current token has on this zone, when Cloudflare
    /// reports them. Used to explain a 403 before it happens.
    pub permissions: Vec<String>,
}

impl Zone {
    /// Whether `hostname` belongs to this zone.
    pub fn covers(&self, hostname: &str) -> bool {
        let hostname = hostname.trim_end_matches('.').to_ascii_lowercase();
        let zone = self.name.to_ascii_lowercase();
        hostname == zone || hostname.ends_with(&format!(".{zone}"))
    }

    /// Picks the zone that owns a hostname. The longest match wins, so
    /// `a.sub.example.com` goes to `sub.example.com` rather than to
    /// `example.com` when both are on the account.
    pub fn best_match<'z>(zones: &'z [Zone], hostname: &str) -> Option<&'z Zone> {
        zones
            .iter()
            .filter(|zone| zone.covers(hostname))
            .max_by_key(|zone| zone.name.len())
    }

    pub fn has_dns_edit(&self) -> bool {
        self.permissions.is_empty()
            || self
                .permissions
                .iter()
                .any(|permission| permission.starts_with("#dns_records:edit"))
    }

    /// True only when the zone explicitly reports DNS edit permission. An
    /// absent permission list is unknown, not a grant — used when deciding
    /// whether the token *provably* has DNS:Edit without attempting a write.
    pub fn has_explicit_dns_edit(&self) -> bool {
        !self.permissions.is_empty()
            && self
                .permissions
                .iter()
                .any(|permission| permission.starts_with("#dns_records:edit"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(name: &str) -> Zone {
        Zone {
            id: "z".repeat(32),
            name: name.into(),
            status: ZoneStatus::Active,
            paused: false,
            account_id: "a".repeat(32),
            account_name: "Acme".into(),
            permissions: vec![],
        }
    }

    #[test]
    fn covers_the_apex_and_subdomains_only() {
        let zone = zone("example.com");
        assert!(zone.covers("example.com"));
        assert!(zone.covers("app.example.com"));
        assert!(zone.covers("APP.EXAMPLE.COM"));
        assert!(zone.covers("app.example.com."));
        assert!(!zone.covers("notexample.com"));
        assert!(!zone.covers("example.com.evil.test"));
    }

    #[test]
    fn best_match_prefers_the_longer_zone() {
        let zones = vec![zone("example.com"), zone("sub.example.com")];
        assert_eq!(
            Zone::best_match(&zones, "a.sub.example.com").unwrap().name,
            "sub.example.com"
        );
        assert_eq!(
            Zone::best_match(&zones, "a.example.com").unwrap().name,
            "example.com"
        );
        assert!(Zone::best_match(&zones, "elsewhere.test").is_none());
    }

    #[test]
    fn only_an_active_zone_can_serve() {
        assert!(ZoneStatus::Active.can_serve());
        assert!(ZoneStatus::Active.caveat().is_none());
        assert!(!ZoneStatus::Pending.can_serve());
        assert!(ZoneStatus::Pending.caveat().is_some());
    }
}
