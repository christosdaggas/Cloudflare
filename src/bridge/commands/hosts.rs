//! Remote hosts, the host-key trust flow, deployments, and local control.

use secrecy::SecretString;
use tauri::State;

use crate::bridge::dto::{ApplyDeploymentRequest, CommandError};
use crate::bridge::state::{AppState, AppStateDto};
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::{
    RemoteAuth, RemoteHost, RemoteHostDraft, RemoteServiceState, RemoteStatus, TransportProtocol,
};
use crate::remote::commands::ServiceAction;
use crate::remote::{DeploymentOutcome, DeploymentPlan, HostKey, Preflight, ServiceOutcome};
use crate::services::hosts::{self, TransportOutcome};
use crate::services::local;
use crate::storage;

/// Reloads the registered hosts from the database.
#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_hosts(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    reload_hosts(&state).await?;
    Ok(state.snapshot().await)
}

/// Registers a host and returns its new ID.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_host(
    state: State<'_, AppState>,
    draft: RemoteHostDraft,
) -> Result<i64, CommandError> {
    let draft = hosts::validate_draft(&draft)?;
    let name = draft.name.clone();
    let store = state.store().await;

    let id = tokio::task::spawn_blocking(move || {
        store.with(|connection| storage::hosts::insert(connection, &draft))
    })
    .await
    .map_err(|error| CommandError::internal(error.to_string()))?
    .ok_or_else(|| CommandError::internal("The database is not available."))?
    .map_err(CommandError::from)?;

    reload_hosts(&state).await?;
    state
        .record(
            ActivityEvent::success(
                ActivityCategory::App,
                format!("Registered the host “{name}”."),
            )
            .with_target(name),
        )
        .await;

    Ok(id)
}

/// Updates an existing host profile.
#[tauri::command(rename_all = "snake_case")]
pub async fn update_host(
    state: State<'_, AppState>,
    host: RemoteHost,
) -> Result<AppStateDto, CommandError> {
    let mut host = host;
    let draft = RemoteHostDraft {
        name: host.name.clone(),
        address: host.address.clone(),
        port: host.port,
        username: host.username.clone(),
        auth: host.auth.clone(),
        service_unit: host.service_unit.clone(),
        tunnel_id: host.tunnel_id.clone(),
        notes: host.notes.clone(),
    };
    let clean = hosts::validate_draft(&draft)?;
    host.name = clean.name.clone();
    host.address = clean.address;
    host.port = clean.port;
    host.username = clean.username;
    host.auth = clean.auth;
    host.service_unit = clean.service_unit;
    host.tunnel_id = clean.tunnel_id;
    host.notes = clean.notes;

    let store = state.store().await;
    let name = host.name.clone();
    tokio::task::spawn_blocking(move || {
        store.with(|connection| storage::hosts::update(connection, &host))
    })
    .await
    .map_err(|error| CommandError::internal(error.to_string()))?
    .ok_or_else(|| CommandError::internal("The database is not available."))?
    .map_err(CommandError::from)?;

    reload_hosts(&state).await?;
    state
        .record(
            ActivityEvent::success(ActivityCategory::App, format!("Updated the host “{name}”."))
                .with_target(name),
        )
        .await;

    Ok(state.snapshot().await)
}

/// Removes a host profile and any passphrase filed for it. The machine itself
/// is not touched.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_host(
    state: State<'_, AppState>,
    host_id: i64,
) -> Result<AppStateDto, CommandError> {
    let name = state
        .host(host_id)
        .await
        .map(|host| host.name)
        .unwrap_or_else(|| format!("host {host_id}"));

    let store = state.store().await;
    tokio::task::spawn_blocking(move || {
        store.with(|connection| {
            let _ = storage::hosts::delete(connection, host_id);
        });
        let _ = storage::secrets::delete_host_passphrase(host_id);
    })
    .await
    .map_err(|error| CommandError::internal(error.to_string()))?;

    state.clear_pending_host_key(host_id).await;
    reload_hosts(&state).await?;
    state
        .record(
            ActivityEvent::info(
                ActivityCategory::App,
                format!("Removed the host profile “{name}”. Nothing on the machine was changed."),
            )
            .with_target(name),
        )
        .await;

    Ok(state.snapshot().await)
}

/// Reads a host's key without authenticating, so the user can be shown the
/// fingerprint before anything is trusted.
#[tauri::command(rename_all = "snake_case")]
pub async fn probe_host_key(
    state: State<'_, AppState>,
    host_id: i64,
) -> Result<HostKey, CommandError> {
    let host = require_host(&state, host_id).await?;
    let controller = state.controller().await;
    let key = hosts::probe_host_key(&controller, &host).await?;
    state.set_pending_host_key(host_id, key.clone()).await;
    Ok(key)
}

