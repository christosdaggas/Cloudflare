//! What can go wrong talking to Cloudflare, and how to say it.
//!
//! Two rules shape this module.
//!
//! **A user-facing message is written for the user.** "403 Forbidden" is not a
//! sentence anyone can act on; "your API token can list tunnels but cannot
//! create the DNS record — add Zone → DNS → Edit to it" is. Each variant
//! carries the operation it failed on so [`CloudflareError::user_message`] can
//! name the missing permission rather than the HTTP status.
//!
//! **Nothing here may carry a secret.** The token is only ever in a header, so
//! a `reqwest::Error` cannot contain it, but every string that reaches this
//! type is run through the redactor anyway — defence in depth against a future
//! call site that formats something it should not.

use std::fmt;
use std::time::Duration;

use crate::domain::activity::redact;
use crate::domain::validation::ValidationError;

use super::envelope::ApiMessage;

/// Which capability a failed call needed, so a 403 can name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Tunnels,
    Zones,
    Dns,
    /// Any live token is enough — token verification, account listing.
    Any,
}

impl Capability {
    /// The dashboard row the user has to add.
    pub fn permission_label(self) -> Option<&'static str> {
        match self {
            Self::Tunnels => Some("Account → Cloudflare Tunnel → Edit"),
            Self::Zones => Some("Zone → Zone → Read"),
            Self::Dns => Some("Zone → DNS → Edit"),
            Self::Any => None,
        }
    }
}

/// What the application was doing. Used only for wording.
#[derive(Debug, Clone)]
pub struct Operation {
    /// A short phrase that fits after "Could not …".
    pub description: &'static str,
    pub capability: Capability,
}

