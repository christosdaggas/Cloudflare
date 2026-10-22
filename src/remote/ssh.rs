//! The SSH transport, on `russh`.
//!
//! Host-key handling is the load-bearing part of this file, so it is worth
//! stating plainly what it does and does not do:
//!
//! * [`PinnedHostKey::check_server_key`] returns `Ok(false)` — which aborts
//!   the handshake before authentication — unless the key's SHA-256
//!   fingerprint equals the one recorded for the host. There is no
//!   "accept anything" mode, no configuration flag that disables the check,
//!   and no code path that writes a fingerprint without the user having been
//!   shown it.
//! * A host with no recorded fingerprint cannot be connected to at all. The
//!   first contact goes through [`SshController::probe_host_key`], which
//!   reads the key, tears the connection down and returns the fingerprint for
//!   the user to approve. No credential is offered during a probe.
//!
//! Everything else follows from [`super::commands`]: the only strings this
//! module sends are the ones that enum can render.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, Handler};
use russh::keys::ssh_key::public::KeyData;
use russh::keys::ssh_key::{HashAlg, PublicKey};
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, Disconnect};
use secrecy::{ExposeSecret, SecretString};

use crate::domain::activity::redact;
use crate::domain::host::{RemoteAuth, RemoteHost, RemoteServiceState};
use crate::domain::validation;

use super::commands::{RemoteCommand, ServiceAction, DEFAULT_LOG_LINES};
use super::controller::{
    AuthMaterial, DeploymentOutcome, DeploymentPlan, DeploymentStep, HostKey, Preflight,
    RemoteConnectorController, RemoteFuture, RemoteRequest, ServiceOutcome,
};
use super::error::{RemoteError, RemoteResult};

/// How long to wait for a TCP connection and a handshake.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a single remote command may take.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The directory this application keeps a tunnel token in, when it deploys
/// one. Under `/etc` rather than a home directory so that the file can be
/// root-owned and mode 0600 with nothing else able to read it.
pub const TOKEN_DIR: &str = "/etc/tunnel-manager";

/// The minimum `cloudflared` that understands `--token-file`. Older builds can
/// only take the token on the command line or in the environment, both of
/// which put it where `ps` can see it, so this application refuses to deploy
/// to them.
const TOKEN_FILE_MIN_VERSION: (u32, u32) = (2025, 4);

/// The `russh` handler. Its only job is the host-key decision.
struct PinnedHostKey {
    /// The fingerprint the user trusted, or `None` while probing.
    expected: Option<String>,
    /// What the host actually presented, recorded for the caller either way.
    seen: Arc<Mutex<Option<HostKey>>>,
}

impl Handler for PinnedHostKey {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // A certificate and a plain key expose their bytes through different
        // types; normalising on `KeyData` is what lets one fingerprint
        // calculation cover both.
        let (key_data, algorithm): (&KeyData, String) = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                (key.key_data(), key.algorithm().to_string())
            }
            PublicKeyOrCertificate::Certificate(certificate) => (
                certificate.public_key(),
                certificate.algorithm().to_string(),
            ),
        };

        let fingerprint = key_data.fingerprint(HashAlg::Sha256).to_string();

        if let Ok(mut seen) = self.seen.lock() {
            *seen = Some(HostKey {
                fingerprint: fingerprint.clone(),
                algorithm,
            });
        }

        match &self.expected {
            // Probing. Accept the handshake so the key can be read, then the
            // caller disconnects without ever authenticating.
            None => Ok(true),
            // Returning Ok(false) aborts before any credential is offered.
            Some(expected) => Ok(fingerprint == *expected),
        }
    }
}

/// The output of one remote command.
struct Output {
    stdout: String,
    stderr: String,
    exit_status: Option<u32>,
    /// True when the 512 KiB cap stopped buffering mid-stream. The text is
    /// a prefix, never a complete answer.
    truncated: bool,
}

impl Output {
    fn ok(&self) -> bool {
        // `None` means the channel closed without an `ExitStatus` — a
        // truncated or killed command, never a success.
        matches!(self.exit_status, Some(0)) && !self.truncated
    }

    fn trimmed(&self) -> &str {
        self.stdout.trim()
    }

    fn lines(&self) -> Vec<String> {
        self.stdout
            .lines()
            .map(|line| line.trim_end().to_string())
            .collect()
    }
}

/// The real controller.
#[derive(Debug, Default)]
pub struct SshController;

impl SshController {
    pub fn new() -> Self {
        Self
    }

    fn config() -> Arc<client::Config> {
        Arc::new(client::Config {
            inactivity_timeout: Some(Duration::from_secs(120)),
            keepalive_interval: Some(Duration::from_secs(30)),
            nodelay: true,
            ..Default::default()
        })
    }