/// Pins the fingerprint the user was shown. There is no path to this command
/// that does not involve a human reading a hash.
#[tauri::command(rename_all = "snake_case")]
pub async fn trust_host_key(
    state: State<'_, AppState>,
    host_id: i64,
    fingerprint: String,
) -> Result<AppStateDto, CommandError> {
    if let Some(pending) = state.pending_host_key(host_id).await {
        if pending.fingerprint != fingerprint {
            return Err(CommandError::validation(
                "The fingerprint changed since it was shown. Read it from the host again.",
            ));
        }
    }

    let host = require_host(&state, host_id).await?;
    let store = state.store().await;
    let pinned = fingerprint.clone();
    tokio::task::spawn_blocking(move || {
        store.with(|connection| storage::hosts::pin_host_key(connection, host_id, &pinned))
    })
    .await
    .map_err(|error| CommandError::internal(error.to_string()))?
    .ok_or_else(|| CommandError::internal("The database is not available."))?
    .map_err(CommandError::from)?;

    state.clear_pending_host_key(host_id).await;
    reload_hosts(&state).await?;
    state
        .record(
            ActivityEvent::success(
                ActivityCategory::Ssh,
                format!("Trusted the host key for “{}”.", host.name),
            )
            .with_target(host.name),
        )
        .await;

    Ok(state.snapshot().await)
}

/// Runs the read-only checks and records the resulting status.
#[tauri::command(rename_all = "snake_case")]
pub async fn preflight_host(
    state: State<'_, AppState>,
    host_id: i64,
) -> Result<Preflight, CommandError> {
    let host = require_host(&state, host_id).await?;
    let controller = state.controller().await;
    state.set_host_status(host_id, RemoteStatus::Checking).await;

    let result = hosts::preflight(&controller, &host, passphrase_for(host_id).await).await;
    let status = hosts::status_from(&result);
    state.set_host_status(host_id, status).await;

    match result {
        Ok(preflight) => {
            let store = state.store().await;
            let _ = tokio::task::spawn_blocking(move || {
                store.with(|connection| storage::hosts::touch(connection, host_id))
            })
            .await;
            state
                .record(
                    ActivityEvent::info(
                        ActivityCategory::Ssh,
                        format!(
                            "Checked “{}”: {}.",
                            host.name,
                            preflight.service_state.label()
                        ),
                    )
                    .with_target(host.name),
                )
                .await;
            Ok(preflight)
        }
        Err(error) => {
            state
                .record(
                    ActivityEvent::warning(
                        ActivityCategory::Ssh,
                        format!("Could not check “{}”: {}", host.name, error.user_message()),
                    )
                    .with_target(host.name),
                )
                .await;
            Err(error.into())
        }
    }
}

/// Starts, stops or restarts `cloudflared` on a host.
#[tauri::command(rename_all = "snake_case")]
pub async fn host_service_action(
    state: State<'_, AppState>,
    host_id: i64,
    action: ServiceAction,
) -> Result<ServiceOutcome, CommandError> {
    let host = require_host(&state, host_id).await?;
    let controller = state.controller().await;
    let result =
        hosts::service_action(&controller, &host, passphrase_for(host_id).await, action).await;

    match result {
        Ok(outcome) => {
            apply_service_state(&state, host_id, &outcome.state_after).await;
            let event = if outcome.succeeded() {
                ActivityEvent::success(
                    ActivityCategory::RemoteService,
                    format!(
                        "{} the cloudflared service on “{}”; it is now {}.",
                        action.label(),
                        host.name,
                        outcome.state_after.label()
                    ),
                )
            } else {
                ActivityEvent::warning(
                    ActivityCategory::RemoteService,
                    format!(
                        "{} was sent to “{}”, but the service is {}.",
                        action.label(),
                        host.name,
                        outcome.state_after.label()
                    ),
                )
            };
            state.record(event.with_target(host.name)).await;
            Ok(outcome)
        }
        Err(error) => {
            state
                .record(
                    ActivityEvent::warning(
                        ActivityCategory::RemoteService,
                        format!(
                            "Could not control “{}”: {}",
                            host.name,
                            error.user_message()
                        ),
                    )
                    .with_target(host.name),
                )
                .await;
            Err(error.into())
        }
    }
}

