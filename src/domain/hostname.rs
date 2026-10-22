//! Public hostnames, the origin services behind them, and the DNS record that
//! has to exist for either to matter.
//!
//! A hostname served through a tunnel is really two facts in two places: an
//! ingress rule in the tunnel's configuration, and a proxied CNAME in the
//! zone. Either can exist without the other, and the interesting states are
//! exactly the ones where they disagree — so [`DnsState`] enumerates the
//! disagreements rather than reducing them to a boolean.

use crate::domain::config::OriginRequest;
use crate::domain::validation::{self, ValidationError, ValidationResult};

/// The schemes Cloudflare accepts in an ingress rule's `service` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginScheme {
    Http,
    Https,
    Tcp,
    Ssh,
    Rdp,
    Unix,
    UnixTls,
    Smb,
    /// `http_status:404` and friends — a rule that answers rather than
    /// proxying. This is what the required catch-all rule uses.
    HttpStatus,
    /// `bastion`, which takes its destination from the request rather than
    /// from the configuration.
    Bastion,
    /// cloudflared's built-in test origin.
    HelloWorld,
}

impl OriginScheme {
    pub const SELECTABLE: &'static [OriginScheme] = &[
        Self::Http,
        Self::Https,
        Self::Tcp,
        Self::Ssh,
        Self::Rdp,
        Self::Smb,
        Self::Unix,
        Self::UnixTls,
        Self::HttpStatus,
    ];

    /// The prefix as it is written in the `service` string, including the
    /// separator.
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Http => "http://",
            Self::Https => "https://",
            Self::Tcp => "tcp://",
            Self::Ssh => "ssh://",
            Self::Rdp => "rdp://",
            Self::Unix => "unix:",
            Self::UnixTls => "unix+tls:",
            Self::Smb => "smb://",
            Self::HttpStatus => "http_status:",
            Self::Bastion => "bastion",
            Self::HelloWorld => "hello_world",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Http => "HTTP",
            Self::Https => "HTTPS",
            Self::Tcp => "TCP",
            Self::Ssh => "SSH",
            Self::Rdp => "RDP",
            Self::Unix => "Unix socket",
            Self::UnixTls => "Unix socket (TLS)",
            Self::Smb => "SMB",
            Self::HttpStatus => "Fixed HTTP status",
            Self::Bastion => "Bastion",
            Self::HelloWorld => "Built-in test page",
        }
    }

    /// What the second field of the editor means for this scheme.
    pub fn target_kind(self) -> TargetKind {
        match self {
            Self::Http | Self::Https | Self::Tcp | Self::Ssh | Self::Rdp | Self::Smb => {
                TargetKind::HostPort
            }
            Self::Unix | Self::UnixTls => TargetKind::SocketPath,
            Self::HttpStatus => TargetKind::StatusCode,
            Self::Bastion | Self::HelloWorld => TargetKind::None,
        }
    }

    /// The port to offer when the user picks this scheme and has not typed
    /// one. `None` means the field starts empty.
    pub fn default_port(self) -> Option<u16> {
        match self {
            Self::Http => Some(80),
            Self::Https => Some(443),
            Self::Ssh => Some(22),
            Self::Rdp => Some(3389),
            Self::Smb => Some(445),
            _ => None,
        }
    }

    /// Whether an HTTP path prefix means anything for this scheme. A `path`
    /// on a `tcp://` rule is silently ignored by cloudflared.
    pub fn supports_path(self) -> bool {
        matches!(self, Self::Http | Self::Https | Self::HttpStatus)
    }
}

/// What the origin's second field holds, which decides how the editor labels
/// and validates it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    HostPort,
    SocketPath,
    StatusCode,
    None,
}

/// A parsed `service` value from an ingress rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginService {
    pub scheme: OriginScheme,
    /// The host for a `host:port` scheme, the path for a socket, the code for
    /// `http_status:`, and empty for the schemes that take no target.
    pub host: String,
    pub port: Option<u16>,
}

