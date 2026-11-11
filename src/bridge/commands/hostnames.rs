//! Public hostnames: the account-wide sweep, publish, repair and the safe
//! disable/enable/unpublish operations.

use tauri::State;

use crate::bridge::dto::{
    CommandError, DisableHostnameRequest, HostnameSweep, PublishHostnameRequest,
};
use crate::bridge::state::AppState;
use crate::cloudflare::Cancel;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::hostname::PublicHostname;
use crate::services::hostnames::{self, Outcome, PublishRequest};

/// Reads one tunnel's ingress rules and joins them with DNS.
#[tauri::command(rename_all = "snake_case")]
pub async fn load_hostnames(
    state: State<'_, AppState>,
    tunnel_id: String,
) -> Result<Vec<PublicHostname>, CommandError> {
    let session = state.require_session().await?;
    let row = state
        .tunnel(&tunnel_id)
        .await
        .ok_or_else(|| CommandError::validation("That tunnel is not in the current list."))?;

    let cancel = Cancel::new();
    Ok(hostnames::load_hostnames(
        &session.client,
        &session.account.id,
        &row.tunnel.id,
        &row.tunnel.name,
        &session.zones,
        &cancel,
    )
    .await?)
}

/// Sweeps every tunnel on the account. Failures are collected per tunnel so a
/// single unreadable configuration does not hide the rest.
#[tauri::command(rename_all = "snake_case")]
pub async fn load_all_hostnames(state: State<'_, AppState>) -> Result<HostnameSweep, CommandError> {
    let session = state.require_session().await?;
    let tunnels = state.tunnels().await;
    let cancel = Cancel::new();

    let mut hostnames = Vec::new();
    let mut errors = Vec::new();

    for row in tunnels {
        match hostnames::load_hostnames(
            &session.client,
            &session.account.id,
            &row.tunnel.id,
            &row.tunnel.name,
            &session.zones,
            &cancel,
        )
        .await
        {
            Ok(mut list) => hostnames.append(&mut list),
            Err(error) if error.is_cancelled() => return Err(error.into()),
            Err(error) => errors.push(format!("{}: {}", row.tunnel.name, error.user_message())),
        }
    }

    hostnames.sort_by(|a, b| {
        a.hostname
            .cmp(&b.hostname)
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(HostnameSweep { hostnames, errors })
}

/// Publishes a hostname: writes the ingress rule and creates or repairs the
/// proxied CNAME.
#[tauri::command(rename_all = "snake_case")]
pub async fn publish_hostname(
    state: State<'_, AppState>,
    request: PublishHostnameRequest,
) -> Result<Outcome, CommandError> {
    let session = state.require_session().await?;
    let row = state
        .tunnel(&request.tunnel_id)
        .await
        .ok_or_else(|| CommandError::validation("That tunnel is not in the current list."))?;

    let publish = PublishRequest {
        account_id: session.account.id.clone(),
        tunnel_id: request.tunnel_id,
        tunnel_name: row.tunnel.name,
        hostname: request.hostname,
        path: request.path,
        service: request.service,
        zone_id: request.zone_id,
        origin_request: request.origin_request,
    };

    let cancel = Cancel::new();
    let outcome = hostnames::publish(&session.client, &publish, &cancel).await;
    record_outcome(&state, &outcome, ActivityCategory::Dns).await;
    note_dns(&state, &outcome).await;
    Ok(outcome)
}

/// Removes a hostname and only the DNS record that points at this tunnel.
#[tauri::command(rename_all = "snake_case")]
pub async fn unpublish_hostname(
    state: State<'_, AppState>,
    tunnel_id: String,
    hostname: String,
    path: Option<String>,
    zone_id: Option<String>,
) -> Result<Outcome, CommandError> {
    let session = state.require_session().await?;
    let cancel = Cancel::new();

    let outcome = hostnames::unpublish(
        &session.client,
        &session.account.id,
        &tunnel_id,
        &hostname,
        path.as_deref(),
        zone_id.as_deref(),
        &cancel,
    )
    .await;

    record_outcome(&state, &outcome, ActivityCategory::Dns).await;
    note_dns(&state, &outcome).await;
    state
        .forget_disabled_origin(&tunnel_id, &hostname, path.as_deref())
        .await;
    Ok(outcome)
}

/// Disables a hostname without removing it. The origin it served is
/// remembered first, so Enable can restore it exactly.
#[tauri::command(rename_all = "snake_case")]
pub async fn disable_hostname(
    state: State<'_, AppState>,
    request: DisableHostnameRequest,
) -> Result<Outcome, CommandError> {
    let session = state.require_session().await?;

    let remembered = state
        .remember_disabled_origin(
            &request.tunnel_id,
            &request.hostname,
            request.path.as_deref(),
            &request.service,
        )
        .await;
    if !remembered {
        return Err(CommandError::new(
            "storage",
            "Could not remember the origin",
            "The origin this hostname serves could not be remembered, so disabling it \
             would make Enable unable to restore it. Nothing was changed.",
        ));
    }

    let cancel = Cancel::new();
    let outcome = hostnames::disable_hostname(
        &session.client,
        &session.account.id,
        &request.tunnel_id,
        &request.hostname,
        request.path.as_deref(),
        &cancel,
    )
    .await;

    record_outcome(&state, &outcome, ActivityCategory::TunnelConfig).await;
    Ok(outcome)
}

/// Re-enables a disabled hostname by restoring the remembered origin.
#[tauri::command(rename_all = "snake_case")]
pub async fn enable_hostname(
    state: State<'_, AppState>,
    tunnel_id: String,
    hostname: String,
    path: Option<String>,
) -> Result<Outcome, CommandError> {
    let session = state.require_session().await?;
    let original = state
        .remembered_disabled_origin(&tunnel_id, &hostname, path.as_deref())
        .await;

    let cancel = Cancel::new();
    let outcome = hostnames::enable_hostname(
        &session.client,
        &session.account.id,
        &tunnel_id,
        &hostname,
        path.as_deref(),
        original,
        &cancel,
    )
    .await;

    record_outcome(&state, &outcome, ActivityCategory::TunnelConfig).await;
    if outcome.succeeded() {
        state
            .forget_disabled_origin(&tunnel_id, &hostname, path.as_deref())
            .await;
    }
    Ok(outcome)
}

/// Repairs a missing or unproxied DNS record without touching the tunnel.
#[tauri::command(rename_all = "snake_case")]
pub async fn repair_dns(
    state: State<'_, AppState>,
    zone_id: String,
    tunnel_id: String,
    hostname: String,
) -> Result<Outcome, CommandError> {
    let session = state.require_session().await?;
    let cancel = Cancel::new();
    let outcome =
        hostnames::repair_dns(&session.client, &zone_id, &tunnel_id, &hostname, &cancel).await;
    record_outcome(&state, &outcome, ActivityCategory::Dns).await;
    note_dns(&state, &outcome).await;
    Ok(outcome)
}

/// Undoes the ingress rule a failed publish added, leaving every other rule.
#[tauri::command(rename_all = "snake_case")]
pub async fn rollback_ingress(
    state: State<'_, AppState>,
    tunnel_id: String,
    hostname: String,
    path: Option<String>,
) -> Result<(), CommandError> {
    let session = state.require_session().await?;
    let cancel = Cancel::new();
    hostnames::roll_back_ingress(
        &session.client,
        &session.account.id,
        &tunnel_id,
        &hostname,
        path.as_deref(),
        &cancel,
    )
    .await?;

    state
        .record(
            ActivityEvent::info(
                ActivityCategory::TunnelConfig,
                format!("Undid the ingress change for {hostname}."),
            )
            .with_target(hostname),
        )
        .await;

    Ok(())
}

async fn record_outcome(state: &AppState, outcome: &Outcome, category: ActivityCategory) {
    let event = if outcome.succeeded() {
        ActivityEvent::success(category, outcome.summary())
    } else if outcome.is_partial() {
        ActivityEvent::warning(category, outcome.summary())
    } else {
        ActivityEvent::error(category, outcome.summary())
    };

    let event = match outcome.failed_step() {
        Some(step) => event.with_detail(step.detail.clone()),
        None => event,
    };

    state
        .record(event.with_target(outcome.hostname.clone()))
        .await;
}

/// Notes whether a DNS write was actually attempted and how it went. A rule
/// written without a zone proves nothing about DNS, so it leaves the checklist
/// row alone.
async fn note_dns(state: &AppState, outcome: &Outcome) {
    use crate::services::hostnames::{Status, Step};

    let dns_steps = outcome.steps.iter().filter(|step| {
        matches!(
            step.step,
            Step::CreateDnsRecord | Step::RepairDnsRecord | Step::RemoveDnsRecord
        )
    });

    for step in dns_steps {
        match step.status {
            Status::Done => {
                state.note_dns_access(true).await;
                return;
            }
            Status::Failed => {
                state.note_dns_access(false).await;
                return;
            }
            Status::Skipped => {}
        }
    }
}