impl Operation {
    pub const fn new(description: &'static str, capability: Capability) -> Self {
        Self {
            description,
            capability,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CloudflareError {
    /// No token has been saved yet.
    #[error("not connected to Cloudflare")]
    NotConnected,

    /// The request never reached Cloudflare: DNS failure, no route, TLS
    /// refused. Distinct from a server error because the remedy is different.
    #[error("could not reach Cloudflare: {detail}")]
    Offline { detail: String },

    #[error("the request to Cloudflare timed out")]
    Timeout { operation: Operation },

    /// The caller asked for it to stop.
    #[error("cancelled")]
    Cancelled,

    /// HTTP 429, or a 403 that Cloudflare's rate limiter produced.
    #[error("rate limited by Cloudflare")]
    RateLimited {
        retry_after: Option<Duration>,
        operation: Operation,
    },

    /// HTTP 401. The token is absent, malformed, revoked or expired.
    #[error("the API token was rejected")]
    Unauthorized {
        errors: Vec<ApiMessage>,
        operation: Operation,
    },

    /// HTTP 403. The token is live but does not cover this call — either the
    /// permission group is missing or the account/zone is out of its scope.
    #[error("the API token is not allowed to do that")]
    Forbidden {
        errors: Vec<ApiMessage>,
        operation: Operation,
    },

    #[error("not found")]
    NotFound {
        errors: Vec<ApiMessage>,
        operation: Operation,
    },

    /// Any other non-2xx, or a 2xx whose envelope said `success: false`.
    #[error("Cloudflare returned an error")]
    Api {
        status: u16,
        errors: Vec<ApiMessage>,
        operation: Operation,
    },

    /// The response was not the JSON this build expects.
    #[error("could not read Cloudflare's response: {detail}")]
    Decode {
        detail: String,
        operation: Operation,
    },

    /// A value failed validation before the request was made.
    #[error("{0}")]
    Invalid(#[from] ValidationError),

    /// Pagination stopped at the page cap with more pages still advertised.
    /// Returned instead of a silently truncated list so callers never
    /// present a partial result as complete.
    #[error("the list from Cloudflare was too large to fetch completely")]
    Truncated { pages: u32, operation: Operation },

    /// Anything else from the HTTP stack.
    #[error("network error: {detail}")]
    Transport { detail: String },
}

impl CloudflareError {
    /// Builds the `Offline` / `Timeout` / `Transport` split out of a
    /// `reqwest::Error`.
    pub fn from_reqwest(error: reqwest::Error, operation: Operation) -> Self {
        let detail = redact(&error.to_string());

        if error.is_timeout() {
            return Self::Timeout { operation };
        }
        if error.is_connect() || error.is_request() {
            return Self::Offline { detail };
        }
        if error.is_decode() {
            return Self::Decode { detail, operation };
        }
        Self::Transport { detail }
    }

    /// One sentence for a banner, a toast or a status page.
    pub fn user_message(&self) -> String {
        match self {
            Self::NotConnected => {
                "Not connected to Cloudflare. Add an API token on the Connection page.".into()
            }

            Self::Offline { .. } => {
                "Could not reach Cloudflare. Check the network connection and try again.".into()
            }

            Self::Timeout { operation } => format!(
                "Timed out while trying to {}. Cloudflare may be slow — try again.",
                operation.description
            ),

            Self::Cancelled => "Cancelled.".into(),

            Self::RateLimited { retry_after, .. } => match retry_after {
                Some(wait) => format!(
                    "Cloudflare is rate limiting this account. Try again in about {} \
                     seconds. The limit is shared with the Cloudflare dashboard, so \
                     closing other tabs helps.",
                    wait.as_secs().max(1)
                ),
                None => "Cloudflare is rate limiting this account. Wait a few minutes \
                         and try again — the limit is shared with the Cloudflare \
                         dashboard."
                    .into(),
            },

            Self::Unauthorized { operation, .. } => format!(
                "Cloudflare rejected the API token while trying to {}. It may have been \
                 revoked or have expired. Check it on the Connection page.",
                operation.description
            ),

            Self::Forbidden { operation, .. } => match operation.capability.permission_label() {
                Some(permission) => format!(
                    "The API token is not allowed to {}. Add “{permission}” to the \
                         token, and make sure the account and zone are inside its \
                         resource scope.",
                    operation.description
                ),
                None => format!("The API token is not allowed to {}.", operation.description),
            },

            Self::NotFound { operation, .. } => format!(
                "Could not {} — Cloudflare says it does not exist. It may have been \
                 removed elsewhere; refresh to see the current state.",
                operation.description
            ),

            Self::Api {
                status,
                errors,
                operation,
            } => {
                let detail = errors
                    .iter()
                    .map(ApiMessage::full_text)
                    .collect::<Vec<_>>()
                    .join("; ");
                if detail.is_empty() {
                    format!(
                        "Could not {} — Cloudflare returned HTTP {status}.",
                        operation.description
                    )
                } else {
                    format!(
                        "Could not {} — Cloudflare said: {}",
                        operation.description,
                        redact(&detail)
                    )
                }
            }

            Self::Decode { operation, .. } => format!(
                "Could not {} — Cloudflare's response was not in a form this version \
                 understands. This is usually a bug in the application.",
                operation.description
            ),

            Self::Invalid(error) => error.message().to_string(),

            Self::Truncated { operation, .. } => format!(
                "Could not {} — the list is too large to fetch completely. \
                 Try again, or narrow it in the Cloudflare dashboard.",
                operation.description
            ),

            Self::Transport { .. } => "The connection to Cloudflare failed. Try again.".into(),
        }
    }

    /// The longer form, for the activity log's detail line. Already redacted.
    pub fn detail(&self) -> Option<String> {
        let text = match self {
            Self::Offline { detail } | Self::Transport { detail } => detail.clone(),
            Self::Decode { detail, .. } => detail.clone(),
            Self::Unauthorized { errors, .. }
            | Self::Forbidden { errors, .. }
            | Self::NotFound { errors, .. }
            | Self::Api { errors, .. } => errors
                .iter()
                .map(|error| format!("{error}"))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };

        if text.trim().is_empty() {
            None
        } else {
            Some(redact(&text))
        }
    }

    /// Whether retrying the *same* request could plausibly succeed.
    ///
    /// This is consulted only for reads. A destructive call is never retried
    /// automatically, however transient the failure looked: a create that
    /// timed out may well have succeeded, and a second one would leave two
    /// tunnels behind.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Offline { .. } | Self::Timeout { .. } | Self::Transport { .. } => true,
            Self::RateLimited { .. } => true,
            Self::Api { status, .. } => *status >= 500,
            _ => false,
        }
    }

    /// How long to wait before a retry, when the server said.
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    /// Whether this is a permission problem, which the UI surfaces
    /// differently: a missing permission is a settings problem, not a
    /// transient failure, so it gets a banner rather than a toast.
    pub fn is_permission_problem(&self) -> bool {
        matches!(self, Self::Unauthorized { .. } | Self::Forbidden { .. })
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Which capability the failed call needed, if it is a permission failure.
    pub fn missing_capability(&self) -> Option<Capability> {
        match self {
            Self::Forbidden { operation, .. } => Some(operation.capability),
            _ => None,
        }
    }

    /// A short title for a status page.
    pub fn title(&self) -> &'static str {
        match self {
            Self::NotConnected => "Not connected",
            Self::Offline { .. } | Self::Transport { .. } => "Offline",
            Self::Timeout { .. } => "Timed out",
            Self::Cancelled => "Cancelled",
            Self::RateLimited { .. } => "Rate limited",
            Self::Unauthorized { .. } => "Token rejected",
            Self::Forbidden { .. } => "Permission needed",
            Self::NotFound { .. } => "Not found",
            Self::Api { .. } => "Cloudflare error",
            Self::Decode { .. } => "Unexpected response",
            Self::Invalid(_) => "Check the details",
            Self::Truncated { .. } => "List too large",
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::Offline { .. } | Self::Transport { .. } => "network-offline-symbolic",
            Self::Unauthorized { .. } | Self::Forbidden { .. } | Self::NotConnected => {
                "channel-secure-symbolic"
            }
            Self::RateLimited { .. } | Self::Timeout { .. } => "alarm-symbolic",
            _ => "dialog-error-symbolic",
        }
    }
}

/// Never let a `Debug` or `Display` of this type reach a log unredacted.
pub struct Redacted<'a>(pub &'a CloudflareError);

impl fmt::Display for Redacted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&redact(&self.0.to_string()))
    }
}

