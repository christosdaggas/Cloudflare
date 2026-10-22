//! Remote failures, and how to explain them.
//!
//! Two variants carry more weight than the rest.
//! [`RemoteError::HostKeyUnknown`] is not an error the user should be able to
//! click past — it is the trust prompt, and it stops the operation.
//! [`RemoteError::HostKeyMismatch`] is a refusal: a host key that changed is
//! either a reinstalled machine or an interception, and this application never
//! guesses which.

use crate::domain::activity::redact;

#[derive(Debug, Clone, thiserror::Error)]
pub enum RemoteError {
    /// No host has been registered, or remote control is switched off.
    #[error("remote control is not configured")]
    NotConfigured,

    /// First contact. Carries the fingerprint so the UI can show it.
    #[error("this host has not been trusted yet")]
    HostKeyUnknown {
        fingerprint: String,
        algorithm: String,
    },

    /// The host presented a different key from the one that was trusted.
    #[error("the host key has changed")]
    HostKeyMismatch {
        expected: String,
        actual: String,
        algorithm: String,
    },

    #[error("could not connect: {detail}")]
    Connect { detail: String },

    #[error("authentication failed")]
    Auth {
        detail: String,
        /// Which methods were tried, for the message.
        tried: Vec<String>,
    },

    /// `SSH_AUTH_SOCK` is unset or the agent refused.
    #[error("no ssh-agent is available")]
    AgentUnavailable { detail: String },

    #[error("could not read the private key at {path}")]
    KeyFile { path: String, detail: String },

    /// The key is encrypted and no passphrase is stored.
    #[error("the private key needs a passphrase")]
    PassphraseRequired { path: String },

    /// A command in the allowlist ran and failed.
    #[error("{description} failed on the host")]
    CommandFailed {
        description: &'static str,
        exit_status: Option<u32>,
        stderr: String,
    },

    /// `sudo -n` refused, which on a correctly set up host means the sudoers
    /// rule is missing rather than that the password was wrong.
    #[error("this login is not permitted to control the service")]
    NotPermitted { detail: String },

    #[error("this host does not appear to run Linux")]
    NotLinux { reported: String },

    #[error("this host does not run systemd")]
    NoSystemd,

    #[error("cloudflared is not installed on this host")]
    CloudflaredMissing,

    /// The user asked for a deployment that would overwrite a file, without
    /// having confirmed the overwrite.
    #[error("that would replace an existing file")]
    WouldOverwrite { path: String },

    #[error("cancelled")]
    Cancelled,

    #[error("the host did not respond in time")]
    Timeout,