    /// Opens a session and authenticates.
    async fn connect(request: &RemoteRequest) -> RemoteResult<Handle<PinnedHostKey>> {
        let host = &request.host;

        let Some(expected) = host.host_key_fingerprint.clone() else {
            // Reaching here would mean an operation was attempted on a host
            // whose key was never approved. That is a bug, and it is caught
            // rather than papered over.
            return Err(RemoteError::NotConfigured);
        };

        let address = validation::ssh_address(&host.address)?;
        let seen = Arc::new(Mutex::new(None));
        let handler = PinnedHostKey {
            expected: Some(expected.clone()),
            seen: Arc::clone(&seen),
        };

        let connect = client::connect(Self::config(), (address.as_str(), host.port), handler);
        let mut session = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
            Err(_) => return Err(RemoteError::Timeout),
            Ok(Ok(session)) => session,
            Ok(Err(error)) => {
                // A rejected host key surfaces from russh as a generic
                // handshake failure, so the recorded key is what tells the two
                // apart — and a mismatch has to be reported as a mismatch, not
                // as "could not connect".
                let actual = seen.lock().ok().and_then(|seen| seen.clone());
                if let Some(actual) = actual {
                    if actual.fingerprint != expected {
                        return Err(RemoteError::HostKeyMismatch {
                            expected,
                            actual: actual.fingerprint,
                            algorithm: actual.algorithm,
                        });
                    }
                }
                return Err(RemoteError::Connect {
                    detail: redact(&error.to_string()),
                });
            }
        };

        request
            .cancel
            .is_cancelled()
            .then_some(())
            .map_or(Ok(()), |()| Err(RemoteError::Cancelled))?;

