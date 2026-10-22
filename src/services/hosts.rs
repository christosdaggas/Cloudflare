//! Registering, checking and controlling remote hosts.
//!
//! The whole of this module is optional behaviour. Every function takes a
//! [`RemoteConnectorController`] rather than constructing one, which is what
//! lets the default test suite run the entire remote surface — including the
//! host-key rules — against [`crate::remote::MockController`] with no SSH
//! server anywhere.
//!
//! The trust flow is the part worth reading twice. A host cannot be operated
//! on until [`trust_host_key`] has recorded a fingerprint, and that function
//! is only ever called from a dialog that showed the fingerprint to the user.
//! There is no path from "add a host" to "run a command" that does not pass
//! through a human looking at a hash.

use std::sync::Arc;

use secrecy::SecretString;

use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::{
    RemoteAuth, RemoteHost, RemoteHostDraft, RemoteServiceState, RemoteStatus,
};
use crate::domain::validation;
use crate::remote::commands::{RemoteCommand, ServiceAction};
use crate::remote::controller::{
    AuthMaterial, DeploymentOutcome, DeploymentPlan, HostKey, Preflight, RemoteRequest,
    ServiceOutcome,
};
use crate::remote::{RemoteConnectorController, RemoteError, RemoteResult};

/// Validates a draft before it is written to the database.
///
/// Returns the corrected draft — hostnames are lowercased, a unit name gains
/// its `.service` suffix — so that what is stored is what will be sent.
pub fn validate_draft(
    draft: &RemoteHostDraft,
) -> Result<RemoteHostDraft, validation::ValidationError> {
    Ok(RemoteHostDraft {
        name: validation::friendly_name(&draft.name)?,
        address: validation::ssh_address(&draft.address)?,
        port: validation::ssh_port(&draft.port.to_string())?,
        username: validation::ssh_username(&draft.username)?,
        auth: match &draft.auth {
            RemoteAuth::Agent => RemoteAuth::Agent,
            RemoteAuth::KeyFile {
                path,
                has_passphrase,
            } => {
                let path = validation::absolute_path(&path.to_string_lossy())?;
                RemoteAuth::KeyFile {
                    path: std::path::PathBuf::from(path),
                    has_passphrase: *has_passphrase,
                }
            }
        },
        service_unit: validation::systemd_unit(&draft.service_unit)?,
        tunnel_id: match &draft.tunnel_id {
            Some(id) => Some(validation::tunnel_id(id)?),
            None => None,
        },
        notes: draft.notes.clone().filter(|notes| !notes.trim().is_empty()),
    })
}

/// Reads a host's key without authenticating, so the user can be shown it.
pub async fn probe_host_key(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
) -> RemoteResult<HostKey> {
    let request = RemoteRequest::new(host.clone(), AuthMaterial::default());
    controller.probe_host_key(&request).await
}

/// The decision a user makes about a host key they have just been shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustDecision {
    /// First contact: nothing was pinned before.
    FirstContact { key: HostKey },
    /// The key matches what was pinned. Nothing to decide.
    Unchanged,
    /// The key changed. This is a refusal, not a question — the caller must
    /// not offer a "trust anyway" button.
    Changed { expected: String, key: HostKey },
}

impl TrustDecision {
    /// Compares a freshly probed key against what is on file.
    pub fn evaluate(host: &RemoteHost, key: HostKey) -> Self {
        match &host.host_key_fingerprint {
            None => Self::FirstContact { key },
            Some(expected) if *expected == key.fingerprint => Self::Unchanged,
            Some(expected) => Self::Changed {
                expected: expected.clone(),
                key,
            },
        }
    }

    /// Whether the application may go on to connect.
    pub fn may_proceed(&self) -> bool {
        matches!(self, Self::Unchanged)
    }

