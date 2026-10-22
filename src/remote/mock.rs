//! A scriptable [`RemoteConnectorController`] for tests.
//!
//! Compiled into the binary rather than gated behind `#[cfg(test)]` because
//! the integration tests in `tests/` link the crate as an external consumer
//! and need it too. It has no network access and no `unsafe`, so the cost of
//! shipping it is a few kilobytes and the benefit is that the whole remote
//! layer — including the host-key rules — is exercised by `cargo test` with no
//! SSH server anywhere.

use std::sync::{Arc, Mutex};

use secrecy::{ExposeSecret, SecretString};

use crate::domain::host::RemoteServiceState;

use super::commands::ServiceAction;
use super::controller::{
    DeploymentOutcome, DeploymentPlan, DeploymentStep, HostKey, Preflight,
    RemoteConnectorController, RemoteFuture, RemoteRequest, ServiceOutcome,
};
use super::error::RemoteError;

/// What the mock did, so a test can assert on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    ProbeHostKey,
    Preflight,
    Service(ServiceAction),
    Logs(u16),
    PlanDeployment {
        tunnel_id: String,
    },
    ApplyDeployment {
        tunnel_id: String,
    },
    /// One allowlisted command run through the seam the protocol switch
    /// uses. Identified by its description, which is unique per command.
    RunAllowlisted(&'static str),
}

/// How the mock should behave.
#[derive(Clone)]
pub struct MockScript {
    pub host_key: HostKey,
    pub preflight: Preflight,
    /// The state the service is in, updated by service actions so a test can
    /// assert on a transition rather than on a single call.
    pub service_state: RemoteServiceState,
    /// The transport protocol the unit is pinned to, as
    /// `UnitEnvironment` reports it. `None` means the unit has no override.
    pub protocol: Option<crate::domain::host::TransportProtocol>,
    pub logs: Vec<String>,
    /// An error to return instead of doing anything. Set per test.
    pub fail_with: Option<RemoteError>,
    /// Whether a service action should refuse to change state, which is how a
    /// unit that crashes on start is simulated.
    pub service_action_sticks: bool,
}

impl Default for MockScript {
    fn default() -> Self {
        Self {
            host_key: HostKey {
                fingerprint: "SHA256:2WkKr2CzGCLPXKMWQTaEHwYCC6vLBpTOTOZ4dQ0Fx0Y".into(),
                algorithm: "ssh-ed25519".into(),
            },
            preflight: Preflight {
                reachable: true,
                host_key_matched: true,
                is_linux: true,
                kernel: Some("Linux".into()),
                systemd_available: true,
                systemd_version: Some("systemd 257 (257.4)".into()),
                cloudflared_path: Some("/usr/bin/cloudflared".into()),
                cloudflared_version: Some("2026.1.0".into()),
                supports_token_file: true,
                unit_present: true,
                service_state: RemoteServiceState::Active,
                service_enabled: Some(true),
                can_control_service: true,
                configured_tunnel_id: None,
                notes: vec![],
            },
            service_state: RemoteServiceState::Active,
            protocol: None,
            logs: vec![
                "2026-08-22T10:00:00+0000 host cloudflared[1]: Registered tunnel connection".into(),
            ],
            service_action_sticks: false,
            fail_with: None,
        }
    }
}

/// The controller itself.
#[derive(Clone, Default)]
pub struct MockController {
    script: Arc<Mutex<MockScript>>,
    calls: Arc<Mutex<Vec<MockCall>>>,
    /// Every secret handed to the mock, so a test can prove a token reached
    /// the transport — and, more usefully, prove it did not reach anything
    /// else.
    received_secrets: Arc<Mutex<Vec<String>>>,
}

