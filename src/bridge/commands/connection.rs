//! Connecting to Cloudflare and the state snapshot every page reads.

use secrecy::SecretString;
use tauri::State;

use crate::bridge::dto::{CommandError, TokenCheckDto};
use crate::bridge::state::{AppState, AppStateDto};
use crate::cloudflare::Cancel;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::Account;
use crate::services::session::{self, ConnectionState, Session};
use crate::storage;

/// The whole current state. Called on mount and after any command that does
/// not already return a fresh snapshot.
#[tauri::command(rename_all = "snake_case")]
pub async fn get_state(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    Ok(state.snapshot().await)
}

/// Verifies a pasted token and discovers the accounts it can see. Does not
/// save anything.
#[tauri::command(rename_all = "snake_case")]
pub async fn check_token(
    _state: State<'_, AppState>,
    token: String,
) -> Result<TokenCheckDto, CommandError> {
    let token = crate::domain::validation::api_token(&token)?;
    let client = session::client_for(SecretString::from(token), &session::api_base_url())?;
    let cancel = Cancel::new();
    let check = session::check_token(&client, &cancel).await?;
    Ok(check.into())
}

/// Finishes the connection: verifies the token again, builds the session,
/// loads the zones, remembers the account and files the token away when the
/// user asked for that.
#[tauri::command(rename_all = "snake_case")]
pub async fn connect(
    state: State<'_, AppState>,
    token: String,
    account_id: String,
    default_zone_id: Option<String>,
    remember: bool,
) -> Result<AppStateDto, CommandError> {
    let token = crate::domain::validation::api_token(&token)?;
    state.set_connection(ConnectionState::Connecting).await;

    match connect_with_token(
        &state,
        SecretString::from(token),
        account_id,
        default_zone_id,
        remember,
    )
    .await
    {
        Ok(()) => Ok(state.snapshot().await),
        Err(error) => {
            state
                .set_connection(ConnectionState::Failed {
                    title: error.title.to_string(),
                    message: error.message.clone(),
                })
                .await;
            Err(error)
        }
    }
}

/// Lists the zones an account-scoped token can see, before a session exists.
/// Used by the onboarding page's default-zone step.
#[tauri::command(rename_all = "snake_case")]
pub async fn preview_zones(
    token: String,
    account_id: String,
) -> Result<Vec<crate::domain::Zone>, CommandError> {
    let token = crate::domain::validation::api_token(&token)?;
    let account_id = crate::domain::validation::account_id(&account_id)?;
    let client = session::client_for(SecretString::from(token), &session::api_base_url())?;
    let cancel = Cancel::new();
    Ok(client.list_zones(Some(&account_id), &cancel).await?)
}

/// The silent reconnect attempted at startup when a token was saved.
#[tauri::command(rename_all = "snake_case")]
pub async fn restore_session(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    restore_session_impl(&state).await
}

pub(crate) async fn restore_session_impl(state: &AppState) -> Result<AppStateDto, CommandError> {
    if !state.pending_restore().await {
        return Ok(state.snapshot().await);
    }

    let loaded = tokio::task::spawn_blocking(storage::secrets::load_api_token).await;
    let token = match loaded {
        Ok(Ok(Some(token))) => token,
        Ok(Ok(None)) => {
            state.set_connection(ConnectionState::Disconnected).await;
            state
                .record(ActivityEvent::warning(
                    ActivityCategory::App,
                    "No saved API token was found. Connect again with a token.",
                ))
                .await;
            return Ok(state.snapshot().await);
        }
        Ok(Err(error)) => {
            let message = error.user_message();
            state
                .set_connection(ConnectionState::Failed {
                    title: "Could not read the saved token".into(),
                    message: message.clone(),
                })
                .await;
            state
                .record(ActivityEvent::warning(ActivityCategory::App, message))
                .await;
            return Ok(state.snapshot().await);
        }
        Err(error) => {
            let message = format!("The credential task did not finish: {error}");
            state
                .set_connection(ConnectionState::Failed {
                    title: "Could not read the saved token".into(),
                    message: message.clone(),
                })
                .await;
            state
                .record(ActivityEvent::warning(ActivityCategory::App, message))
                .await;
            return Ok(state.snapshot().await);
        }
    };

    let settings = state.settings().await;
    let account_id = settings.account_id.clone().unwrap_or_default();

    if let Err(error) = connect_with_token(
        state,
        token,
        account_id,
        settings.default_zone_id.clone(),
        true,
    )
    .await
    {
        state
            .set_connection(ConnectionState::Failed {
                title: error.title.to_string(),
                message: error.message.clone(),
            })
            .await;
        state
            .record(ActivityEvent::warning(
                ActivityCategory::App,
                format!("Could not restore the session: {}", error.message),
            ))
            .await;
    }

    Ok(state.snapshot().await)
}