        Self::authenticate(&mut session, host, &request.auth).await?;
        Ok(session)
    }

    async fn authenticate(
        session: &mut Handle<PinnedHostKey>,
        host: &RemoteHost,
        auth: &AuthMaterial,
    ) -> RemoteResult<()> {
        let username = validation::ssh_username(&host.username)?;

        match &host.auth {
            RemoteAuth::Agent => Self::authenticate_with_agent(session, &username).await,
            RemoteAuth::KeyFile { path, .. } => {
                Self::authenticate_with_key_file(session, &username, path, auth).await
            }
        }
    }

    async fn authenticate_with_agent(
        session: &mut Handle<PinnedHostKey>,
        username: &str,
    ) -> RemoteResult<()> {
        use russh::keys::agent::client::AgentClient;
        use russh::keys::agent::AgentIdentity;

        let mut agent =
            AgentClient::connect_env()
                .await
                .map_err(|error| RemoteError::AgentUnavailable {
                    detail: redact(&error.to_string()),
                })?;

        let identities =
            agent
                .request_identities()
                .await
                .map_err(|error| RemoteError::AgentUnavailable {
                    detail: redact(&error.to_string()),
                })?;

        if identities.is_empty() {
            return Err(RemoteError::AgentUnavailable {
                detail: "the agent holds no keys".into(),
            });
        }

        for identity in identities {
            let public_key: PublicKey = match &identity {
                AgentIdentity::PublicKey { key, .. } => key.clone(),
                AgentIdentity::Certificate { certificate, .. } => {
                    PublicKey::new(certificate.public_key().clone(), certificate.comment())
                }
            };

            let hash_alg = if public_key.algorithm().is_rsa() {
                session
                    .best_supported_rsa_hash()
                    .await
                    .ok()
                    .flatten()
                    .flatten()
            } else {
                None
            };

            match session
                .authenticate_publickey_with(username, public_key, hash_alg, &mut agent)
                .await
            {
                Ok(result) if result.success() => return Ok(()),
                // A key the server does not know is the normal case when the
                // agent holds several; try the next one.
                _ => continue,
            }
        }

        Err(RemoteError::Auth {
            detail: "no key offered by the agent was accepted".into(),
            tried: vec!["ssh-agent".into()],
        })
    }

    async fn authenticate_with_key_file(
        session: &mut Handle<PinnedHostKey>,
        username: &str,
        path: &std::path::Path,
        auth: &AuthMaterial,
    ) -> RemoteResult<()> {
        let display = path.display().to_string();
        let passphrase = auth
            .key_passphrase
            .as_ref()
            .map(|secret| secret.expose_secret().to_string());

        let key = match load_secret_key(path, passphrase.as_deref()) {
            Ok(key) => key,
            Err(error) => {
                let text = error.to_string();
                // russh reports an encrypted key without a passphrase as a
                // decryption failure; saying "it needs a passphrase" is the
                // only version of that a user can act on.
                if passphrase.is_none()
                    && (text.contains("passphrase")
                        || text.contains("crypt")
                        || text.contains("Encrypted"))
                {
                    return Err(RemoteError::PassphraseRequired { path: display });
                }
                return Err(RemoteError::KeyFile {
                    path: display,
                    detail: redact(&text),
                });
            }
        };

        let hash_alg = if key.algorithm().is_rsa() {
            session
                .best_supported_rsa_hash()
                .await
                .ok()
                .flatten()
                .flatten()
        } else {
            None
        };

        let result = session
            .authenticate_publickey(
                username,
                PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
            )
            .await
            .map_err(|error| RemoteError::Auth {
                detail: redact(&error.to_string()),
                tried: vec!["a private key file".into()],
            })?;

        if result.success() {
            Ok(())
        } else {
            Err(RemoteError::Auth {
                detail: "the server rejected the key".into(),
                tried: vec!["a private key file".into()],
            })
        }
    }

    /// Runs one allowlisted command, optionally feeding it stdin.
    ///
    /// `stdin` exists for exactly one purpose: writing the tunnel token into
    /// `install`'s standard input so it never appears in a command line. It is
    /// a [`SecretString`] so nothing can print it by accident.
    async fn run(
        session: &Handle<PinnedHostKey>,
        command: &RemoteCommand,
        stdin: Option<&SecretString>,
        cancel: &crate::cloudflare::Cancel,
    ) -> RemoteResult<Output> {
        if cancel.is_cancelled() {
            return Err(RemoteError::Cancelled);
        }

        let line = command.render()?;

        let execute = async {
            let mut channel =
                session
                    .channel_open_session()
                    .await
                    .map_err(|error| RemoteError::Connect {
                        detail: redact(&error.to_string()),
                    })?;

            channel
                .exec(true, line.as_bytes())
                .await
                .map_err(|error| RemoteError::Connect {
                    detail: redact(&error.to_string()),
                })?;

            if let Some(stdin) = stdin {
                channel
                    .data(stdin.expose_secret().as_bytes())
                    .await
                    .map_err(|error| RemoteError::Connect {
                        detail: redact(&error.to_string()),
                    })?;
                // EOF is what tells `install` the file is complete; without it
                // the command waits forever.
                channel.eof().await.map_err(|error| RemoteError::Connect {
                    detail: redact(&error.to_string()),
                })?;
            }

            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let mut exit_status = None;

            // `ExitStatus` arrives before `Close` and after the last data, so
            // the loop runs to `Close` rather than stopping at `Eof`.
            let mut truncated = false;
            while let Some(message) = channel.wait().await {
                match message {
                    ChannelMsg::Data { data } => stdout.extend_from_slice(&data),
                    // ext 1 is SSH_EXTENDED_DATA_STDERR; nothing else is
                    // defined, and anything else is not ours to interpret.
                    ChannelMsg::ExtendedData { data, ext: 1 } => stderr.extend_from_slice(&data),
                    ChannelMsg::ExitStatus {
                        exit_status: status,
                    } => exit_status = Some(status),
                    ChannelMsg::Close => break,
                    _ => {}
                }

                // A command that produced megabytes is either a mistake or an
                // attack; either way there is no reason to buffer it. The
                // caller is told it only got a prefix.
                if stdout.len() + stderr.len() > 512 * 1024 {
                    truncated = true;
                    break;
                }
            }

            Ok::<_, RemoteError>(Output {
                stdout: String::from_utf8_lossy(&stdout).into_owned(),
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
                exit_status,
                truncated,
            })
        };

        match tokio::time::timeout(COMMAND_TIMEOUT, execute).await {
            Err(_) => Err(RemoteError::Timeout),
            Ok(result) => result,
        }
    }

    async fn disconnect(session: Handle<PinnedHostKey>) {
        let _ = session
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    }
}

/// Parses `systemctl show --value` output, refusing a truncated prefix: three
/// parsed lines out of a cut-off stream would report the wrong state.
fn checked_service_state(
    output: &Output,
) -> RemoteResult<(RemoteServiceState, Option<bool>, bool)> {
    if output.truncated {
        return Err(RemoteError::CommandFailed {
            description: "read the service state",
            exit_status: output.exit_status,
            stderr: "the host returned more than 512 KiB; the state could not be read".into(),
        });
    }
    Ok(parse_service_state(output))
}

/// Parses `systemctl show --value` output: three lines, in the order asked.
fn parse_service_state(output: &Output) -> (RemoteServiceState, Option<bool>, bool) {
    let lines = output.lines();
    let load = lines.first().map(String::as_str).unwrap_or_default();
    let active = lines.get(1).map(String::as_str).unwrap_or_default();
    let enabled = lines.get(2).map(String::as_str).unwrap_or_default();

    let state = RemoteServiceState::from_systemctl(active, load);
    let is_enabled = match enabled.trim() {
        "enabled" | "enabled-runtime" | "static" | "indirect" => Some(true),
        "disabled" | "masked" => Some(false),
        _ => None,
    };
    let present = load.trim() != "not-found" && !load.trim().is_empty();

    (state, is_enabled, present)
}