impl OriginService {
    pub fn http(host: impl Into<String>, port: u16) -> Self {
        Self {
            scheme: OriginScheme::Http,
            host: host.into(),
            port: Some(port),
        }
    }

    /// The catch-all rule every ingress list must end with.
    pub fn catch_all() -> Self {
        Self {
            scheme: OriginScheme::HttpStatus,
            host: "404".into(),
            port: None,
        }
    }

    /// Parses a `service` string as cloudflared would read it.
    ///
    /// Unknown schemes are an error rather than a silent passthrough, because
    /// this is used to decide whether a rule can be shown in the editor at
    /// all — and the caller keeps the raw string either way, so nothing is
    /// lost by refusing to guess.
    pub fn parse(value: &str) -> ValidationResult<Self> {
        let value = value.trim();
        if value.is_empty() {
            return Err(ValidationError::new("An ingress rule needs a service."));
        }

        if value == "bastion" {
            return Ok(Self {
                scheme: OriginScheme::Bastion,
                host: String::new(),
                port: None,
            });
        }
        if value == "hello_world" || value == "hello-world" {
            return Ok(Self {
                scheme: OriginScheme::HelloWorld,
                host: String::new(),
                port: None,
            });
        }

        if let Some(code) = value.strip_prefix("http_status:") {
            let code = code.trim();
            let parsed: u16 = code.parse().map_err(|_| {
                ValidationError::new("A fixed status must be a number, such as 404.")
            })?;
            if !(100..=599).contains(&parsed) {
                return Err(ValidationError::new(
                    "An HTTP status code is between 100 and 599.",
                ));
            }
            return Ok(Self {
                scheme: OriginScheme::HttpStatus,
                host: parsed.to_string(),
                port: None,
            });
        }

        // The two socket schemes take a bare path, not an authority, so they
        // are matched before the `://` family. cloudflared accepts both
        // `unix:/run/app.sock` and `unix:///run/app.sock`; normalise to the
        // first, which is what its own docs use.
        for (prefix, scheme) in [
            ("unix+tls:", OriginScheme::UnixTls),
            ("unix:", OriginScheme::Unix),
        ] {
            if let Some(rest) = value.strip_prefix(prefix) {
                let path = rest.strip_prefix("//").unwrap_or(rest);
                let path = if path.starts_with('/') {
                    path.to_string()
                } else {
                    format!("/{path}")
                };
                validation::absolute_path(&path)?;
                return Ok(Self {
                    scheme,
                    host: path,
                    port: None,
                });
            }
        }

        for (prefix, scheme) in [
            ("https://", OriginScheme::Https),
            ("http://", OriginScheme::Http),
            ("tcp://", OriginScheme::Tcp),
            ("ssh://", OriginScheme::Ssh),
            ("rdp://", OriginScheme::Rdp),
            ("smb://", OriginScheme::Smb),
        ] {
            if let Some(rest) = value.strip_prefix(prefix) {
                let authority = rest.split('/').next().unwrap_or_default();
                let (host, port) = split_host_port(authority)?;
                return Ok(Self { scheme, host, port });
            }
        }

        Err(ValidationError::new(format!(
            "“{value}” does not name a service. Start it with http://, https://, \
             tcp://, ssh://, rdp://, smb://, unix:, unix+tls: or http_status:."
        )))
    }

    /// Renders back to the `service` string Cloudflare stores.
    pub fn to_service_string(&self) -> String {
        match self.scheme.target_kind() {
            TargetKind::None => self.scheme.prefix().to_string(),
            TargetKind::StatusCode | TargetKind::SocketPath => {
                format!("{}{}", self.scheme.prefix(), self.host)
            }
            TargetKind::HostPort => match self.port {
                Some(port) => format!("{}{}:{port}", self.scheme.prefix(), self.host),
                None => format!("{}{}", self.scheme.prefix(), self.host),
            },
        }
    }

