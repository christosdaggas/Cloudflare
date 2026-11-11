//! Non-secret application settings.
//!
//! A flat key/value table rather than a struct-shaped one, because a setting
//! added in a later version then needs no migration — an unknown key is simply
//! ignored and a missing key falls back to its default. Everything here is
//! safe to include in a diagnostic export.

use rusqlite::Connection;

use super::db::Result;
use serde::{Deserialize, Serialize};

/// The settings, with their defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The account the user picked. `None` means the first run has not
    /// finished.
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    /// The zone offered first in the hostname picker. Optional throughout:
    /// tunnels can be managed without ever choosing one.
    pub default_zone_id: Option<String>,
    pub default_zone_name: Option<String>,
    /// `system`, `light` or `dark`.
    pub theme: String,
    /// Seconds between automatic refreshes. Zero means manual only.
    pub refresh_interval_secs: u32,
    pub log_level: String,
    /// Default SSH user offered when adding a host.
    pub default_ssh_username: String,
    pub default_ssh_port: u16,
    pub default_service_unit: String,
    /// The systemd unit this application may control on the machine it is
    /// running on. Separate from the SSH default: "the local connector" is a
    /// fact about this computer, not a preference about remote ones.
    pub local_service_unit: String,
    /// The tunnel the local connector serves, when it has been associated
    /// with one. Drives the restart warning and the detail page's local
    /// controls.
    pub local_tunnel_id: Option<String>,
    /// Whether the user has dismissed the "set up remote control" prompt.
    pub remote_prompt_dismissed: bool,
    /// Whether the token was successfully filed in the credential store.
    pub token_saved: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            account_id: None,
            account_name: None,
            default_zone_id: None,
            default_zone_name: None,
            theme: "system".into(),
            refresh_interval_secs: 60,
            log_level: crate::services::logging::DEFAULT_LOG_LEVEL.into(),
            default_ssh_username: String::new(),
            default_ssh_port: 22,
            default_service_unit: crate::domain::host::DEFAULT_SERVICE_UNIT.into(),
            local_service_unit: crate::domain::host::DEFAULT_SERVICE_UNIT.into(),
            local_tunnel_id: None,
            remote_prompt_dismissed: false,
            token_saved: false,
        }
    }
}

/// The refresh intervals offered in Settings, in seconds. Zero is "manual".
pub const REFRESH_INTERVALS: &[(u32, &str)] = &[
    (0, "Manual only"),
    (30, "Every 30 seconds"),
    (60, "Every minute"),
    (300, "Every 5 minutes"),
    (900, "Every 15 minutes"),
];

impl Settings {
    /// Whether the first-run flow has been completed far enough to use the
    /// application. A zone is deliberately not required.
    pub fn is_configured(&self) -> bool {
        self.account_id.is_some()
    }

    /// Whether startup has everything needed to try the saved session.
    pub fn can_restore(&self) -> bool {
        self.token_saved && self.account_id.is_some() && self.account_name.is_some()
    }

    pub fn refresh_interval(&self) -> Option<std::time::Duration> {
        match self.refresh_interval_secs {
            0 => None,
            seconds => Some(std::time::Duration::from_secs(u64::from(seconds))),
        }
    }

    /// The label shown in Settings for the current interval.
    pub fn refresh_interval_label(&self) -> &'static str {
        REFRESH_INTERVALS
            .iter()
            .find(|(seconds, _)| *seconds == self.refresh_interval_secs)
            .map(|(_, label)| *label)
            .unwrap_or("Custom")
    }
}

pub fn load(connection: &Connection) -> Result<Settings> {
    let mut settings = Settings::default();

    let mut statement = connection.prepare("SELECT key, value FROM settings")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    for row in rows {
        let (key, value) = row?;
        apply(&mut settings, &key, &value);
    }

    Ok(settings)
}