    /// A value failed validation before anything was sent.
    #[error("{0}")]
    Invalid(#[from] crate::domain::validation::ValidationError),
}

impl RemoteError {
    pub fn user_message(&self) -> String {
        match self {
            Self::NotConfigured => "Remote control is not configured for this host.".into(),

            Self::HostKeyUnknown {
                fingerprint,
                algorithm,
            } => format!(
                "This host has not been connected to before. Its {algorithm} key \
                 fingerprint is {fingerprint}. Check it matches what the server \
                 reports before trusting it."
            ),

            Self::HostKeyMismatch {
                expected, actual, ..
            } => format!(
                "The host key has changed. It was {expected} and is now {actual}. \
                 Either the machine was rebuilt, or something is intercepting the \
                 connection. Nothing was sent. If you know the host was rebuilt, \
                 remove it and add it again."
            ),

            Self::Connect { detail } => {
                format!("Could not connect to the host: {}", redact(detail))
            }

            Self::Auth { tried, .. } => {
                if tried.is_empty() {
                    "The host rejected every authentication method. Check the username \
                     and that this key is in the host's authorized_keys."
                        .into()
                } else {
                    format!(
                        "The host rejected authentication after trying {}. Check the \
                         username and that the key is in the host's authorized_keys.",
                        tried.join(" and ")
                    )
                }
            }

            Self::AgentUnavailable { .. } => {
                "No ssh-agent is running, or it holds no keys. Start one and add a key \
                 with ssh-add, or choose a private key file for this host instead."
                    .into()
            }

            Self::KeyFile { path, detail } => format!(
                "Could not read the private key at {path}: {}",
                redact(detail)
            ),

            Self::PassphraseRequired { path } => format!(
                "The private key at {path} is encrypted. Add its passphrase to this \
                 host, or use ssh-agent instead so the passphrase never has to be \
                 stored."
            ),

            Self::CommandFailed {
                description,
                exit_status,
                stderr,
            } => {
                let mut message = format!("Could not {description} on the host.");
                if let Some(status) = exit_status {
                    message.push_str(&format!(" It exited with status {status}."));
                }
                let stderr = redact(stderr.trim());
                if !stderr.is_empty() {
                    message.push_str(&format!(" The host said: {stderr}"));
                }
                message
            }

            Self::NotPermitted { .. } => {
                "This login cannot control the service without a password. Grant it \
                 passwordless sudo for systemctl on this unit only — the Remote \
                 Hosts page has a ready-made sudoers rule."
                    .into()
            }

            Self::NotLinux { reported } => format!(
                "This host reports its kernel as “{reported}”. Only Linux hosts running \
                 systemd can be controlled from here; the Cloudflare features work \
                 regardless."
            ),

            Self::NoSystemd => {
                "This host does not run systemd, so the cloudflared service cannot be \
                 started or stopped from here."
                    .into()
            }

            Self::CloudflaredMissing => {
                "cloudflared is not installed on this host. Install it from Cloudflare's \
                 package repository first — this application will not install software \
                 on your servers."
                    .into()
            }

            Self::WouldOverwrite { path } => format!(
                "There is already a file at {path}. Confirm the replacement before \
                 continuing."
            ),

            Self::Cancelled => "Cancelled.".into(),

            Self::Timeout => {
                "The host did not respond in time. It may be unreachable or under load.".into()
            }

            Self::Invalid(error) => error.message().to_string(),
        }
    }

    /// A short title for a status page or a dialog heading.
    pub fn title(&self) -> &'static str {
        match self {
            Self::NotConfigured => "Remote control not configured",
            Self::HostKeyUnknown { .. } => "Trust this host?",
            Self::HostKeyMismatch { .. } => "Host key changed",
            Self::Connect { .. } | Self::Timeout => "Could not reach the host",
            Self::Auth { .. }
            | Self::AgentUnavailable { .. }
            | Self::KeyFile { .. }
            | Self::PassphraseRequired { .. } => "Could not sign in",
            Self::NotPermitted { .. } => "Permission needed",
            Self::NotLinux { .. } | Self::NoSystemd => "Unsupported host",
            Self::CloudflaredMissing => "cloudflared not installed",
            Self::WouldOverwrite { .. } => "File already exists",
            Self::CommandFailed { .. } => "The host refused",
            Self::Cancelled => "Cancelled",
            Self::Invalid(_) => "Check the details",
        }
    }

    pub fn icon_name(&self) -> &'static str {
        match self {
            Self::HostKeyMismatch { .. } => "security-high-symbolic",
            Self::HostKeyUnknown { .. } => "dialog-question-symbolic",
            Self::Connect { .. } | Self::Timeout => "network-offline-symbolic",
            Self::Auth { .. }
            | Self::AgentUnavailable { .. }
            | Self::KeyFile { .. }
            | Self::PassphraseRequired { .. }
            | Self::NotPermitted { .. } => "channel-secure-symbolic",
            _ => "dialog-error-symbolic",
        }
    }

    /// Whether the UI should offer a trust prompt rather than an error.
    pub fn is_trust_prompt(&self) -> bool {
        matches!(self, Self::HostKeyUnknown { .. })
    }

    /// Whether this is a security refusal that must never be retried
    /// automatically or dismissed silently.
    pub fn is_security_refusal(&self) -> bool {
        matches!(self, Self::HostKeyMismatch { .. })
    }

    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    /// Whether the host answered at all.
    pub fn is_unreachable(&self) -> bool {
        matches!(self, Self::Connect { .. } | Self::Timeout)
    }
}

