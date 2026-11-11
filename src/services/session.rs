//! Connecting to Cloudflare, and what the application knows once it has.
//!
//! A [`Session`] is the answer to "which account are we working in, with what
//! token, and what can that token do". It owns no widgets and no database
//! handle, so the whole connection flow — token verification, account
//! discovery, zone listing, permission probing — is testable against a mock
//! HTTP server with nothing else running.

use secrecy::SecretString;

use crate::cloudflare::error::{CloudflareError, Result};
use crate::cloudflare::models::TokenStatus;
use crate::cloudflare::{Cancel, CloudflareClient, Timeouts};
use crate::domain::account::{PermissionState, TokenCapabilities};
use crate::domain::{Account, Zone};
use serde::Serialize;

/// Where the application stands with Cloudflare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum ConnectionState {
    /// No token has been provided yet. The onboarding page is shown.
    Disconnected,
    /// A token is being checked.
    Connecting,
    /// A token is live and an account is selected.
    Connected {
        account: Account,
        token: TokenStatus,
        capabilities: TokenCapabilities,
    },
    /// A token exists but something is wrong with it. The message is already
    /// user-facing.
    Failed { message: String, title: String },
}

impl ConnectionState {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }

    pub fn account(&self) -> Option<&Account> {
        match self {
            Self::Connected { account, .. } => Some(account),
            _ => None,
        }
    }
}

/// What a successful token check found out.
#[derive(Debug, Clone, Serialize)]
pub struct TokenCheck {
    pub status: TokenStatus,
    pub accounts: Vec<Account>,
    /// True when `/accounts` was refused, which happens with a token scoped to
    /// one account rather than to the user. The account ID then has to be
    /// entered by hand, which is why that field exists on the onboarding page.
    pub accounts_unavailable: Option<String>,
}

impl TokenCheck {
    /// Whether the user must type an account ID because discovery did not
    /// produce one.
    pub fn needs_manual_account(&self) -> bool {
        self.accounts.is_empty()
    }
}

/// Verifies a token and finds out what it can see.
///
/// Two calls, both cheap, and the second is allowed to fail: an account-scoped
/// token authenticates fine and is refused by `/accounts`, which is a
/// perfectly workable situation as long as the user knows their account ID.
pub async fn check_token(client: &CloudflareClient, cancel: &Cancel) -> Result<TokenCheck> {
    let status = client.verify_token(cancel).await?;

    if !status.is_active() {
        return Err(CloudflareError::Unauthorized {
            errors: vec![],
            operation: crate::cloudflare::Operation::new(
                "use this API token",
                crate::cloudflare::Capability::Any,
            ),
        });
    }

    match client.list_accounts(cancel).await {
        Ok(accounts) => Ok(TokenCheck {
            status,
            accounts,
            accounts_unavailable: None,
        }),
        Err(error) if error.is_cancelled() => Err(error),
        Err(error) => Ok(TokenCheck {
            status,
            accounts: Vec::new(),
            accounts_unavailable: Some(error.user_message()),
        }),
    }
}

/// Builds a client for a token. The base URL is a parameter so tests can point
/// at a local server.
pub fn client_for(token: SecretString, base_url: &str) -> Result<CloudflareClient> {
    CloudflareClient::with_base_url(token, base_url, Timeouts::default())
}

/// The API this run talks to.
///
/// `TUNNEL_MANAGER_API_BASE` overrides it. That exists so the interface can be
/// driven against a fixture server during development and screenshotting —
/// every page, every state, without a real account. It is read once per call
/// and never persisted, so it cannot silently point a normal run somewhere
/// unexpected; and a value that is not `https://` is refused unless it is
/// plainly a loopback address.
pub fn api_base_url() -> String {
    let Ok(override_url) = std::env::var("TUNNEL_MANAGER_API_BASE") else {
        return crate::cloudflare::DEFAULT_BASE_URL.to_string();
    };

    let is_loopback = override_url.starts_with("http://127.0.0.1")
        || override_url.starts_with("http://localhost")
        || override_url.starts_with("http://[::1]");

    if override_url.starts_with("https://") || is_loopback {
        tracing::warn!(
            base = %override_url,
            "TUNNEL_MANAGER_API_BASE is set; talking to a non-default API endpoint"
        );
        override_url
    } else {
        tracing::error!(
            "TUNNEL_MANAGER_API_BASE must be https:// or a loopback address; ignoring it"
        );
        crate::cloudflare::DEFAULT_BASE_URL.to_string()
    }
}

/// A live connection: a client, the chosen account, and the zones it can see.
#[derive(Clone)]
pub struct Session {
    pub client: CloudflareClient,
    pub account: Account,
    pub zones: Vec<Zone>,
    pub default_zone_id: Option<String>,
    pub capabilities: TokenCapabilities,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("account", &self.account.id)
            .field("zones", &self.zones.len())
            .field("default_zone_id", &self.default_zone_id)
            .finish_non_exhaustive()
    }
}

impl Session {
    pub fn new(client: CloudflareClient, account: Account) -> Self {
        Self {
            client,
            account,
            zones: Vec::new(),
            default_zone_id: None,
            capabilities: TokenCapabilities::default(),
        }
    }