/// Applies one stored key. An unrecognised key is ignored rather than being an
/// error — that is what lets a downgrade survive a newer version's settings.
fn apply(settings: &mut Settings, key: &str, value: &str) {
    let some = |value: &str| {
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    };

    match key {
        "account_id" => settings.account_id = some(value),
        "account_name" => settings.account_name = some(value),
        "default_zone_id" => settings.default_zone_id = some(value),
        "default_zone_name" => settings.default_zone_name = some(value),
        "theme" => {
            if matches!(value, "system" | "light" | "dark") {
                settings.theme = value.to_string();
            }
        }
        "refresh_interval_secs" => {
            if let Ok(seconds) = value.parse::<u32>() {
                settings.refresh_interval_secs = seconds.min(24 * 60 * 60);
            }
        }
        "log_level" => {
            if crate::services::logging::LOG_LEVELS
                .iter()
                .any(|(name, _)| *name == value)
            {
                settings.log_level = value.to_string();
            }
        }
        "default_ssh_username" => settings.default_ssh_username = value.to_string(),
        "default_ssh_port" => {
            if let Ok(port) = value.parse::<u16>() {
                if port > 0 {
                    settings.default_ssh_port = port;
                }
            }
        }
        "default_service_unit" => {
            if let Ok(unit) = crate::domain::validation::systemd_unit(value) {
                settings.default_service_unit = unit;
            }
        }
        "local_service_unit" => {
            if let Ok(unit) = crate::domain::validation::systemd_unit(value) {
                settings.local_service_unit = unit;
            }
        }
        "local_tunnel_id" => {
            if let Ok(id) = crate::domain::validation::tunnel_id(value) {
                settings.local_tunnel_id = Some(id);
            }
        }
        "remote_prompt_dismissed" => settings.remote_prompt_dismissed = value == "1",
        "token_saved" => settings.token_saved = value == "1",
        _ => {}
    }
}

pub fn save(connection: &Connection, settings: &Settings) -> Result<()> {
    let pairs: Vec<(&str, String)> = vec![
        (
            "account_id",
            settings.account_id.clone().unwrap_or_default(),
        ),
        (
            "account_name",
            settings.account_name.clone().unwrap_or_default(),
        ),
        (
            "default_zone_id",
            settings.default_zone_id.clone().unwrap_or_default(),
        ),
        (
            "default_zone_name",
            settings.default_zone_name.clone().unwrap_or_default(),
        ),
        ("theme", settings.theme.clone()),
        (
            "refresh_interval_secs",
            settings.refresh_interval_secs.to_string(),
        ),
        ("log_level", settings.log_level.clone()),
        (
            "default_ssh_username",
            settings.default_ssh_username.clone(),
        ),
        ("default_ssh_port", settings.default_ssh_port.to_string()),
        (
            "default_service_unit",
            settings.default_service_unit.clone(),
        ),
        ("local_service_unit", settings.local_service_unit.clone()),
        (
            "local_tunnel_id",
            settings.local_tunnel_id.clone().unwrap_or_default(),
        ),
        (
            "remote_prompt_dismissed",
            bool_value(settings.remote_prompt_dismissed),
        ),
        ("token_saved", bool_value(settings.token_saved)),
    ];

    let transaction = connection.unchecked_transaction()?;
    {
        let mut statement = transaction.prepare_cached(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )?;
        for (key, value) in pairs {
            statement.execute(rusqlite::params![key, value])?;
        }
    }
    transaction.commit()?;

    Ok(())
}

fn bool_value(value: bool) -> String {
    if value { "1" } else { "0" }.to_string()
}

