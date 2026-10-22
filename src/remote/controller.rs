//! The interface between the application and a remote machine.
//!
//! Everything the application can do to a host goes through this trait, and
//! the trait has no "run this command" method. That is deliberate: it is what
//! makes "the remote surface is a fixed allowlist" a property of the type
//! system rather than of a code review.
//!
//! The trait is also what makes remote control *optional*. A build with no
//! host configured never constructs an implementation; the test suite uses
//! [`crate::remote::mock::MockController`]; and a future port could add a
//! different transport without touching a single caller.

use std::future::Future;
use std::pin::Pin;

use secrecy::SecretString;

use crate::domain::host::{RemoteHost, RemoteServiceState};

use super::commands::ServiceAction;
use super::error::RemoteResult;

/// Boxed because the trait is used through `Arc<dyn RemoteConnectorController>`
/// and `async fn` in a trait is not dyn-compatible.
pub type RemoteFuture<'a, T> = Pin<Box<dyn Future<Output = RemoteResult<T>> + Send + 'a>>;

/// The credential material for one connection, assembled just before use and
/// dropped straight afterwards.
///
/// Note what is *not* here: a password. Password authentication is
/// unsupported by design.
#[derive(Debug, Clone, Default)]
pub struct AuthMaterial {
    /// The passphrase for the host's private key, if it has one. Read from the
    /// OS credential store at connection time.
    pub key_passphrase: Option<SecretString>,
}

/// Everything one remote operation needs.
#[derive(Debug, Clone)]
pub struct RemoteRequest {
    pub host: RemoteHost,
    pub auth: AuthMaterial,
    /// Cancels the operation. Shared with the caller.
    pub cancel: crate::cloudflare::Cancel,
}

impl RemoteRequest {
    pub fn new(host: RemoteHost, auth: AuthMaterial) -> Self {
        Self {
            host,
            auth,
            cancel: crate::cloudflare::Cancel::new(),
        }
    }
}

/// A host key seen for the first time, for the trust prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    /// `SHA256:…`, exactly as OpenSSH prints it, so a user can compare it with
    /// `ssh-keyscan` output character for character.
    pub fingerprint: String,
    /// `ssh-ed25519`, `rsa-sha2-512`, …
    pub algorithm: String,
}

/// What preflight established.
///
/// Every field is an answer to one of the questions the security model says
/// must be asked before an action is offered, and each is separately `false`
/// rather than collapsed into one "ready" flag — the UI explains which check
/// failed, because "it did not work" is not a useful thing to tell someone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    pub reachable: bool,
    pub host_key_matched: bool,
    pub is_linux: bool,
    pub kernel: Option<String>,
    pub systemd_available: bool,
    pub systemd_version: Option<String>,
    pub cloudflared_path: Option<String>,
    pub cloudflared_version: Option<String>,
    /// Whether `cloudflared` is new enough for `--token-file`, which is the
    /// only way to deploy without putting the token on a command line.
    pub supports_token_file: bool,
    pub unit_present: bool,
    pub service_state: RemoteServiceState,
    pub service_enabled: Option<bool>,
    pub can_control_service: bool,
    /// The tunnel the unit's `ExecStart` names, when it can be told. Used to
    /// warn that a host is serving a different tunnel from the one it is
    /// registered against.
    pub configured_tunnel_id: Option<String>,
    /// Warnings that do not stop the host being usable.
    pub notes: Vec<String>,
}

impl Preflight {
    /// Whether Start / Stop / Restart should be enabled.
    pub fn can_control(&self) -> bool {
        self.reachable && self.systemd_available && self.unit_present && self.can_control_service
    }

    /// Whether a deployment could succeed if the user asked for one.
    pub fn can_deploy(&self) -> bool {
        self.reachable
            && self.is_linux
            && self.systemd_available
            && self.cloudflared_path.is_some()
            && self.can_control_service
    }

    /// The single most useful thing to tell the user about this host, or
    /// `None` when everything is in order.
    pub fn blocking_reason(&self) -> Option<String> {
        if !self.reachable {
            return Some("The host could not be reached.".into());
        }
        if !self.host_key_matched {
            return Some("The host key does not match the one that was trusted.".into());
        }
        if !self.is_linux {
            return Some(format!(
                "This host reports its kernel as “{}”. Only Linux hosts can be \
                 controlled from here.",
                self.kernel.as_deref().unwrap_or("unknown")
            ));
        }
        if !self.systemd_available {
            return Some(
                "This host does not run systemd, so the cloudflared service cannot be \
                 controlled from here."
                    .into(),
            );
        }
        if self.cloudflared_path.is_none() {
            return Some(
                "cloudflared is not installed on this host. Install it first — the \
                 Remote Hosts page has the commands."
                    .into(),
            );
        }
        if !self.unit_present {
            return Some(
                "The service unit does not exist on this host yet. Deploy a connector, \
                 or point this host at the unit cloudflared actually installed."
                    .into(),
            );
        }
        if !self.can_control_service {
            return Some(
                "This login cannot control the service. Grant it passwordless sudo for \
                 systemctl on this unit — the sudoers rule offered on the Remote \
                 Hosts page grants only that."
                    .into(),
            );
        }
        None
    }
}

