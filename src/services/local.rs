//! Controlling cloudflared on the machine this application runs on.
//!
//! The remote layer only speaks SSH: every connector command goes through a
//! registered host. But the most common connector of all — the one on the
//! user's own computer — should not need a host profile, a key trust or a
//! sudoers rule to be restarted. This module runs the same systemd commands
//! as the remote allowlist, in-process, with no shell:
//!
//! * **Reads** (`systemctl show`, `journalctl`, `cloudflared --version`) run
//!   as the user, exactly as typed on a terminal.
//! * **System unit changes** (`systemctl restart`, `daemon-reload`) run as the
//!   user and let systemd's own policy decide: in an active local session
//!   that is an authentication dialog from the desktop, which is the right
//!   UX for a destructive action on the user's own machine.
//! * **Writes under `/etc`** (the protocol drop-in) cannot go through that
//!   policy — a file write is not a D-Bus call — so they run through `pkexec`,
//!   which shows the same kind of dialog. The drop-in content is written to a
//!   temporary file first, so `pkexec` copies a finished file rather than
//!   reading a pipe: nothing half-written can end up in `systemd/system`.
//!
//! Everything executes with an argument vector, never a command line, so
//! there is no shell to inject into even though the values are validated
//! anyway.

use std::process::Command;

use crate::domain::activity::redact;
use crate::domain::host::{RemoteServiceState, TransportProtocol};
use crate::domain::validation;
use crate::remote::commands::ServiceAction;
use crate::remote::controller::ServiceOutcome;
use crate::remote::{RemoteError, RemoteResult};
use crate::services::hosts::TransportOutcome;
use serde::Serialize;

/// What the checks found on this machine.
#[derive(Debug, Clone, Serialize)]
pub struct LocalState {
    pub systemctl_available: bool,
    pub systemd_available: bool,
    pub cloudflared_version: Option<String>,
    pub unit_present: bool,
    pub service_state: RemoteServiceState,
    pub service_enabled: Option<bool>,
    pub protocol: Option<TransportProtocol>,
}

impl LocalState {
    /// Whether Start / Stop / Restart can be offered.
    pub fn can_control(&self) -> bool {
        self.systemctl_available && self.systemd_available && self.unit_present
    }

    /// Whether the unit can be inspected at all, even if it cannot be
    /// controlled. A present cloudflared with a missing unit is a machine
    /// that wants a deployment, not an error.
    pub fn can_inspect(&self) -> bool {
        self.systemctl_available && self.systemd_available
    }

    /// The single most useful thing to tell the user, or `None` when
    /// everything is in order.
    pub fn blocking_reason(&self) -> Option<String> {
        if !self.systemctl_available {
            return Some(
                "systemctl was not found. Local control needs a Linux system with \
                 systemd — this build is not running on one."
                    .into(),
            );
        }
        if !self.systemd_available {
            return Some(
                "This machine is not running systemd, so the cloudflared service \
                 cannot be controlled from here."
                    .into(),
            );
        }
        if self.cloudflared_version.is_none() {
            return Some(
                "cloudflared is not installed on this machine. Install it from \
                 https://pkg.cloudflare.com first."
                    .into(),
            );
        }
        if !self.unit_present {
            return Some(
                "The service unit does not exist on this machine yet. Install the \
                 connector first — the service unit name below is what it will be \
                 checked against."
                    .into(),
            );
        }
        None
    }

    /// Why a control button is disabled when `can_control` is false but the
    /// page is not an error. Written for a tooltip.
    pub fn control_hint(&self) -> &'static str {
        if !self.systemctl_available || !self.systemd_available {
            "Not available: this machine does not run systemd."
        } else if self.cloudflared_version.is_none() {
            "Not available until cloudflared is installed on this machine."
        } else if !self.unit_present {
            "Not available until the service unit exists on this machine."
        } else {
            ""
        }
    }
}

struct RunOutput {
    stdout: String,
    stderr: String,
    code: Option<i32>,
}