    /// Builds one from the wizard's separate fields, validating as it goes.
    pub fn from_fields(scheme: OriginScheme, target: &str, port: &str) -> ValidationResult<Self> {
        match scheme.target_kind() {
            TargetKind::None => Ok(Self {
                scheme,
                host: String::new(),
                port: None,
            }),
            TargetKind::StatusCode => {
                let code: u16 = target.trim().parse().map_err(|_| {
                    ValidationError::new("A fixed status must be a number, such as 404.")
                })?;
                if !(100..=599).contains(&code) {
                    return Err(ValidationError::new(
                        "An HTTP status code is between 100 and 599.",
                    ));
                }
                Ok(Self {
                    scheme,
                    host: code.to_string(),
                    port: None,
                })
            }
            TargetKind::SocketPath => Ok(Self {
                scheme,
                host: validation::absolute_path(target)?,
                port: None,
            }),
            TargetKind::HostPort => {
                let host = validation::origin_host(target)?;
                let port = validation::origin_port(port)?;
                if port.is_none() && scheme.default_port().is_none() {
                    return Err(ValidationError::new("This kind of origin needs a port."));
                }
                Ok(Self {
                    scheme,
                    host,
                    port: port.or_else(|| scheme.default_port()),
                })
            }
        }
    }

    pub fn is_catch_all(&self) -> bool {
        self.scheme == OriginScheme::HttpStatus
    }
}

fn split_host_port(authority: &str) -> ValidationResult<(String, Option<u16>)> {
    if authority.is_empty() {
        return Err(ValidationError::new("A service needs a host."));
    }

    // Bracketed IPv6, with or without a port.
    if let Some(rest) = authority.strip_prefix('[') {
        let Some((address, tail)) = rest.split_once(']') else {
            return Err(ValidationError::new(
                "An IPv6 origin must close its bracket, as [::1]:8080.",
            ));
        };
        let port = match tail.strip_prefix(':') {
            Some(port) => Some(
                port.parse::<u16>()
                    .map_err(|_| ValidationError::new("A port is a number from 1 to 65535."))?,
            ),
            None => None,
        };
        return Ok((format!("[{address}]"), port));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port
                .parse::<u16>()
                .map_err(|_| ValidationError::new("A port is a number from 1 to 65535."))?;
            Ok((host.to_ascii_lowercase(), Some(port)))
        }
        None => Ok((authority.to_ascii_lowercase(), None)),
    }
}

/// A DNS record, as far as this application cares about one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnsRecord {
    pub id: String,
    pub zone_id: String,
    pub name: String,
    pub record_type: String,
    pub content: String,
    pub proxied: bool,
    pub ttl: u32,
    pub comment: Option<String>,
}

impl DnsRecord {
    /// Whether this record is the proxied CNAME that serves `tunnel_id`.
    ///
    /// This is the check that gates deletion. It has to be exact: a hostname
    /// may have several records, and only the one pointing at *this* tunnel is
    /// ours to remove.
    pub fn is_tunnel_cname(&self, tunnel_id: &str) -> bool {
        self.record_type.eq_ignore_ascii_case("CNAME")
            && self
                .content
                .trim_end_matches('.')
                .eq_ignore_ascii_case(&format!("{tunnel_id}.cfargotunnel.com"))
    }

    /// Whether this record points at *some* tunnel, ours or not.
    pub fn is_any_tunnel_cname(&self) -> bool {
        self.record_type.eq_ignore_ascii_case("CNAME")
            && self
                .content
                .trim_end_matches('.')
                .to_ascii_lowercase()
                .ends_with(".cfargotunnel.com")
    }

    /// Whether this record would stop a tunnel CNAME from working if it stayed.
    pub fn conflicts_with_tunnel(&self) -> bool {
        matches!(
            self.record_type.to_ascii_uppercase().as_str(),
            "A" | "AAAA" | "CNAME"
        )
    }
}