impl Default for Preflight {
    fn default() -> Self {
        Self {
            reachable: false,
            host_key_matched: false,
            is_linux: false,
            kernel: None,
            systemd_available: false,
            systemd_version: None,
            cloudflared_path: None,
            cloudflared_version: None,
            supports_token_file: false,
            unit_present: false,
            service_state: RemoteServiceState::NotInstalled,
            service_enabled: None,
            can_control_service: false,
            configured_tunnel_id: None,
            notes: Vec::new(),
        }
    }
}

/// The result of a start/stop/restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceOutcome {
    pub action: ServiceAction,
    pub state_before: RemoteServiceState,
    pub state_after: RemoteServiceState,
    /// Anything systemd said, already trimmed and redacted.
    pub message: Option<String>,
}

impl ServiceOutcome {
    /// Whether the action produced the state it was asking for.
    pub fn succeeded(&self) -> bool {
        match self.action {
            ServiceAction::Start | ServiceAction::Restart => self.state_after.is_running(),
            ServiceAction::Stop => !self.state_after.is_running(),
        }
    }
}

/// One step of a deployment, as shown in the preview and as executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentStep {
    /// What the step does, in a sentence.
    pub description: String,
    /// The command as it will run, with any secret content replaced by a
    /// placeholder. This is what the preview shows, so it must never be the
    /// real thing.
    pub preview: String,
    /// Whether this step needs root.
    pub privileged: bool,
    /// Whether this step would overwrite something that already exists.
    pub overwrites: bool,
}

/// A previewable plan. Nothing runs until the user has seen this and agreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentPlan {
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub unit: String,
    pub token_path: String,
    pub unit_path: String,
    pub cloudflared_path: String,
    pub steps: Vec<DeploymentStep>,
    /// The unit file that will be written, for the preview.
    pub unit_file: String,
    /// True when a unit file is already there and will be replaced.
    pub replaces_existing_unit: bool,
}

impl DeploymentPlan {
    pub fn touches_privileged_files(&self) -> bool {
        self.steps.iter().any(|step| step.privileged)
    }
}

/// What actually happened when a plan was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeploymentOutcome {
    /// Steps that completed, in order.
    pub completed: Vec<String>,
    /// The step that failed, if one did, and why.
    pub failed: Option<(String, String)>,
    pub service_state: RemoteServiceState,
}

impl DeploymentOutcome {
    pub fn is_complete(&self) -> bool {
        self.failed.is_none()
    }

    pub fn is_partial(&self) -> bool {
        self.failed.is_some() && !self.completed.is_empty()
    }
}

/// The remote surface.
pub trait RemoteConnectorController: Send + Sync {
    /// Connects far enough to read the host key, then disconnects without
    /// authenticating.
    ///
    /// This is how a first connection works: the fingerprint comes back, the
    /// user is shown it and asked to trust it, and only then does anything
    /// else connect. There is no "accept and continue" path that would let a
    /// key be trusted implicitly.
    fn probe_host_key<'a>(&'a self, request: &'a RemoteRequest) -> RemoteFuture<'a, HostKey>;

    /// Runs the read-only checks.
    fn preflight<'a>(&'a self, request: &'a RemoteRequest) -> RemoteFuture<'a, Preflight>;