/// Runs a program with an argument vector and captures it.
///
/// Every path through this function ends in a [`RemoteError`] rather than a
/// panic: a missing binary, a killed process and a full pipe are all ordinary
/// outcomes on a user's machine.
fn run(argv: &[&str], description: &'static str) -> RemoteResult<RunOutput> {
    let (program, args) = argv.split_first().ok_or(RemoteError::CommandFailed {
        description,
        exit_status: None,
        stderr: "no command was built".into(),
    })?;

    let output =
        Command::new(program)
            .args(args)
            .output()
            .map_err(|error| RemoteError::CommandFailed {
                description,
                exit_status: None,
                stderr: format!("could not start {program}: {error}"),
            })?;

    Ok(RunOutput {
        stdout: redact(&String::from_utf8_lossy(&output.stdout)),
        stderr: redact(&String::from_utf8_lossy(&output.stderr)),
        code: output.status.code(),
    })
}

fn succeeded(output: &RunOutput) -> bool {
    output.code == Some(0)
}

/// Whether systemd refused for lack of privilege. Written for the two
/// mechanisms this module uses: systemd's own "interactive authentication"
/// refusal, and `pkexec`'s dismissal or missing agent.
fn is_privilege_refusal(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("interactive authentication required")
        || lower.contains("access denied")
        || lower.contains("not authorized")
        || lower.contains("a password is required")
        || lower.contains("may not run")
        || lower.contains("no authentication agent")
        || lower.contains("dismissed")
        || lower.contains("cancelled")
}

fn permission_error(stderr: String) -> RemoteError {
    RemoteError::NotPermitted { detail: stderr }
}

/// Parses `systemctl show --value` output: three lines, in the order asked.
fn parse_service_state(output: &str) -> (RemoteServiceState, Option<bool>, bool) {
    let mut lines = output.lines();
    let load = lines.next().unwrap_or_default();
    let active = lines.next().unwrap_or_default();
    let enabled = lines.next().unwrap_or_default();

    let state = RemoteServiceState::from_systemctl(active, load);
    let is_enabled = match enabled.trim() {
        "enabled" | "enabled-runtime" | "static" | "indirect" => Some(true),
        "disabled" | "masked" => Some(false),
        _ => None,
    };
    let present = load.trim() != "not-found" && !load.trim().is_empty();

    (state, is_enabled, present)
}

/// Reads `2026.10.0` out of whatever `cloudflared --version` printed.
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
        .map(str::to_string)
}

/// Reads this machine's answers to the same questions preflight asks remotely.
pub async fn inspect(unit: &str) -> RemoteResult<LocalState> {
    let unit = validation::systemd_unit(unit)?;
    crate::services::runtime::blocking(move || inspect_blocking(&unit))
        .await
        .unwrap_or(Err(RemoteError::Timeout))
}

fn inspect_blocking(unit: &str) -> RemoteResult<LocalState> {
    let mut state = LocalState {
        systemctl_available: true,
        systemd_available: false,
        cloudflared_version: None,
        unit_present: false,
        service_state: RemoteServiceState::NotInstalled,
        service_enabled: None,
        protocol: None,
    };

    let version = run(&["systemctl", "--version"], "check for systemd")?;
    state.systemd_available = version.code == Some(0) && !version.stdout.trim().is_empty();
    if !state.systemd_available {
        return Ok(state);
    }

    if let Ok(found) = run(
        &[
            "systemctl",
            "show",
            unit,
            "--property=LoadState",
            "--property=ActiveState",
            "--property=UnitFileState",
            "--value",
            "--no-pager",
        ],
        "read the service state",
    ) {
        let (service_state, enabled, present) = parse_service_state(&found.stdout);
        state.service_state = service_state;
        state.service_enabled = enabled;
        state.unit_present = present;
    }

    if let Ok(version) = run(&["cloudflared", "--version"], "check for cloudflared") {
        state.cloudflared_version = parse_cloudflared_version(&version.stdout);
    }

    if let Ok(environment) = run(
        &[
            "systemctl",
            "show",
            unit,
            "--property=Environment",
            "--value",
            "--no-pager",
        ],
        "read the unit's environment",
    ) {
        state.protocol = TransportProtocol::from_env_output(&environment.stdout);
    }

    Ok(state)
}

/// Starts, stops or restarts the unit on this machine.
///
/// The command runs as the user; when systemd wants authorisation the
/// desktop's authentication dialog appears. A refusal or a dismissal is a
/// [`RemoteError::NotPermitted`], not a crash.
pub async fn service_action(unit: &str, action: ServiceAction) -> RemoteResult<ServiceOutcome> {
    let unit = validation::systemd_unit(unit)?;
    crate::services::runtime::blocking(move || service_action_blocking(&unit, action))
        .await
        .unwrap_or(Err(RemoteError::Timeout))
}