/// A bounded tail of the unit's journal.
#[tauri::command(rename_all = "snake_case")]
pub async fn host_logs(
    state: State<'_, AppState>,
    host_id: i64,
    lines: u16,
) -> Result<Vec<String>, CommandError> {
    let host = require_host(&state, host_id).await?;
    let controller = state.controller().await;
    Ok(hosts::logs(&controller, &host, passphrase_for(host_id).await, lines).await?)
}

/// Reads the transport protocol a host's connector is pinned to.
#[tauri::command(rename_all = "snake_case")]
pub async fn read_transport(
    state: State<'_, AppState>,
    host_id: i64,
) -> Result<Option<TransportProtocol>, CommandError> {
    let host = require_host(&state, host_id).await?;
    let controller = state.controller().await;
    Ok(hosts::read_transport(&controller, &host, passphrase_for(host_id).await).await?)
}

/// Pins or clears the transport protocol. This restarts the unit.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_transport(
    state: State<'_, AppState>,
    host_id: i64,
    protocol: Option<TransportProtocol>,
) -> Result<TransportOutcome, CommandError> {
    let host = require_host(&state, host_id).await?;
    let controller = state.controller().await;
    let outcome =
        hosts::set_transport(&controller, &host, passphrase_for(host_id).await, protocol).await?;

    apply_service_state(&state, host_id, &outcome.service_state).await;
    let label = protocol
        .map(TransportProtocol::label)
        .unwrap_or("cloudflared's default");
    state
        .record(
            ActivityEvent::success(
                ActivityCategory::RemoteService,
                format!(
                    "Set the transport for “{}” to {label} and restarted the service.",
                    host.name
                ),
            )
            .with_target(host.name),
        )
        .await;

    Ok(outcome)
}

/// Builds a deployment plan for the user to read.
#[tauri::command(rename_all = "snake_case")]
pub async fn plan_deployment(
    state: State<'_, AppState>,
    host_id: i64,
    tunnel_id: String,
) -> Result<DeploymentPlan, CommandError> {
    let host = require_host(&state, host_id).await?;
    let tunnel_name = state
        .tunnel(&tunnel_id)
        .await
        .map(|row| row.tunnel.name)
        .unwrap_or_else(|| tunnel_id.clone());
    let controller = state.controller().await;

    Ok(hosts::plan_deployment(
        &controller,
        &host,
        passphrase_for(host_id).await,
        &tunnel_id,
        &tunnel_name,
    )
    .await?)
}

/// Applies a plan the user has seen and approved.
#[tauri::command(rename_all = "snake_case")]
pub async fn apply_deployment(
    state: State<'_, AppState>,
    request: ApplyDeploymentRequest,
) -> Result<DeploymentOutcome, CommandError> {
    let host = require_host(&state, request.host_id).await?;
    let controller = state.controller().await;
    let token = SecretString::from(request.token);

    let outcome = hosts::apply_deployment(
        &controller,
        &host,
        passphrase_for(request.host_id).await,
        &request.plan,
        &token,
        request.confirmed_overwrite,
    )
    .await?;

    apply_service_state(&state, request.host_id, &outcome.service_state).await;

    let event = match &outcome.failed {
        None => ActivityEvent::success(
            ActivityCategory::RemoteService,
            format!("Deployed a connector to “{}”.", host.name),
        ),
        Some((step, why)) if outcome.is_partial() => ActivityEvent::warning(
            ActivityCategory::RemoteService,
            format!(
                "Deployment to “{}” stopped at “{step}”: {why}. Some steps completed.",
                host.name
            ),
        ),
        Some((step, why)) => ActivityEvent::error(
            ActivityCategory::RemoteService,
            format!("Deployment to “{}” failed at “{step}”: {why}.", host.name),
        ),
    };
    state.record(event.with_target(host.name)).await;

    Ok(outcome)
}

/// The instructions to show when SSH deployment is skipped.
#[tauri::command(rename_all = "snake_case")]
pub fn manual_setup_instructions(tunnel_id: String) -> String {
    hosts::manual_setup_instructions(&tunnel_id)
}

/// The least-privilege sudoers rule the Remote Hosts page offers to copy.
#[tauri::command(rename_all = "snake_case")]
pub fn sudoers_snippet(username: String, unit: String) -> Result<String, CommandError> {
    Ok(hosts::sudoers_snippet(&username, &unit)?)
}