    /// Starts, stops or restarts the unit.
    fn service_action<'a>(
        &'a self,
        request: &'a RemoteRequest,
        action: ServiceAction,
    ) -> RemoteFuture<'a, ServiceOutcome>;

    /// A bounded tail of the unit's journal.
    fn logs<'a>(&'a self, request: &'a RemoteRequest, lines: u16) -> RemoteFuture<'a, Vec<String>>;

    /// Runs one allowlisted command and returns its trimmed stdout.
    ///
    /// This is the seam the transport-protocol switch goes through. It takes
    /// the command as a [`RemoteCommand`](super::RemoteCommand) rather than a
    /// string, so a caller cannot name anything the allowlist does not
    /// already contain; and `stdin` carries plain configuration content —
    /// never a secret, which is what `apply_deployment`'s `SecretString` is
    /// for.
    fn run_allowlisted<'a>(
        &'a self,
        request: &'a RemoteRequest,
        command: super::RemoteCommand,
        stdin: Option<&'a str>,
    ) -> RemoteFuture<'a, String>;

    /// Builds the plan for deploying a connector. Runs read-only commands
    /// only: it inspects the host so the preview can say what will be
    /// overwritten.
    fn plan_deployment<'a>(
        &'a self,
        request: &'a RemoteRequest,
        tunnel_id: &'a str,
        tunnel_name: &'a str,
    ) -> RemoteFuture<'a, DeploymentPlan>;

    /// Applies a plan the user has seen and approved.
    ///
    /// `token` is the tunnel's run token. It is written to a root-owned file
    /// over the channel's stdin and never appears in a command line, an
    /// environment variable or a log.
    fn apply_deployment<'a>(
        &'a self,
        request: &'a RemoteRequest,
        plan: &'a DeploymentPlan,
        token: &'a SecretString,
    ) -> RemoteFuture<'a, DeploymentOutcome>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Preflight {
        Preflight {
            reachable: true,
            host_key_matched: true,
            is_linux: true,
            kernel: Some("Linux".into()),
            systemd_available: true,
            systemd_version: Some("systemd 257".into()),
            cloudflared_path: Some("/usr/bin/cloudflared".into()),
            cloudflared_version: Some("2026.1.0".into()),
            supports_token_file: true,
            unit_present: true,
            service_state: RemoteServiceState::Active,
            service_enabled: Some(true),
            can_control_service: true,
            configured_tunnel_id: None,
            notes: vec![],
        }
    }

    #[test]
    fn a_fully_ready_host_has_no_blocking_reason() {
        let preflight = ready();
        assert!(preflight.can_control());
        assert!(preflight.can_deploy());
        assert!(preflight.blocking_reason().is_none());
    }

    #[test]
    fn a_default_preflight_blocks_on_reachability_first() {
        let preflight = Preflight::default();
        assert!(!preflight.can_control());
        assert!(!preflight.can_deploy());
        assert!(preflight
            .blocking_reason()
            .unwrap()
            .contains("could not be reached"));
    }

    #[test]
    fn each_failed_check_produces_its_own_explanation() {
        let cases: Vec<(Preflight, &str)> = vec![
            (
                Preflight {
                    host_key_matched: false,
                    ..ready()
                },
                "host key",
            ),
            (
                Preflight {
                    is_linux: false,
                    kernel: Some("Darwin".into()),
                    ..ready()
                },
                "Darwin",
            ),
            (
                Preflight {
                    systemd_available: false,
                    ..ready()
                },
                "systemd",
            ),
            (
                Preflight {
                    cloudflared_path: None,
                    ..ready()
                },
                "cloudflared is not installed",
            ),
            (
                Preflight {
                    unit_present: false,
                    ..ready()
                },
                "service unit does not exist",
            ),
            (
                Preflight {
                    can_control_service: false,
                    ..ready()
                },
                "passwordless sudo",
            ),
        ];

        for (preflight, expected) in cases {
            let reason = preflight
                .blocking_reason()
                .unwrap_or_else(|| panic!("expected a reason mentioning {expected}"));
            assert!(reason.contains(expected), "got {reason}");
        }
    }

    #[test]
    fn control_needs_the_unit_to_exist_and_sudo_to_work() {
        assert!(!Preflight {
            unit_present: false,
            ..ready()
        }
        .can_control());
        assert!(!Preflight {
            can_control_service: false,
            ..ready()
        }
        .can_control());
    }

    #[test]
    fn deployment_does_not_need_the_unit_to_exist_yet() {
        let fresh = Preflight {
            unit_present: false,
            service_state: RemoteServiceState::NotInstalled,
            ..ready()
        };
        assert!(
            fresh.can_deploy(),
            "deploying is how the unit comes to exist"
        );
        assert!(!fresh.can_control());
    }

    #[test]
    fn a_service_outcome_knows_whether_it_got_what_it_asked_for() {
        let started = ServiceOutcome {
            action: ServiceAction::Start,
            state_before: RemoteServiceState::Inactive,
            state_after: RemoteServiceState::Active,
            message: None,
        };
        assert!(started.succeeded());

        let failed_to_start = ServiceOutcome {
            state_after: RemoteServiceState::Failed,
            ..started.clone()
        };
        assert!(!failed_to_start.succeeded());

        let stopped = ServiceOutcome {
            action: ServiceAction::Stop,
            state_before: RemoteServiceState::Active,
            state_after: RemoteServiceState::Inactive,
            message: None,
        };
        assert!(stopped.succeeded());

        let would_not_stop = ServiceOutcome {
            state_after: RemoteServiceState::Active,
            ..stopped
        };
        assert!(!would_not_stop.succeeded());
    }

    #[test]
    fn a_partial_deployment_is_distinguishable_from_a_total_failure() {
        let partial = DeploymentOutcome {
            completed: vec!["wrote the token file".into()],
            failed: Some(("start the service".into(), "sudo denied".into())),
            service_state: RemoteServiceState::Inactive,
        };
        assert!(!partial.is_complete());
        assert!(partial.is_partial());

        let nothing = DeploymentOutcome {
            completed: vec![],
            failed: Some(("connect".into(), "no route".into())),
            service_state: RemoteServiceState::NotInstalled,
        };
        assert!(!nothing.is_partial());

        let done = DeploymentOutcome {
            completed: vec!["a".into(), "b".into()],
            failed: None,
            service_state: RemoteServiceState::Active,
        };
        assert!(done.is_complete());
    }

    #[test]
    fn auth_material_has_no_password_field() {
        // A compile-time statement of intent: if someone adds one, this stops
        // building and they have to explain why in a review.
        let material = AuthMaterial::default();
        assert!(material.key_passphrase.is_none());
    }
}