    pub fn activity(&self, host: &RemoteHost) -> ActivityEvent {
        match self {
            Self::FirstContact { key } => ActivityEvent::info(
                ActivityCategory::Ssh,
                format!(
                    "First connection to {}: {} key {}",
                    host.name, key.algorithm, key.fingerprint
                ),
            )
            .with_target(host.name.clone()),
            Self::Unchanged => ActivityEvent::info(
                ActivityCategory::Ssh,
                format!("Host key for {} matched", host.name),
            )
            .with_target(host.name.clone()),
            Self::Changed { expected, key } => ActivityEvent::error(
                ActivityCategory::Ssh,
                format!(
                    "Refused to connect to {}: host key changed from {} to {}",
                    host.name, expected, key.fingerprint
                ),
            )
            .with_target(host.name.clone()),
        }
    }
}

/// Runs the read-only checks against a trusted host.
pub async fn preflight(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
) -> RemoteResult<Preflight> {
    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );
    controller.preflight(&request).await
}

/// Whether a service action is safe to offer, and what to warn about first.
///
/// The warning matters more than it looks. A tunnel with several connectors
/// keeps serving traffic when one is stopped, and a user who thinks stopping
/// one connector disables the tunnel will be surprised in one direction; a
/// user who thinks it does not will be surprised in the other. So the
/// confirmation says which case they are in.
pub fn action_warning(
    action: ServiceAction,
    tunnel_name: &str,
    other_active_connectors: usize,
) -> Option<String> {
    if !action.is_disruptive() {
        return None;
    }

    Some(match (action, other_active_connectors) {
        (ServiceAction::Stop, 0) => format!(
            "This is the only connector serving “{tunnel_name}”. Stopping it takes every \
             hostname on that tunnel offline until something starts serving it again. \
             The tunnel itself is not deleted and its DNS records stay as they are."
        ),
        (ServiceAction::Stop, others) => format!(
            "{others} other connector(s) will still be serving “{tunnel_name}”, so the \
             tunnel stays up. Only this machine stops handling traffic."
        ),
        (ServiceAction::Restart, 0) => format!(
            "This is the only connector serving “{tunnel_name}”. Restarting interrupts \
             traffic for a few seconds while it reconnects."
        ),
        (ServiceAction::Restart, others) => format!(
            "{others} other connector(s) will keep serving “{tunnel_name}” while this one \
             reconnects, so traffic should not be interrupted."
        ),
        _ => unreachable!("start is not disruptive"),
    })
}

/// Starts, stops or restarts the unit on a host.
pub async fn service_action(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
    action: ServiceAction,
) -> RemoteResult<ServiceOutcome> {
    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );
    controller.service_action(&request, action).await
}

pub async fn logs(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
    lines: u16,
) -> RemoteResult<Vec<String>> {
    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );
    controller.logs(&request, lines).await
}

/// Reads the transport protocol a host's connector is pinned to, if any.
pub async fn read_transport(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
) -> RemoteResult<Option<crate::domain::host::TransportProtocol>> {
    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );
    let output = controller
        .run_allowlisted(
            &request,
            RemoteCommand::UnitEnvironment {
                unit: host.service_unit.clone(),
            },
            None,
        )
        .await?;
    Ok(crate::domain::host::TransportProtocol::from_env_output(
        &output,
    ))
}

/// What applying a protocol choice did, for the toast and the activity log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportOutcome {
    /// The protocol now pinned, or `None` when the override was removed.
    pub protocol: Option<crate::domain::host::TransportProtocol>,
    /// The unit's state after the restart that followed.
    pub service_state: RemoteServiceState,
}