/// Reads `2026.1.0` out of whatever `cloudflared --version` printed.
fn parse_cloudflared_version(text: &str) -> Option<String> {
    text.split_whitespace()
        .find(|word| {
            let mut parts = word.split('.');
            matches!(
                (parts.next(), parts.next()),
                (Some(year), Some(month))
                    if year.len() == 4
                        && year.chars().all(|c| c.is_ascii_digit())
                        && !month.is_empty()
                        && month.chars().all(|c| c.is_ascii_digit())
            )
        })
        .map(|version| version.trim_start_matches('v').to_string())
}

/// Whether a version string is at least the release that added
/// `--token-file`.
pub fn supports_token_file(version: &str) -> bool {
    let mut parts = version.split('.');
    let year = parts.next().and_then(|part| part.parse::<u32>().ok());
    let month = parts.next().and_then(|part| part.parse::<u32>().ok());

    match (year, month) {
        (Some(year), Some(month)) => {
            (year, month) >= (TOKEN_FILE_MIN_VERSION.0, TOKEN_FILE_MIN_VERSION.1)
        }
        _ => false,
    }
}

/// Pulls the tunnel ID out of a unit's `ExecStart`, when the unit was written
/// by this application (the token file is named after the tunnel).
///
/// Only matches `--token-file <dir>/<uuid>.token`: any other `*.token`
/// basename (a backup, a different tool's file) is not ours to interpret.
fn tunnel_id_from_exec_start(exec_start: &str) -> Option<String> {
    let mut words = exec_start.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if word != "--token-file" {
            continue;
        }
        let path = words.peek()?.trim_matches('"');
        let candidate = path.rsplit('/').next()?.strip_suffix(".token")?;
        return validation::tunnel_id(candidate).ok();
    }
    None
}

