//! Registered remote Linux hosts.
//!
//! Everything in this module is optional. A host profile only exists because
//! the user chose to add one, and every screen that mentions one has to work
//! when there are none. The types here therefore describe a host that *may*
//! be reachable, not one that is.
//!
//! No secret lives in this struct. The private key stays where the user put
//! it, referenced by path; an optional passphrase lives in the OS credential
//! store under a key derived from the host's ID. The struct itself is safe to
//! write to SQLite and safe to include in a diagnostic export.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// How to authenticate to a host.
///
/// Password authentication is deliberately absent. It encourages storing a
/// reusable server password in an application database, it cannot be
/// restricted by the server the way an authorized-keys entry can, and every
/// deployment target for this application already has SSH keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RemoteAuth {
    /// Use whatever `ssh-agent` is offering. Nothing is stored and nothing is
    /// read from disk, which makes this the right default.
    Agent,
    /// Use an existing private key file, in place. The file is never copied
    /// into the application's data directory.
    KeyFile {
        path: PathBuf,
        /// Whether a passphrase for this key is filed in the credential store.
        /// The passphrase itself is not here.
        has_passphrase: bool,
    },
}

impl RemoteAuth {
    pub fn label(&self) -> String {
        match self {
            Self::Agent => "ssh-agent".into(),
            Self::KeyFile { path, .. } => {
                format!("Key file · {}", path.display())
            }
        }
    }

    pub fn is_agent(&self) -> bool {
        matches!(self, Self::Agent)
    }
}

/// The default systemd unit `cloudflared service install` creates.
pub const DEFAULT_SERVICE_UNIT: &str = "cloudflared.service";

/// The transport a cloudflared connector uses to reach Cloudflare's edge.
///
/// This is a property of the *connector*, not of the tunnel: Cloudflare's API
/// does not store it, and every machine running a connector picks its own. It
/// is pinned through a `TUNNEL_TRANSPORT_PROTOCOL` environment variable in a
/// systemd drop-in, which is the only place this application reads or writes
/// it. `None` — not represented here — means "let cloudflared choose", which
/// today is QUIC with an automatic fallback.
///
/// The usual reason to pin `http2` is a network where long-lived UDP flows
/// stall: the tunnel looks degraded, POST bodies arrive truncated, and
/// requests that never complete vanish before the origin sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportProtocol {
    /// QUIC over UDP. The default, and the faster of the two on a network
    /// that carries UDP well.
    Quic,
    /// HTTP/2 over TCP. The fallback, and the fix when UDP flows are being
    /// dropped or stalled somewhere between the connector and the edge.
    Http2,
}

impl TransportProtocol {
    /// The value `TUNNEL_TRANSPORT_PROTOCOL` accepts.
    pub fn env_value(self) -> &'static str {
        match self {
            Self::Quic => "quic",
            Self::Http2 => "http2",
        }
    }

    /// Finds a `TUNNEL_TRANSPORT_PROTOCOL=` assignment in `systemctl show
    /// --property=Environment` output, whatever else is on the line — and in
    /// the drop-in file itself, where the same assignment wears systemd's
    /// `Environment=` prefix.
    pub fn from_env_output(output: &str) -> Option<Self> {
        let needle = "TUNNEL_TRANSPORT_PROTOCOL=";
        let value = output
            .split_whitespace()
            .map(|word| word.strip_prefix("Environment=").unwrap_or(word))
            .find(|word| word.starts_with(needle))
            .map(|word| &word[needle.len()..])?;
        match value.trim() {
            "quic" => Some(Self::Quic),
            "http2" | "h2" => Some(Self::Http2),
            _ => None,
        }
    }

    /// A short display label. The dropdown in an `AdwComboRow` gives its
    /// value only a narrow strip beside the arrow, and the longer
    /// "QUIC (UDP)" / "HTTP/2 (TCP)" labels were ellipsized; the subtitle
    /// carries the transport detail instead.
    pub fn label(self) -> &'static str {
        match self {
            Self::Quic => "QUIC",
            Self::Http2 => "HTTP/2",
        }
    }

    /// The one-line case for or against, shown under the choice.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::Quic => {
                "cloudflared's default. Best throughput on a network that carries \
                 long-lived UDP flows well."
            }
            Self::Http2 => {
                "Runs the tunnel over TCP instead. The fix when UDP flows stall: \
                 stalled uploads, requests that never arrive at the origin, \
                 intermittent degradation on one connection."
            }
        }
    }

    /// The drop-in file this application manages for a unit. Its name names
    /// the owner, so a system administrator reading `/etc/systemd/system`
    /// knows exactly what wrote it and what removing it undoes.
    pub const DROP_IN_NAME: &'static str = "tunnel-manager-protocol";

    /// `/etc/systemd/system/<unit>.d/tunnel-manager-protocol.conf`
    pub fn drop_in_path(unit: &str) -> Result<String, crate::domain::validation::ValidationError> {
        let unit = crate::domain::validation::systemd_unit(unit)?;
        Ok(format!(
            "/etc/systemd/system/{unit}.d/{}.conf",
            Self::DROP_IN_NAME
        ))
    }

    /// `/etc/systemd/system/<unit>.d`
    pub fn drop_in_dir(unit: &str) -> Result<String, crate::domain::validation::ValidationError> {
        let unit = crate::domain::validation::systemd_unit(unit)?;
        Ok(format!("/etc/systemd/system/{unit}.d"))
    }

    /// The drop-in file's content. Not a secret, but it is installed from
    /// stdin like every file this application writes, so it never appears in
    /// a process listing.
    pub fn drop_in_content(self) -> String {
        format!(
            "# Managed by Tunnel Manager. Remove this file to let cloudflared\n\
             # choose its own transport again.\n\
             [Service]\n\
             Environment=TUNNEL_TRANSPORT_PROTOCOL={}\n",
            self.env_value()
        )
    }
}