/// Pins or clears the connector's transport protocol on a host.
///
/// Writes (or removes) a systemd drop-in, reloads systemd, and restarts the
/// unit — the restart is what makes the change take effect, and it is
/// disruptive in exactly the way `ServiceAction::Restart` always is, so the
/// caller confirms before calling this.
pub async fn set_transport(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
    protocol: Option<crate::domain::host::TransportProtocol>,
) -> RemoteResult<TransportOutcome> {
    use crate::domain::host::TransportProtocol;

    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }
    let unit = validation::systemd_unit(&host.service_unit)?;
    let dir = TransportProtocol::drop_in_dir(&unit)?;
    let path = TransportProtocol::drop_in_path(&unit)?;

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );

    controller
        .run_allowlisted(&request, RemoteCommand::MakeDir { dir }, None)
        .await?;

    match protocol {
        Some(choice) => {
            let content = choice.drop_in_content();
            controller
                .run_allowlisted(
                    &request,
                    RemoteCommand::InstallFromStdin { path, mode: "0644" },
                    Some(&content),
                )
                .await?;
        }
        None => {
            controller
                .run_allowlisted(&request, RemoteCommand::RemoveFile { path }, None)
                .await?;
        }
    }

    controller
        .run_allowlisted(&request, RemoteCommand::DaemonReload, None)
        .await?;

    // The restart is part of the change: a drop-in does nothing to a running
    // connector until the unit starts again.
    controller
        .service_action(&request, ServiceAction::Restart)
        .await
        .map(|outcome| TransportOutcome {
            protocol,
            service_state: outcome.state_after,
        })
}

/// Builds a deployment plan for the user to read.
pub async fn plan_deployment(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
    tunnel_id: &str,
    tunnel_name: &str,
) -> RemoteResult<DeploymentPlan> {
    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );
    controller
        .plan_deployment(&request, tunnel_id, tunnel_name)
        .await
}

/// Applies a plan the user approved.
///
/// `confirmed_overwrite` is not a formality: a plan that would replace an
/// existing systemd unit is refused unless the user was shown that fact and
/// agreed to it specifically.
pub async fn apply_deployment(
    controller: &Arc<dyn RemoteConnectorController>,
    host: &RemoteHost,
    passphrase: Option<SecretString>,
    plan: &DeploymentPlan,
    token: &SecretString,
    confirmed_overwrite: bool,
) -> RemoteResult<DeploymentOutcome> {
    if host.host_key_fingerprint.is_none() {
        return Err(RemoteError::NotConfigured);
    }
    if plan.replaces_existing_unit && !confirmed_overwrite {
        return Err(RemoteError::WouldOverwrite {
            path: plan.unit_path.clone(),
        });
    }

    let request = RemoteRequest::new(
        host.clone(),
        AuthMaterial {
            key_passphrase: passphrase,
        },
    );
    controller.apply_deployment(&request, plan, token).await
}

/// Turns a preflight, or the failure to obtain one, into the status the UI
/// shows next to a host.
pub fn status_from(result: &RemoteResult<Preflight>) -> RemoteStatus {
    match result {
        Ok(preflight) => RemoteStatus::Reachable {
            service: preflight.service_state.clone(),
            cloudflared_version: preflight.cloudflared_version.clone(),
            systemd_available: preflight.systemd_available,
            can_control_service: preflight.can_control(),
        },
        Err(error) => RemoteStatus::Unreachable {
            message: error.user_message(),
        },
    }
}

/// The instructions shown when remote control is not configured.
///
/// Offered rather than imposed: the point of this text is that a user who does
/// not want this application anywhere near their servers can still run the
/// connector themselves.
pub fn manual_setup_instructions(tunnel_id: &str) -> String {
    format!(
        "This application can control cloudflared over SSH, but it does not have to. \
         To run the connector yourself:\n\
         \n\
         1. Install cloudflared from https://pkg.cloudflare.com.\n\
         2. Copy this tunnel's run token — the Tunnel Details page has a button — and \
         save it on the server, readable only by root:\n\
         \x20     sudo install -d -m 0700 /etc/tunnel-manager\n\
         \x20     sudo install -m 0600 /dev/stdin /etc/tunnel-manager/{tunnel_id}.token\n\
         3. Run it:\n\
         \x20     sudo cloudflared tunnel run --token-file /etc/tunnel-manager/{tunnel_id}.token\n\
         \n\
         The tunnel's health on the Tunnels page comes from Cloudflare, so it will show \
         the connector appearing whether or not this application started it."
    )
}

