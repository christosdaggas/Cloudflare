//! The data shapes the Tauri commands accept and return.
//!
//! Field names are the Rust field names — this is a private IPC surface, and
//! mirroring them exactly is one less translation that can go wrong. The one
//! exception is [`crate::domain::config::OriginRequest`], whose field names are
//! Cloudflare's wire format (`connectTimeout`, `noTLSVerify`, …) because that
//! document is written straight back to the API.

use serde::{Deserialize, Serialize};

use crate::cloudflare::error::CloudflareError;
use crate::domain::hostname::OriginService;
use crate::domain::validation::ValidationError;
use crate::domain::Account;
use crate::remote::RemoteError;
use crate::services::session::TokenCheck;
use crate::storage::secrets::SecretError;
use crate::storage::StorageError;

/// A command failure, in a shape the frontend can branch on.
#[derive(Debug, Clone, Serialize)]
pub struct CommandError {
    /// A stable machine-readable kind, such as `not_connected`,
    /// `host_key_unknown` or `validation`.
    pub kind: &'static str,
    pub title: &'static str,
    /// Already user-facing and already redacted.
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Box<serde_json::Value>>,
}

impl CommandError {
    pub fn new(kind: &'static str, title: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            title,
            message: message.into(),
            data: None,
        }
    }

    pub fn not_connected() -> Self {
        Self::new(
            "not_connected",
            "Not connected",
            "Not connected to Cloudflare. Add an API token on the Connection page.",
        )
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", "Something went wrong", message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new("validation", "Check the details", message)
    }
}

impl From<CloudflareError> for CommandError {
    fn from(error: CloudflareError) -> Self {
        let kind = match &error {
            CloudflareError::NotConnected => "not_connected",
            CloudflareError::Offline { .. } | CloudflareError::Transport { .. } => "network",
            CloudflareError::Timeout { .. } => "timeout",
            CloudflareError::Cancelled => "cancelled",
            CloudflareError::RateLimited { .. } => "rate_limited",
            CloudflareError::Unauthorized { .. } => "unauthorized",
            CloudflareError::Forbidden { .. } => "permission",
            CloudflareError::NotFound { .. } => "not_found",
            CloudflareError::Invalid(_) => "validation",
            CloudflareError::Truncated { .. } => "truncated",
            CloudflareError::Api { .. } | CloudflareError::Decode { .. } => "cloudflare",
        };
        Self {
            kind,
            title: error.title(),
            message: error.user_message(),
            data: None,
        }
    }
}

impl From<RemoteError> for CommandError {
    fn from(error: RemoteError) -> Self {
        let (kind, data) = match &error {
            RemoteError::HostKeyUnknown {
                fingerprint,
                algorithm,
            } => (
                "host_key_unknown",
                Some(Box::new(serde_json::json!({
                    "fingerprint": fingerprint,
                    "algorithm": algorithm,
                }))),
            ),
            RemoteError::HostKeyMismatch {
                expected,
                actual,
                algorithm,
            } => (
                "host_key_mismatch",
                Some(Box::new(serde_json::json!({
                    "expected": expected,
                    "actual": actual,
                    "algorithm": algorithm,
                }))),
            ),
            RemoteError::NotConfigured => ("not_configured", None),
            RemoteError::Cancelled => ("cancelled", None),
            RemoteError::Timeout => ("timeout", None),
            RemoteError::Invalid(_) => ("validation", None),
            _ => ("remote", None),
        };
        Self {
            kind,
            title: error.title(),
            message: error.user_message(),
            data,
        }
    }
}

impl From<ValidationError> for CommandError {
    fn from(error: ValidationError) -> Self {
        Self {
            kind: "validation",
            title: "Check the details",
            message: error.message().to_string(),
            data: None,
        }
    }
}

impl From<StorageError> for CommandError {
    fn from(error: StorageError) -> Self {
        Self {
            kind: "storage",
            title: "Local storage error",
            message: error.user_message(),
            data: None,
        }
    }
}

impl From<SecretError> for CommandError {
    fn from(error: SecretError) -> Self {
        Self {
            kind: "secret",
            title: "Credential store error",
            message: error.user_message(),
            data: None,
        }
    }
}

impl From<String> for CommandError {
    fn from(message: String) -> Self {
        Self::internal(message)
    }
}