fn service_action_blocking(unit: &str, action: ServiceAction) -> RemoteResult<ServiceOutcome> {
    let before = read_state_after(unit)?;

    let output = run(&["systemctl", action.verb(), unit], "control the service")?;
    if !succeeded(&output) {
        if is_privilege_refusal(&output.stderr) {
            return Err(permission_error(output.stderr));
        }
        return Err(RemoteError::CommandFailed {
            description: "control the service",
            exit_status: output.code.map(|code| code as u32),
            stderr: output.stderr,
        });
    }

    let after = read_state_after(unit)?;
    Ok(ServiceOutcome {
        action,
        state_before: before,
        state_after: after,
        message: if output.stderr.trim().is_empty() {
            None
        } else {
            Some(output.stderr.trim().to_string())
        },
    })
}

fn read_state_after(unit: &str) -> RemoteResult<RemoteServiceState> {
    let found = run(
        &[
            "systemctl",
            "show",
            unit,
            "--property=LoadState",
            "--property=ActiveState",
            "--property=UnitFileState",
            "--value",
            "--no-pager",
        ],
        "read the service state",
    )?;
    Ok(parse_service_state(&found.stdout).0)
}

/// A bounded tail of the unit's journal.
pub async fn logs(unit: &str, lines: u16) -> RemoteResult<Vec<String>> {
    let unit = validation::systemd_unit(unit)?;
    let count = lines
        .min(crate::remote::commands::MAX_LOG_LINES)
        .to_string();
    crate::services::runtime::blocking(move || {
        let output = run(
            &[
                "journalctl",
                "-u",
                &unit,
                "-n",
                &count,
                "--no-pager",
                "-o",
                "short-iso",
            ],
            "read recent logs",
        )?;
        if !succeeded(&output) {
            if is_privilege_refusal(&output.stderr) {
                return Err(permission_error(output.stderr));
            }
            return Err(RemoteError::CommandFailed {
                description: "read recent logs",
                exit_status: output.code.map(|code| code as u32),
                stderr: output.stderr,
            });
        }
        let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
        Ok(if lines.is_empty() {
            vec!["The journal returned nothing for this unit.".into()]
        } else {
            lines
        })
    })
    .await
    .unwrap_or(Err(RemoteError::Timeout))
}

/// Pins or clears the connector's transport protocol on this machine.
///
/// The drop-in is written to a temporary file and copied into place with
/// `pkexec install`, then systemd is reloaded and the unit restarted — the
/// restart is what makes the change take effect, and it interrupts traffic
/// in exactly the way every restart does, so the caller confirms first.
pub async fn set_transport(
    unit: &str,
    protocol: Option<TransportProtocol>,
) -> RemoteResult<TransportOutcome> {
    let unit = validation::systemd_unit(unit)?;
    crate::services::runtime::blocking(move || set_transport_blocking(&unit, protocol))
        .await
        .unwrap_or(Err(RemoteError::Timeout))
}

fn set_transport_blocking(
    unit: &str,
    protocol: Option<TransportProtocol>,
) -> RemoteResult<TransportOutcome> {
    let dir = TransportProtocol::drop_in_dir(unit).map_err(|_| RemoteError::NotConfigured)?;
    let path = TransportProtocol::drop_in_path(unit).map_err(|_| RemoteError::NotConfigured)?;

    match protocol {
        Some(choice) => {
            let content = choice.drop_in_content();
            let temporary = std::env::temp_dir().join(format!(
                "tunnel-manager-protocol-{}-{}.conf",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_nanos())
                    .unwrap_or(0),
            ));
            std::fs::write(&temporary, content).map_err(|error| RemoteError::CommandFailed {
                description: "write the protocol override",
                exit_status: None,
                stderr: format!("could not stage the drop-in file: {error}"),
            })?;
            let staged = temporary.to_string_lossy().to_string();

            let result = (|| {
                privileged(
                    &[
                        "/usr/bin/install",
                        "-d",
                        "-m",
                        "0755",
                        "-o",
                        "root",
                        "-g",
                        "root",
                        &dir,
                    ],
                    "create the drop-in directory",
                )?;
                privileged(
                    &[
                        "/usr/bin/install",
                        "-m",
                        "0644",
                        "-o",
                        "root",
                        "-g",
                        "root",
                        &staged,
                        &path,
                    ],
                    "write the protocol override",
                )
            })();
            // The staged file has done its job either way; never leave it.
            let _ = std::fs::remove_file(&temporary);
            result?;
        }
        None => {
            privileged(
                &["/usr/bin/rm", "-f", &path],
                "remove the protocol override",
            )?;
        }
    }

    let reload = run(&["systemctl", "daemon-reload"], "reload systemd")?;
    if !succeeded(&reload) {
        if is_privilege_refusal(&reload.stderr) {
            return Err(permission_error(reload.stderr));
        }
        return Err(RemoteError::CommandFailed {
            description: "reload systemd",
            exit_status: reload.code.map(|code| code as u32),
            stderr: reload.stderr,
        });
    }

    let outcome = service_action_blocking(unit, ServiceAction::Restart)?;
    Ok(TransportOutcome {
        protocol,
        service_state: outcome.state_after,
    })
}

