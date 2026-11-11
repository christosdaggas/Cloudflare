//! The local activity log.

use tauri::State;

use crate::bridge::dto::CommandError;
use crate::bridge::state::{AppState, AppStateDto};
use crate::services::logging;

/// Empties the activity log. The event saying so is recorded afterwards, so the
/// page is never blank without explanation.
#[tauri::command(rename_all = "snake_case")]
pub async fn clear_activity(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    state.clear_activity().await;
    state
        .record(crate::domain::activity::ActivityEvent::info(
            crate::domain::activity::ActivityCategory::App,
            "Cleared the activity log.",
        ))
        .await;
    Ok(state.snapshot().await)
}

/// The recent tracing output, already redacted at the sink.
#[tauri::command(rename_all = "snake_case")]
pub fn recent_log_lines() -> Vec<String> {
    logging::recent_lines()
}

/// The level the subscriber is currently using.
#[tauri::command(rename_all = "snake_case")]
pub fn current_log_level() -> String {
    logging::current_level()
}