/// A saved host profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteHost {
    pub id: i64,
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub auth: RemoteAuth,
    /// The host key this host is pinned to, in OpenSSH's `SHA256:…` form.
    ///
    /// `None` means the host has never been connected to. It never means
    /// "accept anything": the first connection stops and shows the fingerprint
    /// for the user to approve, and a later change is an error, not a prompt.
    pub host_key_fingerprint: Option<String>,
    pub service_unit: String,
    /// The tunnel this host is expected to be running a connector for.
    pub tunnel_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: Option<DateTime<Utc>>,
    /// Free-text note, shown in the list.
    pub notes: Option<String>,
}

impl RemoteHost {
    /// `user@host:port`, for a subtitle. Never used to build a command.
    pub fn display_target(&self) -> String {
        if self.port == 22 {
            format!("{}@{}", self.username, self.address)
        } else {
            format!("{}@{}:{}", self.username, self.address, self.port)
        }
    }

    pub fn is_trusted(&self) -> bool {
        self.host_key_fingerprint.is_some()
    }
}

/// A host being added, before it has an ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteHostDraft {
    pub name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub auth: RemoteAuth,
    pub service_unit: String,
    pub tunnel_id: Option<String>,
    pub notes: Option<String>,
}

impl Default for RemoteHostDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            port: 22,
            username: whoami_fallback(),
            auth: RemoteAuth::Agent,
            service_unit: DEFAULT_SERVICE_UNIT.to_string(),
            tunnel_id: None,
            notes: None,
        }
    }
}

/// The local user's name, as a starting value for the SSH username field.
fn whoami_fallback() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

/// What `systemctl is-active` and friends reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RemoteServiceState {
    Active,
    Inactive,
    Failed,
    /// The unit exists but is in a state that is none of the above.
    Other(String),
    /// The unit is not installed on this host.
    NotInstalled,
}

impl RemoteServiceState {
    pub fn from_systemctl(active: &str, load: &str) -> Self {
        if load.trim() == "not-found" {
            return Self::NotInstalled;
        }
        match active.trim() {
            "active" => Self::Active,
            "inactive" => Self::Inactive,
            "failed" => Self::Failed,
            "" => Self::NotInstalled,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Active => "Running",
            Self::Inactive => "Stopped",
            Self::Failed => "Failed",
            Self::NotInstalled => "Not installed",
            Self::Other(value) => value,
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Active => "media-playback-start-symbolic",
            Self::Inactive => "media-playback-stop-symbolic",
            Self::Failed => "dialog-error-symbolic",
            Self::NotInstalled => "package-x-generic-symbolic",
            Self::Other(_) => "dialog-question-symbolic",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Active => "success",
            Self::Inactive => "status-neutral",
            Self::Failed => "error",
            Self::NotInstalled => "warning",
            Self::Other(_) => "status-neutral",
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Active)
    }
}

/// Whether a host answered, and what it said.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum RemoteStatus {
    /// Never contacted since the application started.
    Unknown,
    Checking,
    Reachable {
        service: RemoteServiceState,
        cloudflared_version: Option<String>,
        systemd_available: bool,
        can_control_service: bool,
    },
    /// Contacted and refused, or unreachable. The message is already redacted.
    Unreachable {
        message: String,
    },
}

