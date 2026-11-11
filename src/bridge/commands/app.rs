//! Product identity, external links and the update check.

use tauri::State;

use crate::branding;
use crate::bridge::dto::{BootstrapDto, CommandError};
use crate::remote::commands::{DEFAULT_LOG_LINES, MAX_LOG_LINES};
use crate::services::logging;
use crate::storage::settings::REFRESH_INTERVALS;
use crate::version_check::UpdateInfo;

/// Static tables and machine facts, fetched once when the window mounts.
#[tauri::command(rename_all = "snake_case")]
pub async fn bootstrap(
    _state: State<'_, crate::bridge::state::AppState>,
) -> Result<BootstrapDto, CommandError> {
    let accent = gnome_accent().await;
    let system_theme = gnome_system_theme().await;
    Ok(BootstrapDto {
        app_name: branding::APP_NAME,
        app_id: branding::APP_ID,
        version: branding::VERSION,
        tagline: branding::APP_TAGLINE,
        affiliation_notice: branding::AFFILIATION_NOTICE,
        releases: crate::release_notes::RELEASES,
        log_levels: logging::LOG_LEVELS,
        refresh_intervals: REFRESH_INTERVALS,
        required_permissions: crate::domain::account::REQUIRED_PERMISSIONS,
        max_log_lines: MAX_LOG_LINES,
        default_log_lines: DEFAULT_LOG_LINES,
        platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        accent,
        system_theme,
    })
}

/// Opens a link in the user's browser or mail client.
///
/// Only `http`, `https` and `mailto` are accepted: a `file://` or `javascript:`
/// link from a compromised document must not become a shell command.
#[tauri::command(rename_all = "snake_case")]
pub async fn open_url(url: String) -> Result<(), CommandError> {
    let parsed = url::Url::parse(&url)
        .map_err(|_| CommandError::validation("That link is not a valid URL."))?;

    if !matches!(parsed.scheme(), "http" | "https" | "mailto") {
        return Err(CommandError::validation(
            "Only http, https and mailto links can be opened from here.",
        ));
    }

    std::process::Command::new("xdg-open")
        .arg(parsed.as_str())
        .spawn()
        .map_err(|error| CommandError::internal(format!("Could not open the link: {error}")))?;

    Ok(())
}

/// Checks GitHub for a newer release. Silent on every failure.
#[tauri::command(rename_all = "snake_case")]
pub async fn check_for_updates() -> Option<UpdateInfo> {
    crate::version_check::check_for_update(branding::VERSION).await
}

/// Asks the window manager to match the application's light or dark choice.
#[tauri::command(rename_all = "snake_case")]
pub async fn set_window_theme(
    window: tauri::WebviewWindow,
    theme: String,
) -> Result<(), CommandError> {
    let theme = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None,
    };
    window
        .set_theme(theme)
        .map_err(|error| CommandError::internal(error.to_string()))
}

/// The desktop's light or dark preference.
///
/// WebKitGTK does not reliably report `prefers-color-scheme` from the GNOME
/// colour scheme on every build, so the application asks the desktop directly
/// and lets the frontend treat this as authoritative.
#[tauri::command(rename_all = "snake_case")]
pub async fn system_theme() -> String {
    gnome_system_theme().await
}

/// Reads the GNOME accent colour so the frontend can follow it.
///
/// Best-effort: a non-GNOME session, or a gsettings that is not installed,
/// simply gets the frontend's default accent.
async fn gnome_accent() -> Option<String> {
    let output = tokio::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "accent-color"])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let value = String::from_utf8_lossy(&output.stdout);
    let name = value.trim().trim_matches('\'').trim();
    let hex = match name {
        "blue" => "#3584e4",
        "teal" => "#2190a4",
        "green" => "#3a944a",
        "yellow" => "#c88800",
        "orange" => "#ed5b00",
        "red" => "#e62e3c",
        "pink" => "#d56199",
        "purple" => "#9141ac",
        "slate" => "#6f8396",
        _ => return None,
    };
    Some(hex.to_string())
}

/// Closes the application. Used by the menu and by Ctrl+Q.
#[tauri::command(rename_all = "snake_case")]
pub fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

/// `dark`, `light`, or `unknown` when no desktop preference can be read.
async fn gnome_system_theme() -> String {
    let output = tokio::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .await;

    if let Ok(output) = output {
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout);
            return match value.trim().trim_matches('\'') {
                "prefer-dark" => "dark".into(),
                "prefer-light" | "default" => "light".into(),
                _ => "unknown".into(),
            };
        }
    }

    if std::env::var("GTK_THEME")
        .map(|value| value.to_ascii_lowercase().contains("dark"))
        .unwrap_or(false)
    {
        return "dark".into();
    }

    "unknown".into()
}
