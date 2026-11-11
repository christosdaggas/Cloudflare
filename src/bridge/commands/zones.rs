//! Zones: the domains the token can see.

use tauri::State;

use crate::bridge::dto::CommandError;
use crate::bridge::state::{AppState, AppStateDto};
use crate::cloudflare::Cancel;
use crate::domain::activity::{ActivityCategory, ActivityEvent};

/// Reloads the zone list through the session, which also refreshes the
/// capability record for Zone:Read and DNS:Edit.
#[tauri::command(rename_all = "snake_case")]
pub async fn refresh_zones(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    let mut session = state.require_session().await?;

    state.set_busy("zones", true).await;
    let cancel = Cancel::new();
    let problem = session.load_zones(&cancel).await;
    let zones = session.zones.clone();
    let default_zone_id = session.default_zone_id.clone();

    state.set_session(Some(session)).await;
    state.set_busy("zones", false).await;

    // A default zone that has been removed from the account must not keep
    // resolving to a name the account no longer has.
    if let Some(id) = &default_zone_id {
        if !zones.iter().any(|zone| &zone.id == id) {
            state
                .update_settings(|settings| {
                    settings.default_zone_id = None;
                    settings.default_zone_name = None;
                })
                .await;
        }
    }

    match problem {
        Some(problem) => {
            state
                .record(ActivityEvent::warning(ActivityCategory::Api, problem))
                .await;
        }
        None => {
            state
                .record(ActivityEvent::info(
                    ActivityCategory::Api,
                    format!("Loaded {} domain(s).", zones.len()),
                ))
                .await;
        }
    }

    Ok(state.snapshot().await)
}
