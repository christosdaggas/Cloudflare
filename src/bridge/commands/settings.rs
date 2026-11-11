//! Preferences, the log level and the redacted diagnostic export.

use tauri::State;

use crate::bridge::dto::CommandError;
use crate::bridge::state::{AppState, AppStateDto};
use crate::domain::validation;
use crate::services::logging;
use crate::storage::Settings;

/// Replaces the editable preferences.
///
/// Credential-derived fields (`account_id`, `account_name`, `token_saved`) are
/// preserved from the live state: the Settings page displays them, but only
/// the connection flow may change them.
#[tauri::command(rename_all = "snake_case")]
pub async fn update_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<AppStateDto, CommandError> {
    let mut next = sanitize(&state, settings).await?;
    let current = state.settings().await;
    next.account_id = current.account_id;
    next.account_name = current.account_name;
    next.token_saved = current.token_saved;

    let theme = next.theme.clone();
    let interval = next.refresh_interval_secs;
    state.update_settings(|settings| *settings = next).await;

    state
        .record(crate::domain::activity::ActivityEvent::info(
            crate::domain::activity::ActivityCategory::App,
            format!("Saved settings: {theme} theme, refresh every {interval}s."),
        ))
        .await;

    Ok(state.snapshot().await)
}

/// Changes the tracing filter at runtime.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_log_level(
    state: State<'_, AppState>,
    level: String,
) -> Result<AppStateDto, CommandError> {
    if !logging::LOG_LEVELS.iter().any(|(name, _)| *name == level) {
        return Err(CommandError::validation(format!(
            "“{level}” is not one of the log levels this build offers."
        )));
    }
    if !logging::set_level(&level) {
        return Err(CommandError::internal(
            "The log filter could not be changed; the subscriber was not installed.",
        ));
    }

    let level_for_state = level.clone();
    state
        .update_settings(|settings| settings.log_level = level_for_state)
        .await;
    state
        .record(crate::domain::activity::ActivityEvent::info(
            crate::domain::activity::ActivityCategory::App,
            format!("Log level set to {level}."),
        ))
        .await;

    Ok(state.snapshot().await)
}

/// A redacted report for a bug report. No token, passphrase or run token is
/// included — the formatter only ever sees settings and log lines, and the log
/// lines were redacted at the sink.
#[tauri::command(rename_all = "snake_case")]
pub async fn diagnostic_report(state: State<'_, AppState>) -> Result<String, CommandError> {
    let snapshot = state.snapshot().await;
    let settings = &snapshot.settings;
    let connection = match &snapshot.connection {
        crate::services::session::ConnectionState::Disconnected => "disconnected",
        crate::services::session::ConnectionState::Connecting => "connecting",
        crate::services::session::ConnectionState::Connected { .. } => "connected",
        crate::services::session::ConnectionState::Failed { .. } => "failed",
    };

    let mut report = String::new();
    report.push_str(&format!(
        "Tunnel Manager {}\nPlatform: {}\nDatabase: {} (schema {})\nCredential store: {}\n\
         Connection: {}\n",
        crate::branding::VERSION,
        snapshot.settings.log_level,
        snapshot.database_path,
        snapshot.schema_version,
        if snapshot.secrets_available {
            "available"
        } else {
            "unavailable"
        },
        connection,
    ));
    if let Some(account) = &snapshot.account {
        report.push_str(&format!("Account: {} ({})\n", account.name, account.id));
    }
    report.push_str(&format!(
        "Zones: {}\nTunnels: {}\nHosts: {}\nTheme: {}\nRefresh: {}s\n\
         Default SSH: {}@:{} · unit {}\nLocal unit: {}\n\n",
        snapshot.zones.len(),
        snapshot.tunnels.len(),
        snapshot.hosts.len(),
        settings.theme,
        settings.refresh_interval_secs,
        settings.default_ssh_username,
        settings.default_ssh_port,
        settings.default_service_unit,
        settings.local_service_unit,
    ));

    report.push_str("Recent activity:\n");
    for event in snapshot.activity.iter().take(20) {
        report.push_str(&format!(
            "  [{}] {} {} — {}\n",
            event.at.format("%Y-%m-%d %H:%M:%S"),
            event.severity.label(),
            event.category.label(),
            event.summary
        ));
    }

    report.push_str("\nRecent log lines:\n");
    for line in logging::recent_lines().iter().rev().take(100).rev() {
        report.push_str("  ");
        report.push_str(line);
        report.push('\n');
    }

    Ok(report)
}

/// Validates and normalises the editable fields, so a value that reached the
/// command by hand cannot put a hostile unit name into the database.
async fn sanitize(state: &AppState, settings: Settings) -> Result<Settings, CommandError> {
    let mut next = settings;

    if !matches!(next.theme.as_str(), "system" | "light" | "dark") {
        return Err(CommandError::validation(
            "The theme must be system, light or dark.",
        ));
    }
    if !logging::LOG_LEVELS
        .iter()
        .any(|(name, _)| *name == next.log_level)
    {
        return Err(CommandError::validation(
            "That log level is not one this build offers.",
        ));
    }

    next.default_ssh_port = validation::ssh_port(&next.default_ssh_port.to_string())?;
    next.default_service_unit = validation::systemd_unit(&next.default_service_unit)?;
    next.local_service_unit = validation::systemd_unit(&next.local_service_unit)?;
    next.default_ssh_username = next.default_ssh_username.trim().to_string();

    if let Some(id) = &next.local_tunnel_id {
        next.local_tunnel_id = Some(validation::tunnel_id(id)?);
    }
    if let Some(id) = &next.default_zone_id {
        next.default_zone_id = Some(validation::account_id(id)?);
    }
    if let Some(name) = &next.default_zone_name {
        let trimmed = name.trim();
        next.default_zone_name = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
    }

    // A user-edited interval is clamped rather than refused: the number is a
    // convenience, not a security boundary.
    next.refresh_interval_secs = next.refresh_interval_secs.min(24 * 60 * 60);

    // A default zone that is not in the live list would only confuse the
    // hostname editor, so it is cleared rather than kept as a stale name.
    if let Some(id) = &next.default_zone_id {
        let zones = state.zones().await;
        if !zones.is_empty() && !zones.iter().any(|zone| &zone.id == id) {
            next.default_zone_id = None;
            next.default_zone_name = None;
        }
    }

    Ok(next)
}