/// Stores a passphrase for an encrypted key in the OS credential store.
#[tauri::command(rename_all = "snake_case")]
pub async fn save_host_passphrase(
    state: State<'_, AppState>,
    host_id: i64,
    passphrase: String,
) -> Result<AppStateDto, CommandError> {
    if passphrase.is_empty() {
        return delete_host_passphrase(state, host_id).await;
    }

    let secret = SecretString::from(passphrase);
    tokio::task::spawn_blocking(move || storage::secrets::save_host_passphrase(host_id, &secret))
        .await
        .map_err(|error| CommandError::internal(error.to_string()))?
        .map_err(CommandError::from)?;

    set_has_passphrase(&state, host_id, true).await?;
    Ok(state.snapshot().await)
}

/// Removes a stored passphrase.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_host_passphrase(
    state: State<'_, AppState>,
    host_id: i64,
) -> Result<AppStateDto, CommandError> {
    tokio::task::spawn_blocking(move || storage::secrets::delete_host_passphrase(host_id))
        .await
        .map_err(|error| CommandError::internal(error.to_string()))?
        .map_err(CommandError::from)?;

    set_has_passphrase(&state, host_id, false).await?;
    Ok(state.snapshot().await)
}

// ── Local machine ────────────────────────────────────────────────────────

/// Reads the local cloudflared service's state.
#[tauri::command(rename_all = "snake_case")]
pub async fn local_inspect(unit: String) -> Result<local::LocalState, CommandError> {
    Ok(local::inspect(&unit).await?)
}

/// Starts, stops or restarts the local unit.
#[tauri::command(rename_all = "snake_case")]
pub async fn local_service_action(
    unit: String,
    action: ServiceAction,
) -> Result<ServiceOutcome, CommandError> {
    Ok(local::service_action(&unit, action).await?)
}

/// The local unit's recent journal lines.
#[tauri::command(rename_all = "snake_case")]
pub async fn local_logs(unit: String, lines: u16) -> Result<Vec<String>, CommandError> {
    Ok(local::logs(&unit, lines).await?)
}

/// Pins or clears the local connector's transport protocol.
#[tauri::command(rename_all = "snake_case")]
pub async fn local_set_transport(
    unit: String,
    protocol: Option<TransportProtocol>,
) -> Result<TransportOutcome, CommandError> {
    Ok(local::set_transport(&unit, protocol).await?)
}

// ── Helpers ──────────────────────────────────────────────────────────────

async fn require_host(state: &AppState, host_id: i64) -> Result<RemoteHost, CommandError> {
    state
        .host(host_id)
        .await
        .ok_or_else(|| CommandError::validation("That host is no longer registered."))
}

async fn passphrase_for(host_id: i64) -> Option<SecretString> {
    tokio::task::spawn_blocking(move || {
        storage::secrets::load_host_passphrase(host_id)
            .ok()
            .flatten()
    })
    .await
    .ok()
    .flatten()
}

async fn reload_hosts(state: &AppState) -> Result<(), CommandError> {
    let store = state.store().await;
    let hosts = tokio::task::spawn_blocking(move || store.with(storage::hosts::list))
        .await
        .map_err(|error| CommandError::internal(error.to_string()))?
        .ok_or_else(|| CommandError::internal("The database is not available."))?
        .map_err(CommandError::from)?;

    state.set_hosts(hosts).await;
    Ok(())
}

/// Replaces the reachability of a host while keeping the facts a service
/// action does not change.
async fn apply_service_state(state: &AppState, host_id: i64, service: &RemoteServiceState) {
    if let RemoteStatus::Reachable {
        cloudflared_version,
        systemd_available,
        can_control_service,
        ..
    } = state.host_status(host_id).await
    {
        state
            .set_host_status(
                host_id,
                RemoteStatus::Reachable {
                    service: service.clone(),
                    cloudflared_version,
                    systemd_available,
                    can_control_service,
                },
            )
            .await;
    }
}

async fn set_has_passphrase(
    state: &AppState,
    host_id: i64,
    has_passphrase: bool,
) -> Result<(), CommandError> {
    let Some(mut host) = state.host(host_id).await else {
        return Ok(());
    };
    let RemoteAuth::KeyFile {
        has_passphrase: current,
        ..
    } = &mut host.auth
    else {
        return Ok(());
    };
    if *current == has_passphrase {
        return Ok(());
    }
    *current = has_passphrase;

    let store = state.store().await;
    tokio::task::spawn_blocking(move || {
        store.with(|connection| storage::hosts::update(connection, &host))
    })
    .await
    .map_err(|error| CommandError::internal(error.to_string()))?
    .ok_or_else(|| CommandError::internal("The database is not available."))?
    .map_err(CommandError::from)?;

    reload_hosts(state).await
}