/// The sudoers rule the Remote Hosts page offers to copy.
///
/// Scoped to one unit and one binary rather than blanket `NOPASSWD: ALL`,
/// because a host that trusts this application with everything is a worse
/// outcome than one that trusts it with a service restart.
pub fn sudoers_snippet(username: &str, unit: &str) -> Result<String, validation::ValidationError> {
    let username = validation::ssh_username(username)?;
    let unit = validation::systemd_unit(unit)?;

    Ok(format!(
        "# /etc/sudoers.d/tunnel-manager — install with:\n\
         #   sudo visudo -f /etc/sudoers.d/tunnel-manager\n\
         #\n\
         # This grants {username} exactly the commands Tunnel Manager uses and\n\
         # nothing else. It does not grant a shell, and it does not grant sudo\n\
         # for any other unit.\n\
         Cmnd_Alias TUNNEL_MANAGER = \\\n\
         \x20   /usr/bin/systemctl start {unit}, \\\n\
         \x20   /usr/bin/systemctl stop {unit}, \\\n\
         \x20   /usr/bin/systemctl restart {unit}, \\\n\
         \x20   /usr/bin/systemctl enable --now {unit}, \\\n\
         \x20   /usr/bin/systemctl daemon-reload, \\\n\
         \x20   /usr/bin/journalctl -u {unit} *\n\
         {username} ALL=(root) NOPASSWD: TUNNEL_MANAGER\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{MockCall, MockController, MockScript};
    use chrono::Utc;

    fn host(trusted: bool) -> RemoteHost {
        RemoteHost {
            id: 1,
            name: "edge".into(),
            address: "edge.example.test".into(),
            port: 22,
            username: "deploy".into(),
            auth: RemoteAuth::Agent,
            host_key_fingerprint: trusted
                .then(|| "SHA256:2WkKr2CzGCLPXKMWQTaEHwYCC6vLBpTOTOZ4dQ0Fx0Y".to_string()),
            service_unit: "cloudflared.service".into(),
            tunnel_id: Some("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415".into()),
            created_at: Utc::now(),
            last_seen_at: None,
            notes: None,
        }
    }

    fn controller() -> Arc<dyn RemoteConnectorController> {
        Arc::new(MockController::with_defaults())
    }

    #[test]
    fn a_draft_is_normalised_as_it_is_validated() {
        let draft = RemoteHostDraft {
            name: "  Edge  ".into(),
            address: "EDGE.example.TEST".into(),
            port: 2222,
            username: "deploy".into(),
            auth: RemoteAuth::Agent,
            service_unit: "cloudflared".into(),
            tunnel_id: Some("F70FF985-A4EF-4643-BBBC-4A0ED4FC8415".into()),
            notes: Some("   ".into()),
        };

        let clean = validate_draft(&draft).unwrap();
        assert_eq!(clean.name, "Edge");
        assert_eq!(clean.address, "edge.example.test");
        assert_eq!(clean.service_unit, "cloudflared.service");
        assert_eq!(
            clean.tunnel_id.as_deref(),
            Some("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415")
        );
        assert!(clean.notes.is_none(), "whitespace is not a note");
    }

    #[test]
    fn a_draft_with_a_hostile_unit_name_is_refused() {
        let draft = RemoteHostDraft {
            name: "edge".into(),
            address: "edge.example.test".into(),
            username: "deploy".into(),
            service_unit: "cloudflared.service; rm -rf /".into(),
            ..RemoteHostDraft::default()
        };
        assert!(validate_draft(&draft).is_err());
    }

    #[test]
    fn a_draft_with_a_user_at_host_address_is_refused() {
        let draft = RemoteHostDraft {
            name: "edge".into(),
            address: "root@edge.example.test".into(),
            username: "deploy".into(),
            service_unit: "cloudflared.service".into(),
            ..RemoteHostDraft::default()
        };
        assert!(validate_draft(&draft).is_err());
    }

    #[test]
    fn first_contact_is_a_decision_and_a_match_is_not() {
        let key = HostKey {
            fingerprint: "SHA256:2WkKr2CzGCLPXKMWQTaEHwYCC6vLBpTOTOZ4dQ0Fx0Y".into(),
            algorithm: "ssh-ed25519".into(),
        };

        let first = TrustDecision::evaluate(&host(false), key.clone());
        assert!(matches!(first, TrustDecision::FirstContact { .. }));
        assert!(!first.may_proceed(), "first contact must stop and ask");

        let unchanged = TrustDecision::evaluate(&host(true), key);
        assert_eq!(unchanged, TrustDecision::Unchanged);
        assert!(unchanged.may_proceed());
    }

    #[test]
    fn a_changed_host_key_never_permits_proceeding() {
        let decision = TrustDecision::evaluate(
            &host(true),
            HostKey {
                fingerprint: "SHA256:somethingelse".into(),
                algorithm: "ssh-ed25519".into(),
            },
        );

        assert!(matches!(decision, TrustDecision::Changed { .. }));
        assert!(!decision.may_proceed());

        let event = decision.activity(&host(true));
        assert_eq!(event.severity, crate::domain::ActivitySeverity::Error);
        assert!(event.summary.contains("Refused to connect"));
    }

    #[tokio::test]
    async fn no_operation_runs_against_an_untrusted_host() {
        let controller = controller();
        let untrusted = host(false);

        assert!(matches!(
            preflight(&controller, &untrusted, None).await,
            Err(RemoteError::NotConfigured)
        ));
        assert!(matches!(
            service_action(&controller, &untrusted, None, ServiceAction::Restart).await,
            Err(RemoteError::NotConfigured)
        ));
        assert!(matches!(
            logs(&controller, &untrusted, None, 10).await,
            Err(RemoteError::NotConfigured)
        ));
        assert!(matches!(
            plan_deployment(&controller, &untrusted, None, "t", "n").await,
            Err(RemoteError::NotConfigured)
        ));
    }

    #[tokio::test]
    async fn a_preflight_becomes_a_reachable_status() {
        let controller = controller();
        let result = preflight(&controller, &host(true), None).await;
        let status = status_from(&result);

        assert!(!status.is_unreachable());
        assert!(status.can_control());
        assert_eq!(status.label(), "Running");
    }

    #[tokio::test]
    async fn a_failed_preflight_becomes_an_unreachable_status_with_the_reason() {
        let controller: Arc<dyn RemoteConnectorController> =
            Arc::new(MockController::new(MockScript {
                fail_with: Some(RemoteError::Timeout),
                ..MockScript::default()
            }));

        let result = preflight(&controller, &host(true), None).await;
        let status = status_from(&result);
        assert!(status.is_unreachable());
        assert!(!status.can_control());
        match status {
            RemoteStatus::Unreachable { message } => {
                assert!(message.contains("did not respond"))
            }
            other => panic!("expected unreachable, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_deployment_that_would_overwrite_a_unit_needs_explicit_confirmation() {
        let controller = controller();
        let host = host(true);
        let mut plan = plan_deployment(
            &controller,
            &host,
            None,
            "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415",
            "web",
        )
        .await
        .unwrap();
        plan.replaces_existing_unit = true;

        let refused = apply_deployment(
            &controller,
            &host,
            None,
            &plan,
            &SecretString::from("token"),
            false,
        )
        .await;
        assert!(matches!(refused, Err(RemoteError::WouldOverwrite { .. })));

        let allowed = apply_deployment(
            &controller,
            &host,
            None,
            &plan,
            &SecretString::from("token"),
            true,
        )
        .await;
        assert!(allowed.unwrap().is_complete());
    }

    #[test]
    fn stopping_the_only_connector_says_the_tunnel_goes_offline() {
        let warning = action_warning(ServiceAction::Stop, "web", 0).unwrap();
        assert!(warning.contains("only connector"));
        assert!(warning.contains("offline"));
        assert!(
            warning.contains("not deleted"),
            "a user must not think stopping a connector deletes the tunnel"
        );
    }

    #[test]
    fn stopping_one_of_several_connectors_says_the_tunnel_stays_up() {
        let warning = action_warning(ServiceAction::Stop, "web", 2).unwrap();
        assert!(warning.contains("2 other connector(s)"));
        assert!(warning.contains("stays up"));
    }

    #[test]
    fn starting_needs_no_warning() {
        assert!(action_warning(ServiceAction::Start, "web", 0).is_none());
    }

    #[test]
    fn restarting_distinguishes_the_single_connector_case() {
        let alone = action_warning(ServiceAction::Restart, "web", 0).unwrap();
        assert!(alone.contains("interrupts traffic"));

        let redundant = action_warning(ServiceAction::Restart, "web", 3).unwrap();
        assert!(redundant.contains("should not be interrupted"));
    }

    #[test]
    fn the_sudoers_snippet_is_scoped_to_one_unit() {
        let snippet = sudoers_snippet("deploy", "cloudflared.service").unwrap();

        assert!(snippet.contains("NOPASSWD: TUNNEL_MANAGER"));
        assert!(!snippet.contains("NOPASSWD: ALL"), "never blanket sudo");
        assert!(snippet.contains("systemctl restart cloudflared.service"));
        assert!(snippet.contains("journalctl -u cloudflared.service"));
        assert!(!snippet.contains("/bin/sh"), "no shell");
        assert!(!snippet.contains("/bin/bash"));
    }

    #[test]
    fn the_sudoers_snippet_refuses_hostile_input() {
        assert!(sudoers_snippet("deploy", "x.service, ALL").is_err());
        assert!(sudoers_snippet("root ALL=(ALL)", "cloudflared.service").is_err());
    }

    #[test]
    fn manual_instructions_never_include_a_token_and_use_the_token_file() {
        let text = manual_setup_instructions("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415");
        assert!(text.contains("--token-file"));
        assert!(text.contains("0600"));
        assert!(text.contains("does not have to"));
        assert!(!text.contains("--token "));
    }

    #[tokio::test]
    async fn the_protocol_can_be_read_back_after_it_is_pinned() {
        use crate::domain::host::TransportProtocol;

        let controller = controller();
        let host = host(true);

        assert_eq!(
            read_transport(&controller, &host, None).await.unwrap(),
            None,
            "nothing is pinned to begin with"
        );

        let outcome = set_transport(&controller, &host, None, Some(TransportProtocol::Http2))
            .await
            .unwrap();
        assert_eq!(outcome.protocol, Some(TransportProtocol::Http2));
        assert_eq!(outcome.service_state, RemoteServiceState::Active);

        assert_eq!(
            read_transport(&controller, &host, None).await.unwrap(),
            Some(TransportProtocol::Http2)
        );

        let cleared = set_transport(&controller, &host, None, None).await.unwrap();
        assert_eq!(cleared.protocol, None);
        assert_eq!(
            read_transport(&controller, &host, None).await.unwrap(),
            None,
            "removing the drop-in unpins the protocol"
        );
    }

    #[tokio::test]
    async fn pinning_the_protocol_restarts_the_service() {
        use crate::domain::host::TransportProtocol;

        let mock = Arc::new(MockController::new(MockScript {
            service_state: RemoteServiceState::Active,
            ..MockScript::default()
        }));
        let controller: Arc<dyn RemoteConnectorController> = mock.clone();
        let host = host(true);

        set_transport(&controller, &host, None, Some(TransportProtocol::Quic))
            .await
            .unwrap();

        assert!(
            mock.calls()
                .contains(&MockCall::Service(ServiceAction::Restart)),
            "a drop-in only takes effect on restart, so the restart must have run"
        );
        assert!(
            mock.call_count() >= 4,
            "make dir, install, daemon-reload and restart all ran"
        );
    }

    #[tokio::test]
    async fn an_untrusted_host_cannot_have_its_protocol_changed() {
        use crate::domain::host::TransportProtocol;

        let controller = controller();
        let untrusted = host(false);

        assert!(matches!(
            read_transport(&controller, &untrusted, None).await,
            Err(RemoteError::NotConfigured)
        ));
        assert!(matches!(
            set_transport(
                &controller,
                &untrusted,
                None,
                Some(TransportProtocol::Http2)
            )
            .await,
            Err(RemoteError::NotConfigured)
        ));
    }
}