pub type Result<T> = std::result::Result<T, CloudflareError>;

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: Operation = Operation::new("list tunnels", Capability::Tunnels);
    const DNS: Operation = Operation::new("create the DNS record", Capability::Dns);

    fn message(code: i64, text: &str) -> ApiMessage {
        ApiMessage {
            code,
            message: text.into(),
            documentation_url: None,
            source: None,
            error_chain: vec![],
        }
    }

    #[test]
    fn a_forbidden_error_names_the_permission_to_add() {
        let error = CloudflareError::Forbidden {
            errors: vec![message(9109, "Unauthorized to access requested resource")],
            operation: DNS,
        };
        let text = error.user_message();
        assert!(text.contains("Zone → DNS → Edit"), "got {text}");
        assert!(text.contains("create the DNS record"), "got {text}");
        assert!(error.is_permission_problem());
        assert_eq!(error.missing_capability(), Some(Capability::Dns));
    }

    #[test]
    fn an_unauthorized_error_points_at_the_connection_page() {
        let error = CloudflareError::Unauthorized {
            errors: vec![message(10000, "Authentication error")],
            operation: LIST,
        };
        assert!(error.user_message().contains("Connection page"));
        assert!(error.is_permission_problem());
        assert!(!error.is_transient());
    }

    #[test]
    fn rate_limiting_quotes_the_wait_and_explains_the_shared_budget() {
        let error = CloudflareError::RateLimited {
            retry_after: Some(Duration::from_secs(42)),
            operation: LIST,
        };
        let text = error.user_message();
        assert!(text.contains("42"), "got {text}");
        assert!(
            text.contains("dashboard"),
            "the shared budget is worth saying"
        );
        assert!(error.is_transient());
        assert_eq!(error.retry_after(), Some(Duration::from_secs(42)));
    }

    #[test]
    fn a_server_error_is_transient_and_a_client_error_is_not() {
        let server = CloudflareError::Api {
            status: 503,
            errors: vec![],
            operation: LIST,
        };
        assert!(server.is_transient());

        let client = CloudflareError::Api {
            status: 400,
            errors: vec![message(1004, "Invalid something")],
            operation: LIST,
        };
        assert!(!client.is_transient());
    }

    #[test]
    fn an_api_error_quotes_what_cloudflare_said() {
        let error = CloudflareError::Api {
            status: 400,
            errors: vec![message(1004, "DNS Validation Error")],
            operation: DNS,
        };
        assert!(error.user_message().contains("DNS Validation Error"));
    }

    #[test]
    fn error_text_is_redacted() {
        let error = CloudflareError::Api {
            status: 400,
            errors: vec![message(
                1004,
                "bad token AbCd3fGh1jKlMn0pQrStUvWxYz012345678901",
            )],
            operation: DNS,
        };
        assert!(!error.user_message().contains("AbCd3fGh1jKl"));
        assert!(!error.detail().unwrap().contains("AbCd3fGh1jKl"));
    }

    #[test]
    fn offline_is_transient_and_does_not_blame_the_token() {
        let error = CloudflareError::Offline {
            detail: "dns error".into(),
        };
        assert!(error.is_transient());
        assert!(!error.is_permission_problem());
        assert!(error.user_message().contains("network"));
    }

    #[test]
    fn every_variant_has_a_non_empty_user_message_and_title() {
        let variants = [
            CloudflareError::NotConnected,
            CloudflareError::Offline { detail: "x".into() },
            CloudflareError::Timeout { operation: LIST },
            CloudflareError::Cancelled,
            CloudflareError::RateLimited {
                retry_after: None,
                operation: LIST,
            },
            CloudflareError::Unauthorized {
                errors: vec![],
                operation: LIST,
            },
            CloudflareError::Forbidden {
                errors: vec![],
                operation: LIST,
            },
            CloudflareError::NotFound {
                errors: vec![],
                operation: LIST,
            },
            CloudflareError::Api {
                status: 500,
                errors: vec![],
                operation: LIST,
            },
            CloudflareError::Decode {
                detail: "x".into(),
                operation: LIST,
            },
            CloudflareError::Invalid(ValidationError::new("nope")),
            CloudflareError::Transport { detail: "x".into() },
        ];

        for error in variants {
            assert!(!error.user_message().trim().is_empty(), "{error:?}");
            assert!(!error.title().is_empty(), "{error:?}");
            assert!(error.icon_name().ends_with("-symbolic"), "{error:?}");
        }
    }
}