pub type RemoteResult<T> = std::result::Result<T, RemoteError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mismatched_host_key_says_nothing_was_sent() {
        let error = RemoteError::HostKeyMismatch {
            expected: "SHA256:aaa".into(),
            actual: "SHA256:bbb".into(),
            algorithm: "ssh-ed25519".into(),
        };
        let message = error.user_message();
        assert!(message.contains("SHA256:aaa"));
        assert!(message.contains("SHA256:bbb"));
        assert!(
            message.contains("Nothing was sent"),
            "the user has to know no credential reached the wrong host"
        );
        assert!(error.is_security_refusal());
        assert!(!error.is_trust_prompt());
    }

    #[test]
    fn an_unknown_host_key_is_a_prompt_not_a_failure() {
        let error = RemoteError::HostKeyUnknown {
            fingerprint: "SHA256:abc".into(),
            algorithm: "ssh-ed25519".into(),
        };
        assert!(error.is_trust_prompt());
        assert!(!error.is_security_refusal());
        assert!(error.user_message().contains("SHA256:abc"));
        assert_eq!(error.title(), "Trust this host?");
    }

    #[test]
    fn a_sudo_refusal_points_at_the_least_privilege_rule() {
        let error = RemoteError::NotPermitted {
            detail: "sudo: a password is required".into(),
        };
        let message = error.user_message();
        assert!(message.contains("passwordless sudo"));
        assert!(message.contains("this unit only"));
        assert!(message.contains("Remote Hosts page"));
    }

    #[test]
    fn cloudflared_missing_says_the_app_will_not_install_it() {
        let message = RemoteError::CloudflaredMissing.user_message();
        assert!(message.contains("will not install software"));
    }

    #[test]
    fn a_command_failure_quotes_stderr_redacted() {
        let error = RemoteError::CommandFailed {
            description: "control the service",
            exit_status: Some(4),
            stderr: "failed with token AbCd3fGh1jKlMn0pQrStUvWxYz012345678901".into(),
        };
        let message = error.user_message();
        assert!(message.contains("status 4"));
        assert!(!message.contains("AbCd3fGh1jKl"), "got {message}");
    }

    #[test]
    fn a_passphrase_error_suggests_the_agent_instead() {
        let message = RemoteError::PassphraseRequired {
            path: "/home/ada/.ssh/id_rsa".into(),
        }
        .user_message();
        assert!(message.contains("ssh-agent"));
        assert!(message.contains("/home/ada/.ssh/id_rsa"));
    }

    #[test]
    fn every_variant_has_a_message_a_title_and_an_icon() {
        let variants = [
            RemoteError::NotConfigured,
            RemoteError::HostKeyUnknown {
                fingerprint: "SHA256:a".into(),
                algorithm: "ssh-ed25519".into(),
            },
            RemoteError::HostKeyMismatch {
                expected: "a".into(),
                actual: "b".into(),
                algorithm: "ssh-ed25519".into(),
            },
            RemoteError::Connect { detail: "x".into() },
            RemoteError::Auth {
                detail: "x".into(),
                tried: vec!["ssh-agent".into()],
            },
            RemoteError::AgentUnavailable { detail: "x".into() },
            RemoteError::KeyFile {
                path: "/k".into(),
                detail: "x".into(),
            },
            RemoteError::PassphraseRequired { path: "/k".into() },
            RemoteError::CommandFailed {
                description: "read recent logs",
                exit_status: None,
                stderr: String::new(),
            },
            RemoteError::NotPermitted { detail: "x".into() },
            RemoteError::NotLinux {
                reported: "Darwin".into(),
            },
            RemoteError::NoSystemd,
            RemoteError::CloudflaredMissing,
            RemoteError::WouldOverwrite {
                path: "/etc/x".into(),
            },
            RemoteError::Cancelled,
            RemoteError::Timeout,
            RemoteError::Invalid(crate::domain::validation::ValidationError::new("no")),
        ];

        for error in variants {
            assert!(!error.user_message().trim().is_empty(), "{error:?}");
            assert!(!error.title().is_empty(), "{error:?}");
            assert!(error.icon_name().ends_with("-symbolic"), "{error:?}");
        }
    }

    #[test]
    fn no_message_leaks_a_secret_shaped_string() {
        let error = RemoteError::Connect {
            detail: "handshake failed for AbCd3fGh1jKlMn0pQrStUvWxYz012345678901".into(),
        };
        assert!(!error.user_message().contains("AbCd3fGh1jKl"));
    }
}