impl RemoteStatus {
    pub fn label(&self) -> String {
        match self {
            Self::Unknown => "Not checked".into(),
            Self::Checking => "Checking…".into(),
            Self::Reachable { service, .. } => service.label().to_string(),
            Self::Unreachable { .. } => "Unreachable".into(),
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Unknown => "dialog-question-symbolic",
            Self::Checking => "content-loading-symbolic",
            Self::Reachable { service, .. } => service.icon_name(),
            Self::Unreachable { .. } => "network-offline-symbolic",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Unknown | Self::Checking => "status-neutral",
            Self::Reachable { service, .. } => service.css_class(),
            Self::Unreachable { .. } => "error",
        }
    }

    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::Unreachable { .. })
    }

    /// Whether Start / Stop / Restart should be offered at all.
    pub fn can_control(&self) -> bool {
        matches!(
            self,
            Self::Reachable {
                can_control_service: true,
                systemd_available: true,
                ..
            }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_state_reads_systemctl_output() {
        assert_eq!(
            RemoteServiceState::from_systemctl("active", "loaded"),
            RemoteServiceState::Active
        );
        assert_eq!(
            RemoteServiceState::from_systemctl("inactive", "loaded"),
            RemoteServiceState::Inactive
        );
        assert_eq!(
            RemoteServiceState::from_systemctl("failed", "loaded"),
            RemoteServiceState::Failed
        );
        assert_eq!(
            RemoteServiceState::from_systemctl("inactive", "not-found"),
            RemoteServiceState::NotInstalled
        );
        assert_eq!(
            RemoteServiceState::from_systemctl("activating", "loaded"),
            RemoteServiceState::Other("activating".into())
        );
    }

    #[test]
    fn a_host_with_no_pinned_key_is_not_trusted() {
        let host = RemoteHost {
            id: 1,
            name: "edge".into(),
            address: "edge.example.com".into(),
            port: 22,
            username: "deploy".into(),
            auth: RemoteAuth::Agent,
            host_key_fingerprint: None,
            service_unit: DEFAULT_SERVICE_UNIT.into(),
            tunnel_id: None,
            created_at: Utc::now(),
            last_seen_at: None,
            notes: None,
        };
        assert!(!host.is_trusted());
        assert_eq!(host.display_target(), "deploy@edge.example.com");
    }

    #[test]
    fn display_target_shows_a_non_default_port() {
        let mut host = RemoteHost {
            id: 1,
            name: "edge".into(),
            address: "edge.example.com".into(),
            port: 2222,
            username: "deploy".into(),
            auth: RemoteAuth::Agent,
            host_key_fingerprint: Some("SHA256:x".into()),
            service_unit: DEFAULT_SERVICE_UNIT.into(),
            tunnel_id: None,
            created_at: Utc::now(),
            last_seen_at: None,
            notes: None,
        };
        assert_eq!(host.display_target(), "deploy@edge.example.com:2222");
        host.port = 22;
        assert_eq!(host.display_target(), "deploy@edge.example.com");
    }

    #[test]
    fn control_is_offered_only_when_everything_lines_up() {
        assert!(!RemoteStatus::Unknown.can_control());
        assert!(!RemoteStatus::Unreachable {
            message: "no route".into()
        }
        .can_control());
        assert!(!RemoteStatus::Reachable {
            service: RemoteServiceState::Active,
            cloudflared_version: None,
            systemd_available: false,
            can_control_service: true,
        }
        .can_control());
        assert!(!RemoteStatus::Reachable {
            service: RemoteServiceState::Active,
            cloudflared_version: None,
            systemd_available: true,
            can_control_service: false,
        }
        .can_control());
        assert!(RemoteStatus::Reachable {
            service: RemoteServiceState::Active,
            cloudflared_version: Some("2026.1.0".into()),
            systemd_available: true,
            can_control_service: true,
        }
        .can_control());
    }

    #[test]
    fn a_draft_defaults_to_the_agent_and_port_22() {
        let draft = RemoteHostDraft::default();
        assert_eq!(draft.port, 22);
        assert!(draft.auth.is_agent());
        assert_eq!(draft.service_unit, DEFAULT_SERVICE_UNIT);
    }

    #[test]
    fn the_protocol_is_found_wherever_it_sits_in_the_environment() {
        assert_eq!(
            TransportProtocol::from_env_output("TUNNEL_TRANSPORT_PROTOCOL=http2"),
            Some(TransportProtocol::Http2)
        );
        assert_eq!(
            TransportProtocol::from_env_output("FOO=bar TUNNEL_TRANSPORT_PROTOCOL=quic BAZ=quux"),
            Some(TransportProtocol::Quic)
        );
        assert_eq!(
            TransportProtocol::from_env_output("Environment=TUNNEL_TRANSPORT_PROTOCOL=http2"),
            Some(TransportProtocol::Http2),
            "the drop-in file's own line parses too"
        );
        assert_eq!(TransportProtocol::from_env_output("FOO=bar"), None);
        assert_eq!(TransportProtocol::from_env_output(""), None);
        assert_eq!(
            TransportProtocol::from_env_output("TUNNEL_TRANSPORT_PROTOCOL=carrier-pigeon"),
            None
        );
    }

    #[test]
    fn the_drop_in_paths_refuse_a_hostile_unit() {
        assert!(TransportProtocol::drop_in_path("cloudflared.service").is_ok());
        assert_eq!(
            TransportProtocol::drop_in_path("cloudflared.service").unwrap(),
            "/etc/systemd/system/cloudflared.service.d/tunnel-manager-protocol.conf"
        );
        assert!(TransportProtocol::drop_in_path("../../etc/passwd").is_err());
        assert!(TransportProtocol::drop_in_path("cloudflared.service; evil").is_err());
    }

    #[test]
    fn the_drop_in_content_names_the_environment_variable_and_nothing_else() {
        let content = TransportProtocol::Http2.drop_in_content();
        assert!(content.contains("[Service]"));
        assert!(content.contains("TUNNEL_TRANSPORT_PROTOCOL=http2"));
        assert!(content.contains("Tunnel Manager"));
        assert_eq!(
            content.matches("Environment=").count(),
            1,
            "exactly one assignment"
        );
    }
}
