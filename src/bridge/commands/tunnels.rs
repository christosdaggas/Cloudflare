//! Tunnels: the list, the create wizard, rename and delete.

use tauri::State;

use crate::bridge::dto::{CommandError, CreateOutcomeDto, CreateTunnelRequest};
use crate::bridge::state::{AppState, AppStateDto};
use crate::cloudflare::Cancel;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::tunnel::Connector;
use crate::services::hostnames::StepResult;
use crate::services::tunnels::{
    self, CreateOutcome, CreateRequest, DeleteImpact, HostnameRequest, TunnelRow,
};
use crate::storage;

/// Reloads the tunnel list and starts filling in hostname counts.
#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_tunnels(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    refresh_tunnels_impl(&state).await?;
    Ok(state.snapshot().await)
}

pub(crate) async fn refresh_tunnels_impl(state: &AppState) -> Result<(), CommandError> {
    let session = state.require_session().await?;
    state.set_busy("tunnels", true).await;

    let cancel = Cancel::new();
    let result = tunnels::list(&session.client, &session.account.id, &cancel).await;

    state.set_busy("tunnels", false).await;

    match result {
        Ok(rows) => {
            state.note_tunnel_access(true).await;
            let pending: Vec<TunnelRow> = rows.clone();
            state.set_tunnels(rows).await;

            // The count lives in each tunnel's configuration, which the list
            // endpoint does not include. Load them after the list is visible
            // so a slow account does not delay the page.
            let app = state.clone();
            let client = session.client.clone();
            let account_id = session.account.id.clone();
            tokio::spawn(async move {
                let mut counts: Vec<(String, usize)> = Vec::new();
                for row in pending {
                    if let Some(count) =
                        tunnels::load_hostname_count(&client, &account_id, &row.tunnel, &cancel)
                            .await
                    {
                        counts.push((row.tunnel.id, count));
                    }
                }
                if !counts.is_empty() {
                    app.set_hostname_counts(counts).await;
                }
            });

            Ok(())
        }
        Err(error) => {
            let message = error.user_message();
            state.set_tunnels_error(message.clone()).await;
            state
                .record(ActivityEvent::warning(ActivityCategory::Api, message))
                .await;
            Err(error.into())
        }
    }
}

/// The changes the wizard is about to make, for its review step.
#[tauri::command(rename_all = "snake_case")]
pub async fn review_tunnel(
    state: State<'_, AppState>,
    request: CreateTunnelRequest,
) -> Result<Vec<String>, CommandError> {
    let account_id = state.account_id().await.unwrap_or_default();
    Ok(tunnels::review(&build_request(account_id, request)))
}

/// Runs the create wizard. Returns the individual step outcomes.
#[tauri::command(rename_all = "snake_case")]
pub async fn create_tunnel(
    state: State<'_, AppState>,
    request: CreateTunnelRequest,
) -> Result<CreateOutcomeDto, CommandError> {
    let session = state.require_session().await?;
    let name = request.name.clone();
    let request = build_request(session.account.id.clone(), request);

    let cancel = Cancel::new();
    let outcome = tunnels::create(&session.client, &request, &cancel).await;

    record_create(&state, &name, &outcome).await;
    let _ = refresh_tunnels_impl(&state).await;

    Ok(outcome.into())
}

fn build_request(account_id: String, request: CreateTunnelRequest) -> CreateRequest {
    CreateRequest {
        account_id,
        name: request.name,
        hostname: request.hostname.map(|hostname| HostnameRequest {
            hostname: hostname.hostname,
            path: hostname.path,
            service: hostname.service,
            zone_id: hostname.zone_id,
        }),
    }
}