/// How the zone's DNS compares with what the tunnel needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DnsState {
    /// A proxied CNAME to this tunnel exists. Nothing to do.
    Correct { record_id: String },
    /// The right CNAME exists but is not proxied, so traffic bypasses the
    /// tunnel and the hostname does not resolve to anything useful.
    NotProxied { record_id: String },
    /// Nothing points the hostname anywhere.
    Missing,
    /// An A, AAAA or CNAME record exists that is not this tunnel's.
    Conflicting {
        record_id: String,
        record_type: String,
        content: String,
    },
    /// The zone could not be read — usually a token without Zone:Read, or a
    /// hostname whose zone is not on this account.
    Unknown { reason: String },
}

impl DnsState {
    /// Works out the state from every record the zone returned for the
    /// hostname.
    ///
    /// Ordering matters. A correct record wins even if other records exist
    /// alongside it, because at that point the hostname *does* work and the
    /// extra records are the user's business, not ours.
    pub fn evaluate(records: &[DnsRecord], tunnel_id: &str) -> Self {
        if let Some(record) = records
            .iter()
            .find(|record| record.is_tunnel_cname(tunnel_id))
        {
            return if record.proxied {
                Self::Correct {
                    record_id: record.id.clone(),
                }
            } else {
                Self::NotProxied {
                    record_id: record.id.clone(),
                }
            };
        }

        if let Some(record) = records.iter().find(|record| record.conflicts_with_tunnel()) {
            return Self::Conflicting {
                record_id: record.id.clone(),
                record_type: record.record_type.clone(),
                content: record.content.clone(),
            };
        }

        Self::Missing
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Correct { .. } => "DNS ready",
            Self::NotProxied { .. } => "Not proxied",
            Self::Missing => "No DNS record",
            Self::Conflicting { .. } => "Conflicting record",
            Self::Unknown { .. } => "DNS not checked",
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Correct { .. } => "object-select-symbolic",
            Self::NotProxied { .. } => "dialog-warning-symbolic",
            Self::Missing => "list-add-symbolic",
            Self::Conflicting { .. } => "dialog-error-symbolic",
            Self::Unknown { .. } => "dialog-question-symbolic",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Correct { .. } => "success",
            Self::NotProxied { .. } | Self::Missing => "warning",
            Self::Conflicting { .. } => "error",
            Self::Unknown { .. } => "status-neutral",
        }
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self, Self::Correct { .. })
    }

    /// Whether the Repair action can fix this without asking anything further.
    pub fn is_repairable(&self) -> bool {
        matches!(self, Self::Missing | Self::NotProxied { .. })
    }

    /// The sentence shown under the badge.
    pub fn detail(&self, tunnel_name: &str) -> String {
        match self {
            Self::Correct { .. } => {
                format!("A proxied CNAME points this hostname at “{tunnel_name}”.")
            }
            Self::NotProxied { .. } => {
                "The CNAME exists but is not proxied, so requests never reach the \
                 tunnel. Repairing turns the proxy on."
                    .into()
            }
            Self::Missing => "No DNS record points this hostname anywhere. Repairing creates the \
                 proxied CNAME."
                .into(),
            Self::Conflicting {
                record_type,
                content,
                ..
            } => format!(
                "An existing {record_type} record points at {content}. It has to be \
                 removed or changed before the tunnel can serve this hostname — this \
                 application will not delete a record it did not create."
            ),
            Self::Unknown { reason } => reason.clone(),
        }
    }
}

/// One hostname served through a tunnel, joined with what DNS says about it.
///
/// Not `Eq`: the origin behaviour can carry arbitrary preserved JSON, and
/// `serde_json::Value` holds floats.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicHostname {
    pub hostname: String,
    pub path: Option<String>,
    pub service: OriginService,
    /// The raw `service` string, kept so a rule this build cannot parse is
    /// still shown accurately rather than being reformatted into a guess.
    pub raw_service: String,
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub zone_id: Option<String>,
    pub zone_name: Option<String>,
    pub dns: DnsState,
    /// How cloudflared talks to the origin for this rule — the dashboard's
    /// "Additional application settings". `None` means the rule carries no
    /// `originRequest` and cloudflared uses its defaults.
    pub origin_request: Option<OriginRequest>,
    /// The rule's index in the tunnel's ingress list, which is what identifies
    /// it for an edit or a removal — hostnames may repeat with different
    /// paths.
    pub rule_index: usize,
    /// Whether the user has disabled this hostname: the rule is still in the
    /// tunnel's configuration, but its service is `http_status:404`, so
    /// Cloudflare answers 404 and the origin is never contacted. The origin
    /// it served is remembered locally for Enable.
    pub disabled: bool,
}