/// Clears the account, zone and token flag — everything the "Disconnect"
/// action in Settings undoes. Theme, refresh interval and SSH defaults are
/// deliberately kept: they are preferences, not credentials.
pub fn clear_connection(connection: &Connection) -> Result<()> {
    connection.execute(
        "DELETE FROM settings WHERE key IN
         ('account_id','account_name','default_zone_id','default_zone_name','token_saved')",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db;

    fn memory() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        db::migrate(&connection).unwrap();
        connection
    }

    #[test]
    fn defaults_apply_to_an_empty_database() {
        let settings = load(&memory()).unwrap();
        assert_eq!(settings, Settings::default());
        assert!(!settings.is_configured());
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.refresh_interval_secs, 60);
    }

    #[test]
    fn settings_round_trip() {
        let connection = memory();
        let settings = Settings {
            account_id: Some("a".repeat(32)),
            account_name: Some("Acme".into()),
            default_zone_id: Some("z".repeat(32)),
            default_zone_name: Some("example.com".into()),
            theme: "dark".into(),
            refresh_interval_secs: 300,
            log_level: "debug".into(),
            default_ssh_username: "deploy".into(),
            default_ssh_port: 2222,
            default_service_unit: "cloudflared.service".into(),
            local_service_unit: "cloudflared-edge.service".into(),
            local_tunnel_id: Some("f70ff985-a4ef-4643-bbbc-4a0ed4fc8415".into()),
            remote_prompt_dismissed: true,
            token_saved: true,
        };

        save(&connection, &settings).unwrap();
        assert_eq!(load(&connection).unwrap(), settings);
    }

    #[test]
    fn saving_twice_updates_rather_than_duplicating() {
        let connection = memory();
        let mut settings = Settings::default();
        save(&connection, &settings).unwrap();
        settings.theme = "light".into();
        save(&connection, &settings).unwrap();

        let count: i64 = connection
            .query_row(
                "SELECT count(*) FROM settings WHERE key = 'theme'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(load(&connection).unwrap().theme, "light");
    }

    #[test]
    fn an_unknown_key_is_ignored_not_fatal() {
        let connection = memory();
        connection
            .execute(
                "INSERT INTO settings (key, value) VALUES ('from_the_future', 'x')",
                [],
            )
            .unwrap();
        assert_eq!(load(&connection).unwrap(), Settings::default());
    }

    #[test]
    fn a_bad_value_falls_back_to_the_default() {
        let connection = memory();
        for (key, value) in [
            ("theme", "chartreuse"),
            ("refresh_interval_secs", "not a number"),
            ("log_level", "shout"),
            ("default_ssh_port", "0"),
            ("default_service_unit", "../etc/passwd"),
            ("local_service_unit", "cloudflared.service; evil"),
            ("local_tunnel_id", "not-a-tunnel-id"),
        ] {
            connection
                .execute(
                    "INSERT INTO settings (key, value) VALUES (?1, ?2)",
                    rusqlite::params![key, value],
                )
                .unwrap();
        }

        let settings = load(&connection).unwrap();
        assert_eq!(settings.theme, "system");
        assert_eq!(settings.refresh_interval_secs, 60);
        assert_eq!(settings.log_level, "info");
        assert_eq!(settings.default_ssh_port, 22);
        assert_eq!(settings.default_service_unit, "cloudflared.service");
        assert_eq!(settings.local_service_unit, "cloudflared.service");
        assert!(settings.local_tunnel_id.is_none());
    }

    #[test]
    fn an_absurd_refresh_interval_is_clamped() {
        let connection = memory();
        connection
            .execute(
                "INSERT INTO settings (key, value) VALUES ('refresh_interval_secs', '999999999')",
                [],
            )
            .unwrap();
        assert_eq!(load(&connection).unwrap().refresh_interval_secs, 86_400);
    }

    #[test]
    fn a_zone_is_not_required_to_be_configured() {
        let settings = Settings {
            account_id: Some("a".repeat(32)),
            ..Settings::default()
        };
        assert!(settings.is_configured());
        assert!(settings.default_zone_id.is_none());
    }

    #[test]
    fn disconnecting_clears_credentials_but_keeps_preferences() {
        let connection = memory();
        save(
            &connection,
            &Settings {
                account_id: Some("a".repeat(32)),
                account_name: Some("Acme".into()),
                default_zone_id: Some("z".repeat(32)),
                theme: "dark".into(),
                refresh_interval_secs: 300,
                token_saved: true,
                ..Settings::default()
            },
        )
        .unwrap();

        clear_connection(&connection).unwrap();

        let after = load(&connection).unwrap();
        assert!(after.account_id.is_none());
        assert!(after.default_zone_id.is_none());
        assert!(!after.token_saved);
        assert_eq!(after.theme, "dark", "a preference is not a credential");
        assert_eq!(after.refresh_interval_secs, 300);
    }

    #[test]
    fn refresh_interval_maps_to_a_duration_and_a_label() {
        let manual = Settings {
            refresh_interval_secs: 0,
            ..Settings::default()
        };
        assert!(manual.refresh_interval().is_none());
        assert_eq!(manual.refresh_interval_label(), "Manual only");

        let every_minute = Settings::default();
        assert_eq!(
            every_minute.refresh_interval(),
            Some(std::time::Duration::from_secs(60))
        );
        assert_eq!(every_minute.refresh_interval_label(), "Every minute");
    }
}