/// Forgets the token, account and everything derived from them.
#[tauri::command(rename_all = "snake_case")]
pub async fn disconnect(state: State<'_, AppState>) -> Result<AppStateDto, CommandError> {
    state.disconnect().await;
    Ok(state.snapshot().await)
}

/// Shared by `connect` and `restore_session`.
pub(crate) async fn connect_with_token(
    state: &AppState,
    token: SecretString,
    account_id: String,
    default_zone_id: Option<String>,
    remember: bool,
) -> Result<(), CommandError> {
    let account_id = crate::domain::validation::account_id(&account_id)?;
    let client = session::client_for(token.clone(), &session::api_base_url())?;
    let cancel = Cancel::new();

    let check = session::check_token(&client, &cancel).await?;

    let account = check
        .accounts
        .iter()
        .find(|account| account.id == account_id)
        .cloned()
        .unwrap_or_else(|| Account {
            id: account_id.clone(),
            name: account_id.clone(),
        });

    let mut live = Session::new(client, account.clone());
    let zones_problem = live.load_zones(&cancel).await;

    // Only keep a default zone that the token can still see. A zone removed
    // from the account must not resolve to a stale name.
    let keep_default = default_zone_id
        .as_ref()
        .map(|id| live.zones.iter().any(|zone| &zone.id == id))
        .unwrap_or(false);
    live.default_zone_id = if keep_default {
        default_zone_id.clone()
    } else {
        None
    };

    let default_zone_name = live.default_zone_id.as_ref().and_then(|id| {
        live.zones
            .iter()
            .find(|zone| &zone.id == id)
            .map(|zone| zone.name.clone())
    });
    let account_id = live.account.id.clone();
    let account_name = live.account.name.clone();
    let capabilities = live.capabilities;
    let token_status = check.status;

    state.set_session(Some(live)).await;
    state
        .set_connection(ConnectionState::Connected {
            account,
            token: token_status,
            capabilities,
        })
        .await;
    state
        .update_settings(|settings| {
            settings.account_id = Some(account_id.clone());
            settings.account_name = Some(account_name.clone());
            settings.default_zone_id = default_zone_id.clone();
            settings.default_zone_name = default_zone_name.clone();
            settings.token_saved = remember;
        })
        .await;

    if remember {
        state.save_token(token).await;
    } else {
        let _ = tokio::task::spawn_blocking(storage::secrets::delete_api_token).await;
        state
            .update_settings(|settings| settings.token_saved = false)
            .await;
    }

    state
        .record(
            ActivityEvent::success(
                ActivityCategory::App,
                format!("Connected to {account_name}."),
            )
            .with_target(account_name.clone()),
        )
        .await;

    if let Some(problem) = zones_problem {
        state
            .record(ActivityEvent::warning(ActivityCategory::Api, problem))
            .await;
    }

    // A tunnel-list failure is recorded by the refresh itself and does not
    // make the connection fail: a token scoped to tunnels alone is legitimate.
    let _ = super::tunnels::refresh_tunnels_impl(state).await;

    Ok(())
}