impl MockController {
    pub fn new(script: MockScript) -> Self {
        Self {
            script: Arc::new(Mutex::new(script)),
            calls: Arc::new(Mutex::new(Vec::new())),
            received_secrets: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(MockScript::default())
    }

    /// Replaces the script mid-test, for simulating a host that changes.
    pub fn set_script(&self, script: MockScript) {
        if let Ok(mut current) = self.script.lock() {
            *current = script;
        }
    }

    pub fn calls(&self) -> Vec<MockCall> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    pub fn call_count(&self) -> usize {
        self.calls
            .lock()
            .map(|calls| calls.len())
            .unwrap_or_default()
    }

    pub fn received_secrets(&self) -> Vec<String> {
        self.received_secrets
            .lock()
            .map(|secrets| secrets.clone())
            .unwrap_or_default()
    }

    pub fn service_state(&self) -> RemoteServiceState {
        self.script
            .lock()
            .map(|script| script.service_state.clone())
            .unwrap_or(RemoteServiceState::NotInstalled)
    }

    fn record(&self, call: MockCall) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call);
        }
    }

    fn scripted_failure(&self) -> Option<RemoteError> {
        self.script
            .lock()
            .ok()
            .and_then(|script| script.fail_with.clone())
    }
}

impl RemoteConnectorController for MockController {
    fn probe_host_key<'a>(&'a self, _request: &'a RemoteRequest) -> RemoteFuture<'a, HostKey> {
        Box::pin(async move {
            self.record(MockCall::ProbeHostKey);
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }
            Ok(self
                .script
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .host_key
                .clone())
        })
    }

    fn preflight<'a>(&'a self, request: &'a RemoteRequest) -> RemoteFuture<'a, Preflight> {
        Box::pin(async move {
            self.record(MockCall::Preflight);
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }
            // The real controller refuses to connect to a host with no pinned
            // key; the mock enforces the same rule so a test cannot pass
            // against behaviour the real one would reject.
            if request.host.host_key_fingerprint.is_none() {
                return Err(RemoteError::NotConfigured);
            }

            let script = self
                .script
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut preflight = script.preflight.clone();
            preflight.service_state = script.service_state.clone();
            Ok(preflight)
        })
    }

    fn service_action<'a>(
        &'a self,
        request: &'a RemoteRequest,
        action: ServiceAction,
    ) -> RemoteFuture<'a, ServiceOutcome> {
        Box::pin(async move {
            self.record(MockCall::Service(action));
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }
            if request.host.host_key_fingerprint.is_none() {
                return Err(RemoteError::NotConfigured);
            }

            let mut script = self
                .script
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let state_before = script.service_state.clone();

            if !script.service_action_sticks {
                script.service_state = match action {
                    ServiceAction::Start | ServiceAction::Restart => RemoteServiceState::Active,
                    ServiceAction::Stop => RemoteServiceState::Inactive,
                };
            }

            Ok(ServiceOutcome {
                action,
                state_before,
                state_after: script.service_state.clone(),
                message: None,
            })
        })
    }

    fn logs<'a>(
        &'a self,
        _request: &'a RemoteRequest,
        lines: u16,
    ) -> RemoteFuture<'a, Vec<String>> {
        Box::pin(async move {
            self.record(MockCall::Logs(lines));
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }
            let script = self
                .script
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Ok(script
                .logs
                .iter()
                .rev()
                .take(usize::from(lines.max(1)))
                .rev()
                .cloned()
                .collect())
        })
    }

    fn run_allowlisted<'a>(
        &'a self,
        request: &'a RemoteRequest,
        command: super::RemoteCommand,
        stdin: Option<&'a str>,
    ) -> RemoteFuture<'a, String> {
        Box::pin(async move {
            self.record(MockCall::RunAllowlisted(command.description()));
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }
            if request.host.host_key_fingerprint.is_none() {
                return Err(RemoteError::NotConfigured);
            }

            match command {
                // The one command whose output anyone parses: answer with the
                // protocol the script says the unit is pinned to, so a test
                // can drive the switch end to end.
                super::RemoteCommand::UnitEnvironment { .. } => {
                    let protocol = self
                        .script
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .protocol
                        .map(|protocol| {
                            format!("TUNNEL_TRANSPORT_PROTOCOL={}", protocol.env_value())
                        })
                        .unwrap_or_default();
                    Ok(protocol)
                }
                // The drop-in write and its undo, tracked so a test can read
                // the state back through `UnitEnvironment` afterwards.
                super::RemoteCommand::InstallFromStdin { path, .. } if path.ends_with(".conf") => {
                    let pinned =
                        stdin.and_then(crate::domain::host::TransportProtocol::from_env_output);
                    self.script
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .protocol = pinned;
                    Ok(String::new())
                }
                super::RemoteCommand::RemoveFile { path } if path.ends_with(".conf") => {
                    self.script
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .protocol = None;
                    Ok(String::new())
                }
                _ => Ok(String::new()),
            }
        })
    }

    fn plan_deployment<'a>(
        &'a self,
        request: &'a RemoteRequest,
        tunnel_id: &'a str,
        tunnel_name: &'a str,
    ) -> RemoteFuture<'a, DeploymentPlan> {
        Box::pin(async move {
            self.record(MockCall::PlanDeployment {
                tunnel_id: tunnel_id.to_string(),
            });
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }

            let unit = request.host.service_unit.clone();
            let token_path = format!("{}/{tunnel_id}.token", super::ssh::TOKEN_DIR);
            let unit_path = format!("/etc/systemd/system/{unit}");

            Ok(DeploymentPlan {
                tunnel_id: tunnel_id.to_string(),
                tunnel_name: tunnel_name.to_string(),
                unit: unit.clone(),
                token_path: token_path.clone(),
                unit_path: unit_path.clone(),
                cloudflared_path: "/usr/bin/cloudflared".into(),
                steps: vec![
                    DeploymentStep {
                        description: "Write the tunnel's run token".into(),
                        preview: format!("sudo -n install -m 0600 /dev/stdin '{token_path}'"),
                        privileged: true,
                        overwrites: false,
                    },
                    DeploymentStep {
                        description: "Write the systemd unit".into(),
                        preview: format!("sudo -n install -m 0644 /dev/stdin '{unit_path}'"),
                        privileged: true,
                        overwrites: false,
                    },
                    DeploymentStep {
                        description: format!("Enable and start {unit}"),
                        preview: format!("sudo -n systemctl enable --now '{unit}'"),
                        privileged: true,
                        overwrites: false,
                    },
                ],
                unit_file: format!(
                    "[Service]\nExecStart=/usr/bin/cloudflared tunnel run --token-file {token_path}\n"
                ),
                replaces_existing_unit: false,
            })
        })
    }

    fn apply_deployment<'a>(
        &'a self,
        _request: &'a RemoteRequest,
        plan: &'a DeploymentPlan,
        token: &'a SecretString,
    ) -> RemoteFuture<'a, DeploymentOutcome> {
        Box::pin(async move {
            self.record(MockCall::ApplyDeployment {
                tunnel_id: plan.tunnel_id.clone(),
            });
            if let Ok(mut secrets) = self.received_secrets.lock() {
                secrets.push(token.expose_secret().to_string());
            }
            if let Some(error) = self.scripted_failure() {
                return Err(error);
            }

            let mut script = self
                .script
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            script.service_state = RemoteServiceState::Active;

            Ok(DeploymentOutcome {
                completed: plan
                    .steps
                    .iter()
                    .map(|step| step.description.clone())
                    .collect(),
                failed: None,
                service_state: RemoteServiceState::Active,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::host::{RemoteAuth, RemoteHost};
    use crate::remote::controller::AuthMaterial;

    fn host(trusted: bool) -> RemoteHost {
        RemoteHost {
            id: 1,
            name: "edge".into(),
            address: "edge.example.test".into(),
            port: 22,
            username: "deploy".into(),
            auth: RemoteAuth::Agent,
            host_key_fingerprint: trusted.then(|| "SHA256:trusted".to_string()),
            service_unit: "cloudflared.service".into(),
            tunnel_id: Some("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415".into()),
            created_at: chrono::Utc::now(),
            last_seen_at: None,
            notes: None,
        }
    }

    fn request(trusted: bool) -> RemoteRequest {
        RemoteRequest::new(host(trusted), AuthMaterial::default())
    }

    #[tokio::test]
    async fn an_untrusted_host_cannot_be_operated_on() {
        let controller = MockController::with_defaults();
        let request = request(false);

        assert!(matches!(
            controller.preflight(&request).await,
            Err(RemoteError::NotConfigured)
        ));
        assert!(matches!(
            controller
                .service_action(&request, ServiceAction::Restart)
                .await,
            Err(RemoteError::NotConfigured)
        ));
    }

    #[tokio::test]
    async fn probing_a_host_key_works_before_it_is_trusted() {
        let controller = MockController::with_defaults();
        let key = controller.probe_host_key(&request(false)).await.unwrap();
        assert!(key.fingerprint.starts_with("SHA256:"));
        assert_eq!(controller.calls(), vec![MockCall::ProbeHostKey]);
    }

    #[tokio::test]
    async fn service_actions_move_the_state() {
        let controller = MockController::with_defaults();
        let request = request(true);

        let stopped = controller
            .service_action(&request, ServiceAction::Stop)
            .await
            .unwrap();
        assert_eq!(stopped.state_before, RemoteServiceState::Active);
        assert_eq!(stopped.state_after, RemoteServiceState::Inactive);
        assert!(stopped.succeeded());

        let started = controller
            .service_action(&request, ServiceAction::Start)
            .await
            .unwrap();
        assert_eq!(started.state_before, RemoteServiceState::Inactive);
        assert_eq!(started.state_after, RemoteServiceState::Active);
        assert!(started.succeeded());

        assert_eq!(
            controller.calls(),
            vec![
                MockCall::Service(ServiceAction::Stop),
                MockCall::Service(ServiceAction::Start)
            ]
        );
    }

    #[tokio::test]
    async fn a_service_that_will_not_start_is_reported_as_failed() {
        let controller = MockController::new(MockScript {
            service_state: RemoteServiceState::Failed,
            service_action_sticks: true,
            ..MockScript::default()
        });

        let outcome = controller
            .service_action(&request(true), ServiceAction::Start)
            .await
            .unwrap();
        assert!(!outcome.succeeded());
        assert_eq!(outcome.state_after, RemoteServiceState::Failed);
    }

    #[tokio::test]
    async fn preflight_reflects_the_current_service_state() {
        let controller = MockController::with_defaults();
        let request = request(true);

        controller
            .service_action(&request, ServiceAction::Stop)
            .await
            .unwrap();
        let preflight = controller.preflight(&request).await.unwrap();
        assert_eq!(preflight.service_state, RemoteServiceState::Inactive);
        assert!(preflight.can_control());
    }

    #[tokio::test]
    async fn a_scripted_failure_is_returned_from_every_method() {
        let controller = MockController::new(MockScript {
            fail_with: Some(RemoteError::Timeout),
            ..MockScript::default()
        });
        let request = request(true);

        assert!(controller.preflight(&request).await.is_err());
        assert!(controller.logs(&request, 10).await.is_err());
        assert!(controller
            .service_action(&request, ServiceAction::Start)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn logs_are_bounded_by_the_requested_line_count() {
        let controller = MockController::new(MockScript {
            logs: (0..10).map(|index| format!("line {index}")).collect(),
            ..MockScript::default()
        });

        let lines = controller.logs(&request(true), 3).await.unwrap();
        assert_eq!(lines, vec!["line 7", "line 8", "line 9"]);
        assert_eq!(controller.calls(), vec![MockCall::Logs(3)]);
    }

    #[tokio::test]
    async fn deployment_records_the_token_it_was_given() {
        let controller = MockController::with_defaults();
        let request = request(true);

        let plan = controller
            .plan_deployment(&request, "f70ff985-a4ef-4643-bbbc-4a0ed4fc8415", "web")
            .await
            .unwrap();

        // The preview must never contain the token.
        for step in &plan.steps {
            assert!(!step.preview.contains("eyJ"), "{}", step.preview);
        }
        assert!(plan.touches_privileged_files());

        let outcome = controller
            .apply_deployment(&request, &plan, &SecretString::from("eyJhIjoieCJ9"))
            .await
            .unwrap();

        assert!(outcome.is_complete());
        assert_eq!(outcome.service_state, RemoteServiceState::Active);
        assert_eq!(controller.received_secrets(), vec!["eyJhIjoieCJ9"]);
    }
}
