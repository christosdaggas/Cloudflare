//! Optional control of `cloudflared` on a registered Linux host.
//!
//! Everything in this module is optional. The application is fully usable with
//! no host configured: every Cloudflare feature works, and the screens that
//! mention a remote host say "not configured" rather than being disabled.
//!
//! The security posture, in one place:
//!
//! * **Host keys are pinned.** A host cannot be operated on until its
//!   fingerprint has been shown to the user and explicitly trusted, and a key
//!   that later changes is a refusal, not a prompt. There is no configuration
//!   that turns this off.
//! * **There is no arbitrary command execution.** [`commands::RemoteCommand`]
//!   is a closed enum; the UI cannot express anything outside it.
//! * **No secret is ever an argument.** The one secret that goes to a host —
//!   a tunnel's run token — travels over the SSH channel's stdin into
//!   `install`, so it never reaches the remote process table or shell history.
//! * **Privileged changes are previewed.** A deployment produces a
//!   [`controller::DeploymentPlan`] the user reads before anything runs.
//! * **`sudo` is always `-n`.** A host that would prompt fails with an
//!   explanation instead of hanging.

pub mod commands;
pub mod controller;
pub mod error;
pub mod mock;
pub mod ssh;

pub use commands::{RemoteCommand, ServiceAction};
pub use controller::{
    AuthMaterial, DeploymentOutcome, DeploymentPlan, DeploymentStep, HostKey, Preflight,
    RemoteConnectorController, RemoteRequest, ServiceOutcome,
};
pub use error::{RemoteError, RemoteResult};
pub use mock::{MockCall, MockController, MockScript};
pub use ssh::SshController;