async fn record_create(state: &AppState, name: &str, outcome: &CreateOutcome) {
    let failed: Vec<&StepResult> = outcome
        .steps
        .iter()
        .filter(|step| step.status == crate::services::hostnames::Status::Failed)
        .collect();

    let event = if failed.is_empty() {
        ActivityEvent::success(
            ActivityCategory::Tunnel,
            format!("Created the tunnel “{name}”."),
        )
    } else {
        ActivityEvent::warning(
            ActivityCategory::Tunnel,
            format!(
                "Creating the tunnel “{name}” did not finish: {}",
                failed
                    .iter()
                    .map(|step| step.detail.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        )
    };

    state.record(event.with_target(name.to_string())).await;
}

/// Renames one tunnel, leaving every other row alone.
#[tauri::command(rename_all = "snake_case")]
pub async fn rename_tunnel(
    state: State<'_, AppState>,
    tunnel_id: String,
    name: String,
) -> Result<AppStateDto, CommandError> {
    let session = state.require_session().await?;
    let cancel = Cancel::new();
    let tunnel = tunnels::rename(
        &session.client,
        &session.account.id,
        &tunnel_id,
        &name,
        &cancel,
    )
    .await?;

    if let Some(mut row) = state.tunnel(&tunnel_id).await {
        row.tunnel = tunnel.clone();
        state.update_tunnel(row).await;
    }

    state
        .record(
            ActivityEvent::success(
                ActivityCategory::Tunnel,
                format!("Renamed the tunnel to “{}”.", tunnel.name),
            )
            .with_target(tunnel.name.clone()),
        )
        .await;

    Ok(state.snapshot().await)
}

/// What deleting a tunnel would affect, gathered before the confirmation.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_tunnel_impact(
    state: State<'_, AppState>,
    tunnel_id: String,
) -> Result<DeleteImpact, CommandError> {
    let session = state.require_session().await?;
    let row = state
        .tunnel(&tunnel_id)
        .await
        .ok_or_else(|| CommandError::validation("That tunnel is not in the current list."))?;

    let cancel = Cancel::new();
    Ok(tunnels::delete_impact(
        &session.client,
        &session.account.id,
        &row.tunnel,
        &row.managed_hosts,
        &cancel,
    )
    .await)
}

/// Deletes a tunnel. DNS records are deliberately left behind; the
/// confirmation dialog has already said so.
#[tauri::command(rename_all = "snake_case")]
pub async fn delete_tunnel(
    state: State<'_, AppState>,
    tunnel_id: String,
) -> Result<AppStateDto, CommandError> {
    let session = state.require_session().await?;
    let name = state
        .tunnel(&tunnel_id)
        .await
        .map(|row| row.tunnel.name)
        .unwrap_or_else(|| tunnel_id.clone());

    let cancel = Cancel::new();
    tunnels::delete(&session.client, &session.account.id, &tunnel_id, &cancel).await?;

    let store = state.store().await;
    let detached = tunnel_id.clone();
    let _ = tokio::task::spawn_blocking(move || {
        store.with(|connection| storage::hosts::detach_tunnel(connection, &detached))
    })
    .await;

    state.remove_tunnel(&tunnel_id).await;
    state
        .record(
            ActivityEvent::success(
                ActivityCategory::Tunnel,
                format!("Deleted the tunnel “{name}”. Its DNS records were left in place."),
            )
            .with_target(name),
        )
        .await;

    Ok(state.snapshot().await)
}

/// Deregisters one connector at the edge. This does not stop `cloudflared` on
/// the machine; it will reconnect.
#[tauri::command(rename_all = "snake_case")]
pub async fn disconnect_connector(
    state: State<'_, AppState>,
    tunnel_id: String,
    client_id: String,
) -> Result<AppStateDto, CommandError> {
    let session = state.require_session().await?;
    let cancel = Cancel::new();
    tunnels::disconnect_connector(
        &session.client,
        &session.account.id,
        &tunnel_id,
        &client_id,
        &cancel,
    )
    .await?;

    state
        .record(ActivityEvent::info(
            ActivityCategory::Tunnel,
            "Deregistered a connector. The cloudflared process on that machine was not stopped.",
        ))
        .await;

    Ok(state.snapshot().await)
}

/// The connectors currently registered against a tunnel.
#[tauri::command(rename_all = "snake_case")]
pub async fn list_connectors(
    state: State<'_, AppState>,
    tunnel_id: String,
) -> Result<Vec<Connector>, CommandError> {
    let session = state.require_session().await?;
    let cancel = Cancel::new();
    Ok(tunnels::connectors(&session.client, &session.account.id, &tunnel_id, &cancel).await?)
}

/// Fetches the run token for the clipboard or a deployment. The token is not
/// stored anywhere.
#[tauri::command(rename_all = "snake_case")]
pub async fn fetch_run_token(
    state: State<'_, AppState>,
    tunnel_id: String,
) -> Result<String, CommandError> {
    use secrecy::ExposeSecret;

    let session = state.require_session().await?;
    let cancel = Cancel::new();
    let token =
        tunnels::fetch_run_token(&session.client, &session.account.id, &tunnel_id, &cancel).await?;

    state
        .record(ActivityEvent::info(
            ActivityCategory::Tunnel,
            "Fetched a tunnel's run token for the clipboard. Treat it like a password.",
        ))
        .await;

    Ok(token.expose_secret().to_string())
}