/// The systemd unit this application writes.
///
/// Both interpolated values come from the remote host or the account, so
/// they are scrubbed: a newline in either would inject a second directive.
fn unit_file(cloudflared: &str, token_path: &str, tunnel_name: &str) -> String {
    let tunnel_name = tunnel_name.replace(['\n', '\r'], " ");
    let cloudflared = cloudflared.split_whitespace().next().unwrap_or(cloudflared);
    format!(
        "[Unit]\n\
         Description=Cloudflare Tunnel — {tunnel_name}\n\
         Documentation=https://developers.cloudflare.com/cloudflare-one/connections/connect-apps\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=notify\n\
         TimeoutStartSec=0\n\
         ExecStart={cloudflared} --no-autoupdate tunnel --loglevel info run --token-file {token_path}\n\
         Restart=on-failure\n\
         RestartSec=5s\n\
         \n\
         # The token file is root-owned and mode 0600; these keep the rest of\n\
         # the filesystem out of reach of the connector.\n\
         NoNewPrivileges=true\n\
         ProtectSystem=strict\n\
         ProtectHome=true\n\
         PrivateTmp=true\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

impl RemoteConnectorController for SshController {
    fn probe_host_key<'a>(&'a self, request: &'a RemoteRequest) -> RemoteFuture<'a, HostKey> {
        Box::pin(async move {
            let address = validation::ssh_address(&request.host.address)?;
            let seen = Arc::new(Mutex::new(None));
            let handler = PinnedHostKey {
                expected: None,
                seen: Arc::clone(&seen),
            };

            let connect = client::connect(
                Self::config(),
                (address.as_str(), request.host.port),
                handler,
            );

            let session = match tokio::time::timeout(CONNECT_TIMEOUT, connect).await {
                Err(_) => return Err(RemoteError::Timeout),
                Ok(Ok(session)) => Some(session),
                // Even a failed handshake usually gets far enough to have seen
                // the key, which is what a probe wants.
                Ok(Err(error)) => {
                    if seen.lock().ok().and_then(|seen| seen.clone()).is_none() {
                        return Err(RemoteError::Connect {
                            detail: redact(&error.to_string()),
                        });
                    }
                    None
                }
            };

            // Tear the connection down without authenticating. A probe must
            // never offer a credential to a host that has not been trusted.
            if let Some(session) = session {
                Self::disconnect(session).await;
            }

            seen.lock()
                .ok()
                .and_then(|seen| seen.clone())
                .ok_or(RemoteError::Connect {
                    detail: "the host did not present a key".into(),
                })
        })
    }

    fn preflight<'a>(&'a self, request: &'a RemoteRequest) -> RemoteFuture<'a, Preflight> {
        Box::pin(async move {
            let unit = validation::systemd_unit(&request.host.service_unit)?;
            let session = Self::connect(request).await?;

            let mut preflight = Preflight {
                reachable: true,
                host_key_matched: true,
                ..Preflight::default()
            };

            let kernel = Self::run(&session, &RemoteCommand::Kernel, None, &request.cancel).await?;
            preflight.kernel = Some(kernel.trimmed().to_string());
            preflight.is_linux = kernel.trimmed().eq_ignore_ascii_case("Linux");

            if !preflight.is_linux {
                Self::disconnect(session).await;
                return Ok(preflight);
            }

            let systemd = Self::run(
                &session,
                &RemoteCommand::SystemdVersion,
                None,
                &request.cancel,
            )
            .await?;
            preflight.systemd_available = systemd.ok();
            preflight.systemd_version = systemd
                .lines()
                .first()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty());

            let path = Self::run(
                &session,
                &RemoteCommand::CloudflaredPath,
                None,
                &request.cancel,
            )
            .await?;
            if path.ok() && !path.trimmed().is_empty() {
                preflight.cloudflared_path = Some(path.trimmed().to_string());

                let version = Self::run(
                    &session,
                    &RemoteCommand::CloudflaredVersion,
                    None,
                    &request.cancel,
                )
                .await?;
                preflight.cloudflared_version = parse_cloudflared_version(version.trimmed());
                preflight.supports_token_file = preflight
                    .cloudflared_version
                    .as_deref()
                    .map(supports_token_file)
                    .unwrap_or(false);

                if !preflight.supports_token_file {
                    preflight.notes.push(format!(
                        "cloudflared {} does not support --token-file, which is the only \
                         way this application will hand a token to a host. Deployment is \
                         disabled; upgrade to {}.{} or later.",
                        preflight
                            .cloudflared_version
                            .as_deref()
                            .unwrap_or("(unknown)"),
                        TOKEN_FILE_MIN_VERSION.0,
                        TOKEN_FILE_MIN_VERSION.1
                    ));
                }
            }

            let state = Self::run(
                &session,
                &RemoteCommand::ServiceState { unit: unit.clone() },
                None,
                &request.cancel,
            )
            .await?;
            let (service_state, enabled, present) = checked_service_state(&state)?;
            preflight.service_state = service_state;
            preflight.service_enabled = enabled;
            preflight.unit_present = present;

            if present {
                let exec = Self::run(
                    &session,
                    &RemoteCommand::UnitExecStart { unit: unit.clone() },
                    None,
                    &request.cancel,
                )
                .await?;
                preflight.configured_tunnel_id = tunnel_id_from_exec_start(exec.trimmed());
            }

            let sudo = Self::run(
                &session,
                &RemoteCommand::SudoCheck { unit },
                None,
                &request.cancel,
            )
            .await?;
            preflight.can_control_service = sudo.trimmed().ends_with("allowed");

            if let (Some(configured), Some(expected)) = (
                preflight.configured_tunnel_id.as_deref(),
                request.host.tunnel_id.as_deref(),
            ) {
                if configured != expected {
                    preflight.notes.push(format!(
                        "This host's service is running tunnel {configured}, not the \
                         tunnel it is registered against."
                    ));
                }
            }

            Self::disconnect(session).await;
            Ok(preflight)
        })
    }

    fn service_action<'a>(
        &'a self,
        request: &'a RemoteRequest,
        action: ServiceAction,
    ) -> RemoteFuture<'a, ServiceOutcome> {
        Box::pin(async move {
            let unit = validation::systemd_unit(&request.host.service_unit)?;
            let session = Self::connect(request).await?;

            let before = Self::run(
                &session,
                &RemoteCommand::ServiceState { unit: unit.clone() },
                None,
                &request.cancel,
            )
            .await?;
            let (state_before, _, _) = checked_service_state(&before)?;

            let command = RemoteCommand::Service {
                unit: unit.clone(),
                action,
            };
            let result = Self::run(&session, &command, None, &request.cancel).await?;

            if !result.ok() {
                let stderr = result.stderr.clone();
                Self::disconnect(session).await;

                // `sudo -n` failing because it wanted a password is a
                // permission problem with a specific fix, not a generic
                // command failure.
                if stderr.contains("a password is required")
                    || stderr.contains("no tty present")
                    || stderr.contains("may not run")
                {
                    return Err(RemoteError::NotPermitted {
                        detail: redact(&stderr),
                    });
                }

                return Err(RemoteError::CommandFailed {
                    description: command.description(),
                    exit_status: result.exit_status,
                    stderr,
                });
            }

            let after = Self::run(
                &session,
                &RemoteCommand::ServiceState { unit },
                None,
                &request.cancel,
            )
            .await?;
            let (state_after, _, _) = checked_service_state(&after)?;

            Self::disconnect(session).await;

            let message = result.stderr.trim();
            Ok(ServiceOutcome {
                action,
                state_before,
                state_after,
                message: (!message.is_empty()).then(|| redact(message)),
            })
        })
    }

    fn logs<'a>(&'a self, request: &'a RemoteRequest, lines: u16) -> RemoteFuture<'a, Vec<String>> {
        Box::pin(async move {
            let unit = validation::systemd_unit(&request.host.service_unit)?;
            let session = Self::connect(request).await?;

            let command = RemoteCommand::Logs {
                unit,
                lines: if lines == 0 { DEFAULT_LOG_LINES } else { lines },
            };
            let output = Self::run(&session, &command, None, &request.cancel).await?;
            Self::disconnect(session).await;

            if !output.ok() && output.trimmed().is_empty() {
                let stderr = output.stderr.clone();
                if stderr.contains("a password is required") || stderr.contains("may not run") {
                    return Err(RemoteError::NotPermitted {
                        detail: redact(&stderr),
                    });
                }
                return Err(RemoteError::CommandFailed {
                    description: command.description(),
                    exit_status: output.exit_status,
                    stderr,
                });
            }

            // The journal can hold anything the service printed, so it is
            // redacted before it is shown or logged.
            let mut lines: Vec<String> = output
                .lines()
                .into_iter()
                .map(|line| redact(&line))
                .collect();
            if output.truncated {
                lines.push("… output truncated at 512 KiB — narrow the journal instead.".into());
            }
            Ok(lines)
        })
    }

    fn run_allowlisted<'a>(
        &'a self,
        request: &'a RemoteRequest,
        command: RemoteCommand,
        stdin: Option<&'a str>,
    ) -> RemoteFuture<'a, String> {
        Box::pin(async move {
            let session = Self::connect(request).await?;

            // The content is configuration, not a credential, but it travels
            // the same way the token does — over the channel's stdin, never
            // on the command line.
            let secret_stdin = stdin.map(SecretString::from);
            let output =
                Self::run(&session, &command, secret_stdin.as_ref(), &request.cancel).await;
            Self::disconnect(session).await;

            let output = output?;
            if !output.ok() {
                let stderr = redact(&output.stderr);
                if stderr.contains("a password is required") || stderr.contains("may not run") {
                    return Err(RemoteError::NotPermitted { detail: stderr });
                }
                return Err(RemoteError::CommandFailed {
                    description: command.description(),
                    exit_status: output.exit_status,
                    stderr,
                });
            }
            Ok(redact(output.trimmed()))
        })
    }

    fn plan_deployment<'a>(
        &'a self,
        request: &'a RemoteRequest,
        tunnel_id: &'a str,
        tunnel_name: &'a str,
    ) -> RemoteFuture<'a, DeploymentPlan> {
        Box::pin(async move {
            let unit = validation::systemd_unit(&request.host.service_unit)?;
            let tunnel_id = validation::tunnel_id(tunnel_id)?;
            let session = Self::connect(request).await?;

            let path = Self::run(
                &session,
                &RemoteCommand::CloudflaredPath,
                None,
                &request.cancel,
            )
            .await?;
            let cloudflared = path.trimmed().to_string();
            // `command -v` output becomes part of `ExecStart`; it must be a
            // single absolute path, or a compromised host could inject
            // directives into the unit file.
            if cloudflared.is_empty() {
                Self::disconnect(session).await;
                return Err(RemoteError::CloudflaredMissing);
            }
            if validation::absolute_path(&cloudflared).is_err() {
                Self::disconnect(session).await;
                return Err(RemoteError::CommandFailed {
                    description: "check for cloudflared",
                    exit_status: path.exit_status,
                    stderr: "the host returned an unusable cloudflared path".into(),
                });
            }

            let version = Self::run(
                &session,
                &RemoteCommand::CloudflaredVersion,
                None,
                &request.cancel,
            )
            .await?;
            let version = parse_cloudflared_version(version.trimmed());
            if !version.as_deref().map(supports_token_file).unwrap_or(false) {
                Self::disconnect(session).await;
                return Err(RemoteError::CommandFailed {
                    description: "check for cloudflared",
                    exit_status: None,
                    stderr: format!(
                        "cloudflared {} is too old for --token-file; {}.{} or later is \
                         needed so the token is never put on a command line.",
                        version.as_deref().unwrap_or("(unknown)"),
                        TOKEN_FILE_MIN_VERSION.0,
                        TOKEN_FILE_MIN_VERSION.1
                    ),
                });
            }

            let unit_path = format!("/etc/systemd/system/{unit}");
            let token_path = format!("{TOKEN_DIR}/{tunnel_id}.token");

            let existing = Self::run(
                &session,
                &RemoteCommand::PathExists {
                    path: unit_path.clone(),
                },
                None,
                &request.cancel,
            )
            .await?;
            let replaces_existing_unit = existing.trimmed() == "yes";

            Self::disconnect(session).await;

            let unit_file = unit_file(&cloudflared, &token_path, tunnel_name);

            let steps = vec![
                DeploymentStep {
                    description: format!(
                        "Create {TOKEN_DIR}, owned by root and readable by nobody else"
                    ),
                    preview: RemoteCommand::MakeTokenDir {
                        dir: TOKEN_DIR.into(),
                    }
                    .render()?,
                    privileged: true,
                    overwrites: false,
                },
                DeploymentStep {
                    description: format!("Write the tunnel's run token to {token_path}, mode 0600"),
                    // The token travels over the channel's stdin; the preview
                    // shows the command, which does not contain it.
                    preview: format!(
                        "{}   ← the token is sent over the encrypted channel, not on this line",
                        RemoteCommand::InstallFromStdin {
                            path: token_path.clone(),
                            mode: "0600",
                        }
                        .render()?
                    ),
                    privileged: true,
                    overwrites: false,
                },
                DeploymentStep {
                    description: format!("Write the systemd unit to {unit_path}"),
                    preview: RemoteCommand::InstallFromStdin {
                        path: unit_path.clone(),
                        mode: "0644",
                    }
                    .render()?,
                    privileged: true,
                    overwrites: replaces_existing_unit,
                },
                DeploymentStep {
                    description: "Reload systemd so it reads the new unit".into(),
                    preview: RemoteCommand::DaemonReload.render()?,
                    privileged: true,
                    overwrites: false,
                },
                DeploymentStep {
                    description: format!("Enable and start {unit}"),
                    preview: RemoteCommand::EnableNow { unit: unit.clone() }.render()?,
                    privileged: true,
                    overwrites: false,
                },
            ];

            Ok(DeploymentPlan {
                tunnel_id,
                tunnel_name: tunnel_name.to_string(),
                unit,
                token_path,
                unit_path,
                cloudflared_path: cloudflared,
                steps,
                unit_file,
                replaces_existing_unit,
            })
        })
    }

    fn apply_deployment<'a>(
        &'a self,
        request: &'a RemoteRequest,
        plan: &'a DeploymentPlan,
        token: &'a SecretString,
    ) -> RemoteFuture<'a, DeploymentOutcome> {
        Box::pin(async move {
            let session = Self::connect(request).await?;

            let mut outcome = DeploymentOutcome {
                completed: Vec::new(),
                failed: None,
                service_state: RemoteServiceState::NotInstalled,
            };

            // Each step is (description, command, optional stdin). The token
            // is the only secret and it goes over stdin.
            let unit_file = SecretString::from(plan.unit_file.clone());
            let steps: Vec<(&str, RemoteCommand, Option<&SecretString>)> = vec![
                (
                    "created the token directory",
                    RemoteCommand::MakeTokenDir {
                        dir: TOKEN_DIR.into(),
                    },
                    None,
                ),
                (
                    "wrote the token file",
                    RemoteCommand::InstallFromStdin {
                        path: plan.token_path.clone(),
                        mode: "0600",
                    },
                    Some(token),
                ),
                (
                    "wrote the systemd unit",
                    RemoteCommand::InstallFromStdin {
                        path: plan.unit_path.clone(),
                        mode: "0644",
                    },
                    Some(&unit_file),
                ),
                ("reloaded systemd", RemoteCommand::DaemonReload, None),
                (
                    "enabled and started the service",
                    RemoteCommand::EnableNow {
                        unit: plan.unit.clone(),
                    },
                    None,
                ),
            ];

            for (description, command, stdin) in steps {
                match Self::run(&session, &command, stdin, &request.cancel).await {
                    Ok(output) if output.ok() => {
                        outcome.completed.push(description.to_string());
                    }
                    Ok(output) => {
                        outcome.failed =
                            Some((description.to_string(), redact(output.stderr.trim())));
                        break;
                    }
                    Err(error) => {
                        outcome.failed = Some((description.to_string(), error.user_message()));
                        break;
                    }
                }
            }

            if let Ok(state) = Self::run(
                &session,
                &RemoteCommand::ServiceState {
                    unit: plan.unit.clone(),
                },
                None,
                &request.cancel,
            )
            .await
            {
                // A truncated prefix is not a state; keep NotInstalled rather
                // than report whatever three lines survived.
                if !state.truncated {
                    outcome.service_state = parse_service_state(&state).0;
                }
            }

            Self::disconnect(session).await;
            Ok(outcome)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str) -> Output {
        Output {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_status: Some(0),
            truncated: false,
        }
    }

    #[test]
    fn a_missing_exit_status_is_not_success() {
        let missing = Output {
            stdout: String::new(),
            stderr: String::new(),
            exit_status: None,
            truncated: false,
        };
        assert!(!missing.ok());
    }

    #[test]
    fn a_truncated_prefix_is_not_success() {
        let cut = Output {
            stdout: "loaded\nactive\nenabled\n".into(),
            stderr: String::new(),
            exit_status: Some(0),
            truncated: true,
        };
        assert!(!cut.ok());
        assert!(checked_service_state(&cut).is_err());
    }

    #[test]
    fn parses_a_running_enabled_unit() {
        let (state, enabled, present) = parse_service_state(&output("loaded\nactive\nenabled\n"));
        assert_eq!(state, RemoteServiceState::Active);
        assert_eq!(enabled, Some(true));
        assert!(present);
    }

    #[test]
    fn parses_a_stopped_unit() {
        let (state, enabled, present) =
            parse_service_state(&output("loaded\ninactive\ndisabled\n"));
        assert_eq!(state, RemoteServiceState::Inactive);
        assert_eq!(enabled, Some(false));
        assert!(present);
    }

    #[test]
    fn parses_a_missing_unit() {
        let (state, _, present) = parse_service_state(&output("not-found\ninactive\n\n"));
        assert_eq!(state, RemoteServiceState::NotInstalled);
        assert!(!present, "a not-found unit is not present");
    }

    #[test]
    fn parses_a_failed_unit() {
        let (state, _, present) = parse_service_state(&output("loaded\nfailed\nenabled\n"));
        assert_eq!(state, RemoteServiceState::Failed);
        assert!(present);
    }

    #[test]
    fn an_empty_response_does_not_claim_the_unit_exists() {
        let (_, _, present) = parse_service_state(&output(""));
        assert!(!present);
    }

    #[test]
    fn reads_the_cloudflared_version_out_of_its_banner() {
        assert_eq!(
            parse_cloudflared_version("cloudflared version 2026.1.0 (built 2026-01-05)").as_deref(),
            Some("2026.1.0")
        );
        assert_eq!(
            parse_cloudflared_version("cloudflared version 2025.4.0").as_deref(),
            Some("2025.4.0")
        );
        assert_eq!(parse_cloudflared_version("nothing here"), None);
    }

    #[test]
    fn the_token_file_version_gate_holds() {
        assert!(supports_token_file("2025.4.0"));
        assert!(supports_token_file("2025.11.0"));
        assert!(supports_token_file("2026.1.0"));
        assert!(!supports_token_file("2025.3.0"));
        assert!(!supports_token_file("2024.12.1"));
        assert!(!supports_token_file("garbage"));
        assert!(!supports_token_file(""));
    }

    #[test]
    fn the_generated_unit_uses_token_file_and_never_the_token() {
        let unit = unit_file(
            "/usr/bin/cloudflared",
            "/etc/tunnel-manager/f70ff985-a4ef-4643-bbbc-4a0ed4fc8415.token",
            "production web",
        );

        assert!(unit.contains("--token-file /etc/tunnel-manager/"));
        assert!(
            !unit.contains("--token "),
            "the token must never be an argument"
        );
        assert!(
            !unit.contains("TUNNEL_TOKEN"),
            "nor an environment variable, which /proc exposes"
        );
        assert!(unit.contains("NoNewPrivileges=true"));
        assert!(unit.contains("ProtectSystem=strict"));
        assert!(unit.contains("WantedBy=multi-user.target"));
        assert!(unit.contains("production web"));
    }

    #[test]
    fn reads_the_tunnel_id_back_out_of_an_exec_start() {
        let exec = "{ path=/usr/bin/cloudflared ; argv[]=/usr/bin/cloudflared tunnel run \
                    --token-file /etc/tunnel-manager/f70ff985-a4ef-4643-bbbc-4a0ed4fc8415.token ; }";
        assert_eq!(
            tunnel_id_from_exec_start(exec).as_deref(),
            Some("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415")
        );
    }

    #[test]
    fn an_exec_start_from_cloudflares_own_installer_yields_no_tunnel_id() {
        // That unit passes the token inline, so there is nothing to read.
        let exec = "{ path=/usr/bin/cloudflared ; argv[]=/usr/bin/cloudflared tunnel run \
                    --token eyJhIjoiNWFiNGU5Zg== ; }";
        assert_eq!(tunnel_id_from_exec_start(exec), None);
    }

    #[test]
    fn output_treats_a_missing_exit_status_as_failure() {
        // A channel that closes without an ExitStatus never reported
        // success — the command may have been killed mid-run.
        let no_status = Output {
            stdout: "ok".into(),
            stderr: String::new(),
            exit_status: None,
            truncated: false,
        };
        assert!(!no_status.ok());

        let failed = Output {
            stdout: String::new(),
            stderr: "nope".into(),
            exit_status: Some(1),
            truncated: false,
        };
        assert!(!failed.ok());
    }

    #[test]
    fn the_token_directory_is_under_etc_not_a_home_directory() {
        assert!(TOKEN_DIR.starts_with("/etc/"));
        assert!(!TOKEN_DIR.contains("home"));
        assert!(!TOKEN_DIR.contains("tmp"));
    }
}