impl PublicHostname {
    /// The label shown in a list: the hostname, plus the path when there is
    /// one, because `app.example.com` twice over is not a useful list.
    pub fn display_name(&self) -> String {
        match &self.path {
            Some(path) => format!("{}{}", self.hostname, path),
            None => self.hostname.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(record_type: &str, content: &str, proxied: bool) -> DnsRecord {
        DnsRecord {
            id: format!("rec-{record_type}"),
            zone_id: "z".repeat(32),
            name: "app.example.com".into(),
            record_type: record_type.into(),
            content: content.into(),
            proxied,
            ttl: 1,
            comment: None,
        }
    }

    const TUNNEL: &str = "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415";

    #[test]
    fn parses_every_documented_scheme() {
        let cases: &[(&str, OriginScheme)] = &[
            ("http://localhost:8080", OriginScheme::Http),
            ("https://10.0.0.5:8443", OriginScheme::Https),
            ("tcp://localhost:2222", OriginScheme::Tcp),
            ("ssh://localhost:22", OriginScheme::Ssh),
            ("rdp://localhost:3389", OriginScheme::Rdp),
            ("smb://localhost:445", OriginScheme::Smb),
            ("unix:/run/app.sock", OriginScheme::Unix),
            ("unix+tls:/run/app.sock", OriginScheme::UnixTls),
            ("http_status:404", OriginScheme::HttpStatus),
            ("bastion", OriginScheme::Bastion),
            ("hello_world", OriginScheme::HelloWorld),
        ];

        for (input, scheme) in cases {
            let parsed =
                OriginService::parse(input).unwrap_or_else(|error| panic!("{input}: {error}"));
            assert_eq!(parsed.scheme, *scheme, "{input}");
        }
    }

    #[test]
    fn round_trips_a_service_string() {
        for input in [
            "http://localhost:8080",
            "https://10.0.0.5:8443",
            "tcp://localhost:2222",
            "unix:/run/app.sock",
            "unix+tls:/run/app.sock",
            "http_status:404",
        ] {
            let parsed = OriginService::parse(input).unwrap();
            assert_eq!(parsed.to_service_string(), input, "{input}");
        }
    }

    #[test]
    fn normalises_a_unix_socket_written_with_slashes() {
        let parsed = OriginService::parse("unix:///run/app.sock").unwrap();
        assert_eq!(parsed.to_service_string(), "unix:/run/app.sock");
    }

    #[test]
    fn parses_a_bracketed_ipv6_origin() {
        let parsed = OriginService::parse("http://[2001:db8::1]:8080").unwrap();
        assert_eq!(parsed.host, "[2001:db8::1]");
        assert_eq!(parsed.port, Some(8080));
        assert_eq!(parsed.to_service_string(), "http://[2001:db8::1]:8080");
    }

    #[test]
    fn rejects_an_unknown_scheme_rather_than_guessing() {
        assert!(OriginService::parse("gopher://localhost:70").is_err());
        assert!(OriginService::parse("localhost:8080").is_err());
        assert!(OriginService::parse("").is_err());
    }

    #[test]
    fn rejects_an_out_of_range_status() {
        assert!(OriginService::parse("http_status:99").is_err());
        assert!(OriginService::parse("http_status:600").is_err());
        assert!(OriginService::parse("http_status:none").is_err());
    }

    #[test]
    fn from_fields_supplies_the_default_port() {
        let service = OriginService::from_fields(OriginScheme::Http, "localhost", "").unwrap();
        assert_eq!(service.port, Some(80));
        assert_eq!(service.to_service_string(), "http://localhost:80");
    }

    #[test]
    fn from_fields_requires_a_port_where_there_is_no_default() {
        assert!(OriginService::from_fields(OriginScheme::Tcp, "localhost", "").is_err());
        assert!(OriginService::from_fields(OriginScheme::Tcp, "localhost", "9000").is_ok());
    }

    #[test]
    fn catch_all_is_a_fixed_404() {
        assert_eq!(
            OriginService::catch_all().to_service_string(),
            "http_status:404"
        );
        assert!(OriginService::catch_all().is_catch_all());
    }

    #[test]
    fn recognises_only_this_tunnels_cname() {
        let ours = record("CNAME", &format!("{TUNNEL}.cfargotunnel.com"), true);
        let theirs = record("CNAME", "other-id.cfargotunnel.com", true);

        assert!(ours.is_tunnel_cname(TUNNEL));
        assert!(!theirs.is_tunnel_cname(TUNNEL));
        assert!(theirs.is_any_tunnel_cname());
    }

    #[test]
    fn recognises_a_cname_written_with_a_trailing_dot() {
        let dotted = record("CNAME", &format!("{TUNNEL}.cfargotunnel.com."), true);
        assert!(dotted.is_tunnel_cname(TUNNEL));
    }

    #[test]
    fn dns_state_correct_when_the_proxied_cname_exists() {
        let records = vec![record("CNAME", &format!("{TUNNEL}.cfargotunnel.com"), true)];
        assert!(matches!(
            DnsState::evaluate(&records, TUNNEL),
            DnsState::Correct { .. }
        ));
    }

    #[test]
    fn dns_state_flags_an_unproxied_cname() {
        let records = vec![record(
            "CNAME",
            &format!("{TUNNEL}.cfargotunnel.com"),
            false,
        )];
        let state = DnsState::evaluate(&records, TUNNEL);
        assert!(matches!(state, DnsState::NotProxied { .. }));
        assert!(state.is_repairable());
    }

    #[test]
    fn dns_state_missing_when_only_unrelated_types_exist() {
        let records = vec![record("TXT", "v=spf1 -all", false)];
        assert_eq!(DnsState::evaluate(&records, TUNNEL), DnsState::Missing);
    }

    #[test]
    fn dns_state_reports_a_conflicting_a_record() {
        let records = vec![record("A", "203.0.113.10", true)];
        let state = DnsState::evaluate(&records, TUNNEL);
        match &state {
            DnsState::Conflicting {
                record_type,
                content,
                ..
            } => {
                assert_eq!(record_type, "A");
                assert_eq!(content, "203.0.113.10");
            }
            other => panic!("expected a conflict, got {other:?}"),
        }
        assert!(!state.is_repairable());
    }

    #[test]
    fn dns_state_reports_a_cname_to_another_tunnel_as_a_conflict() {
        let records = vec![record("CNAME", "other-id.cfargotunnel.com", true)];
        assert!(matches!(
            DnsState::evaluate(&records, TUNNEL),
            DnsState::Conflicting { .. }
        ));
    }

    #[test]
    fn a_correct_record_wins_over_an_unrelated_one() {
        let records = vec![
            record("TXT", "hello", false),
            record("CNAME", &format!("{TUNNEL}.cfargotunnel.com"), true),
        ];
        assert!(DnsState::evaluate(&records, TUNNEL).is_healthy());
    }

    #[test]
    fn every_dns_state_has_a_distinct_icon() {
        let states = [
            DnsState::Correct {
                record_id: "1".into(),
            },
            DnsState::NotProxied {
                record_id: "1".into(),
            },
            DnsState::Missing,
            DnsState::Conflicting {
                record_id: "1".into(),
                record_type: "A".into(),
                content: "1.2.3.4".into(),
            },
            DnsState::Unknown {
                reason: "no".into(),
            },
        ];
        let mut icons: Vec<&str> = states.iter().map(DnsState::icon_name).collect();
        icons.sort_unstable();
        let before = icons.len();
        icons.dedup();
        assert_eq!(icons.len(), before);
    }
}