/// The result of checking a pasted token, before a session exists.
#[derive(Debug, Clone, Serialize)]
pub struct TokenCheckDto {
    pub status: crate::cloudflare::models::TokenStatus,
    pub accounts: Vec<Account>,
    pub accounts_unavailable: Option<String>,
    pub needs_manual_account: bool,
}

impl From<TokenCheck> for TokenCheckDto {
    fn from(check: TokenCheck) -> Self {
        let needs_manual_account = check.needs_manual_account();
        Self {
            status: check.status,
            accounts: check.accounts,
            needs_manual_account,
            accounts_unavailable: check.accounts_unavailable,
        }
    }
}

/// The result of running the create wizard.
///
/// `token` is the tunnel's run token, returned only so the wizard's deploy
/// step can hand it to a host. It is never persisted and never included in a
/// state snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct CreateOutcomeDto {
    pub steps: Vec<crate::services::hostnames::StepResult>,
    pub tunnel: Option<crate::domain::Tunnel>,
    pub token: Option<String>,
    pub hostname: Option<crate::services::hostnames::Outcome>,
    pub succeeded: bool,
    pub is_partial: bool,
    pub manual_instructions: Option<String>,
}

impl From<crate::services::tunnels::CreateOutcome> for CreateOutcomeDto {
    fn from(outcome: crate::services::tunnels::CreateOutcome) -> Self {
        use secrecy::ExposeSecret;

        let succeeded = outcome.succeeded();
        let is_partial = outcome.is_partial();
        let manual_instructions = outcome.manual_instructions();
        let crate::services::tunnels::CreateOutcome {
            steps,
            tunnel,
            token,
            hostname,
        } = outcome;

        Self {
            steps,
            tunnel,
            token: token.map(|token| token.expose_secret().to_string()),
            hostname,
            succeeded,
            is_partial,
            manual_instructions,
        }
    }
}

/// What the wizard collected. The account ID is filled in by the command from
/// the live session, so the frontend cannot aim a create at another account.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTunnelRequest {
    pub name: String,
    pub hostname: Option<CreateHostnameRequest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateHostnameRequest {
    pub hostname: String,
    pub path: Option<String>,
    pub service: OriginService,
    pub zone_id: Option<String>,
}

/// Everything a publish needs, minus the account ID and tunnel name (which the
/// backend looks up).
#[derive(Debug, Clone, Deserialize)]
pub struct PublishHostnameRequest {
    pub tunnel_id: String,
    pub hostname: String,
    pub path: Option<String>,
    pub service: OriginService,
    pub zone_id: Option<String>,
    #[serde(default)]
    pub origin_request: crate::domain::config::OriginRequestUpdate,
}

/// A disable request carries the origin currently served, because that is the
/// value Enable will need to put back.
#[derive(Debug, Clone, Deserialize)]
pub struct DisableHostnameRequest {
    pub tunnel_id: String,
    pub hostname: String,
    pub path: Option<String>,
    pub service: String,
}

/// A deployment the frontend has shown to the user and had confirmed.
#[derive(Debug, Clone, Deserialize)]
pub struct ApplyDeploymentRequest {
    pub host_id: i64,
    pub plan: crate::remote::DeploymentPlan,
    pub token: String,
    #[serde(default)]
    pub confirmed_overwrite: bool,
}

/// The account-wide hostname sweep. `errors` carries the tunnels that could
/// not be read, so one permission problem does not hide every other row.
#[derive(Debug, Clone, Serialize)]
pub struct HostnameSweep {
    pub hostnames: Vec<crate::domain::hostname::PublicHostname>,
    pub errors: Vec<String>,
}

/// Product identity, static tables and machine facts, fetched once at startup.
#[derive(Debug, Clone, Serialize)]
pub struct BootstrapDto {
    pub app_name: &'static str,
    pub app_id: &'static str,
    pub version: &'static str,
    pub tagline: &'static str,
    pub affiliation_notice: &'static str,
    pub releases: &'static [crate::release_notes::Release],
    pub log_levels: &'static [(&'static str, &'static str)],
    pub refresh_intervals: &'static [(u32, &'static str)],
    pub required_permissions: &'static [crate::domain::account::RequiredPermission],
    pub max_log_lines: u16,
    pub default_log_lines: u16,
    pub platform: String,
    pub accent: Option<String>,
    pub system_theme: String,
}