    /// Loads the zones the token can see on this account.
    ///
    /// A failure here is recorded as a missing capability rather than
    /// propagated: publishing a hostname needs zones, but listing and
    /// controlling tunnels does not, and a token scoped to tunnels alone is a
    /// legitimate way to run this application.
    pub async fn load_zones(&mut self, cancel: &Cancel) -> Option<String> {
        match self.client.list_zones(Some(&self.account.id), cancel).await {
            Ok(zones) => {
                self.zones = zones;
                self.capabilities.zones.0 = PermissionState::Granted;
                self.capabilities.dns.0 = if !self.zones.is_empty()
                    && self.zones.iter().all(Zone::has_explicit_dns_edit)
                {
                    // Cloudflare reports per-zone permissions on the zone
                    // listing, so DNS:Edit can often be established without
                    // attempting a write. An absent permission list is
                    // unknown, not a grant, so it leaves the state Unknown
                    // and the first write proves it one way or the other.
                    PermissionState::Granted
                } else {
                    PermissionState::Unknown
                };
                None
            }
            Err(error) => {
                if error.is_permission_problem() {
                    self.capabilities.zones.0 = PermissionState::Denied;
                    self.capabilities.dns.0 = PermissionState::Denied;
                }
                Some(error.user_message())
            }
        }
    }

    /// The zone that owns a hostname, if the account has one.
    pub fn zone_for(&self, hostname: &str) -> Option<&Zone> {
        Zone::best_match(&self.zones, hostname)
    }

    pub fn default_zone(&self) -> Option<&Zone> {
        let id = self.default_zone_id.as_deref()?;
        self.zones.iter().find(|zone| zone.id == id)
    }

    /// Whether hostname publishing can be offered at all.
    pub fn can_publish_hostnames(&self) -> bool {
        !self.zones.is_empty()
    }

    /// Records that a tunnel operation succeeded, which is the only reliable
    /// evidence the token really has Tunnel:Edit.
    pub fn note_tunnel_access(&mut self, granted: bool) {
        self.capabilities.tunnels.0 = if granted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        };
    }

    pub fn note_dns_access(&mut self, granted: bool) {
        self.capabilities.dns.0 = if granted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::zone::ZoneStatus;

    fn zone(name: &str, permissions: Vec<String>) -> Zone {
        Zone {
            id: "z".repeat(32),
            name: name.into(),
            status: ZoneStatus::Active,
            paused: false,
            account_id: "a".repeat(32),
            account_name: "Acme".into(),
            permissions,
        }
    }

    fn session() -> Session {
        Session::new(
            CloudflareClient::with_base_url(
                SecretString::from("t".repeat(40)),
                "https://example.test/client/v4",
                Timeouts::default(),
            )
            .unwrap(),
            Account {
                id: "a".repeat(32),
                name: "Acme".into(),
            },
        )
    }

    #[test]
    fn a_new_session_claims_no_capabilities() {
        let session = session();
        assert!(!session.capabilities.is_complete());
        assert!(!session.capabilities.has_known_gap());
        assert!(!session.can_publish_hostnames());
    }

    #[test]
    fn zone_matching_prefers_the_longest_zone() {
        let mut session = session();
        session.zones = vec![
            zone("example.com", vec![]),
            Zone {
                id: "y".repeat(32),
                ..zone("sub.example.com", vec![])
            },
        ];

        assert_eq!(
            session.zone_for("app.sub.example.com").unwrap().name,
            "sub.example.com"
        );
        assert!(session.zone_for("elsewhere.test").is_none());
        assert!(session.can_publish_hostnames());
    }

    #[test]
    fn the_default_zone_resolves_only_when_it_is_still_present() {
        let mut session = session();
        session.zones = vec![zone("example.com", vec![])];
        session.default_zone_id = Some("z".repeat(32));
        assert_eq!(session.default_zone().unwrap().name, "example.com");

        session.default_zone_id = Some("gone".into());
        assert!(
            session.default_zone().is_none(),
            "a zone removed from the account must not resolve"
        );
    }

    #[test]
    fn noting_access_moves_a_capability_off_unknown() {
        let mut session = session();
        assert_eq!(session.capabilities.tunnels.0, PermissionState::Unknown);

        session.note_tunnel_access(true);
        assert_eq!(session.capabilities.tunnels.0, PermissionState::Granted);

        session.note_dns_access(false);
        assert_eq!(session.capabilities.dns.0, PermissionState::Denied);
        assert!(session.capabilities.has_known_gap());
    }

    #[test]
    fn connection_state_exposes_the_account_only_when_connected() {
        assert!(ConnectionState::Disconnected.account().is_none());
        assert!(ConnectionState::Connecting.account().is_none());
        assert!(!ConnectionState::Disconnected.is_connected());

        let connected = ConnectionState::Connected {
            account: Account {
                id: "a".repeat(32),
                name: "Acme".into(),
            },
            token: TokenStatus {
                id: None,
                status: "active".into(),
                expires_on: None,
            },
            capabilities: TokenCapabilities::default(),
        };
        assert!(connected.is_connected());
        assert_eq!(connected.account().unwrap().name, "Acme");
    }

    #[test]
    fn a_token_check_with_no_accounts_asks_for_one_by_hand() {
        let check = TokenCheck {
            status: TokenStatus {
                id: None,
                status: "active".into(),
                expires_on: None,
            },
            accounts: vec![],
            accounts_unavailable: Some("The API token is not allowed to list accounts.".into()),
        };
        assert!(check.needs_manual_account());
    }
}