/// Runs a file-writing command as root through `pkexec`, which shows the
/// desktop's authentication dialog. A dismissal or a missing agent is a
/// permission error, never a hang: `pkexec` fails fast either way.
fn privileged(argv: &[&str], description: &'static str) -> RemoteResult<()> {
    let mut full = vec!["pkexec"];
    full.extend_from_slice(argv);
    let output = run(&full, description)?;
    if succeeded(&output) {
        return Ok(());
    }
    if is_privilege_refusal(&output.stderr) {
        return Err(permission_error(output.stderr));
    }
    Err(RemoteError::CommandFailed {
        description,
        exit_status: output.code.map(|code| code as u32),
        stderr: output.stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_service_lines_parse_in_order() {
        let (state, enabled, present) = parse_service_state("loaded\nactive\nenabled\n");
        assert_eq!(state, RemoteServiceState::Active);
        assert_eq!(enabled, Some(true));
        assert!(present);
    }

    #[test]
    fn a_missing_unit_parses_as_not_installed() {
        let (state, _, present) = parse_service_state("not-found\ninactive\ndisabled\n");
        assert_eq!(state, RemoteServiceState::NotInstalled);
        assert!(!present);
    }

    #[test]
    fn a_failed_unit_parses_as_failed() {
        let (state, _, present) = parse_service_state("loaded\nfailed\nenabled\n");
        assert_eq!(state, RemoteServiceState::Failed);
        assert!(present);
    }

    #[test]
    fn short_output_is_still_a_state_not_a_panic() {
        let (state, enabled, present) = parse_service_state("");
        assert_eq!(state, RemoteServiceState::NotInstalled);
        assert_eq!(enabled, None);
        assert!(!present);
    }

    #[test]
    fn the_version_is_found_wherever_it_sits_in_the_output() {
        assert_eq!(
            parse_cloudflared_version("cloudflared version 2026.10.0 (go1.24)"),
            Some("2026.10.0".into())
        );
        assert_eq!(parse_cloudflared_version("no version here"), None);
    }

    #[test]
    fn privilege_refusals_are_recognised_in_both_flavours() {
        assert!(is_privilege_refusal("Interactive authentication required."));
        assert!(is_privilege_refusal("Error: Access denied by policy."));
        assert!(is_privilege_refusal(
            "pkexec must be setuid root — no wait, it says: No authentication agent found."
        ));
        assert!(is_privilege_refusal("The request was dismissed."));
        assert!(!is_privilege_refusal("Failed to restart: Unit not found."));
    }

    #[test]
    fn a_fully_ready_machine_has_no_blocking_reason() {
        let ready = LocalState {
            systemctl_available: true,
            systemd_available: true,
            cloudflared_version: Some("2026.10.0".into()),
            unit_present: true,
            service_state: RemoteServiceState::Active,
            service_enabled: Some(true),
            protocol: Some(TransportProtocol::Http2),
        };
        assert!(ready.can_control());
        assert!(ready.can_inspect());
        assert!(ready.blocking_reason().is_none());
    }

    #[test]
    fn a_machine_without_systemd_explains_itself() {
        let bare = LocalState {
            systemctl_available: false,
            systemd_available: false,
            cloudflared_version: None,
            unit_present: false,
            service_state: RemoteServiceState::NotInstalled,
            service_enabled: None,
            protocol: None,
        };
        assert!(!bare.can_control());
        assert!(!bare.can_inspect());
        assert!(bare
            .blocking_reason()
            .unwrap()
            .contains("systemctl was not found"));
        assert!(!bare.control_hint().is_empty());
    }
}
