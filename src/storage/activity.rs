//! Persisting the activity log.
//!
//! Everything written here has already been through
//! [`crate::domain::activity::redact`] at construction — an [`ActivityEvent`]
//! cannot be built without it. This module redacts again on the way in anyway,
//! because a table that outlives the process is exactly the kind of place a
//! leak would sit unnoticed.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};

use crate::domain::activity::{redact, ActivityCategory, ActivityEvent, ActivitySeverity};

use super::db::Result;

/// How many events are kept. Older ones are pruned on write, so the table
/// cannot grow without bound on a long-running install.
const RETAIN: usize = 2_000;

pub fn record(connection: &Connection, event: &ActivityEvent) -> Result<i64> {
    connection.execute(
        "INSERT INTO activity (at, severity, category, summary, detail, target)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            event.at.to_rfc3339(),
            severity_name(event.severity),
            category_name(event.category),
            redact(&event.summary),
            event.detail.as_deref().map(redact),
            event.target.as_deref().map(redact),
        ],
    )?;

    let id = connection.last_insert_rowid();

    // Prune occasionally rather than on every insert: the check is a cheap
    // count, but the delete is not, and the exact ceiling does not matter.
    if id % 64 == 0 {
        prune(connection)?;
    }

    Ok(id)
}

pub fn prune(connection: &Connection) -> Result<usize> {
    let removed = connection.execute(
        "DELETE FROM activity WHERE id NOT IN
           (SELECT id FROM activity ORDER BY id DESC LIMIT ?1)",
        [RETAIN as i64],
    )?;
    Ok(removed)
}

/// The most recent events, newest first.
pub fn recent(connection: &Connection, limit: usize) -> Result<Vec<ActivityEvent>> {
    let mut statement = connection.prepare(
        "SELECT id, at, severity, category, summary, detail, target
         FROM activity ORDER BY id DESC LIMIT ?1",
    )?;

    let rows = statement.query_map([limit as i64], row_to_event)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn clear(connection: &Connection) -> Result<()> {
    connection.execute("DELETE FROM activity", [])?;
    Ok(())
}

pub fn count(connection: &Connection) -> Result<usize> {
    let count: i64 = connection.query_row("SELECT count(*) FROM activity", [], |row| row.get(0))?;
    Ok(count as usize)
}

fn severity_name(severity: ActivitySeverity) -> &'static str {
    match severity {
        ActivitySeverity::Info => "info",
        ActivitySeverity::Success => "success",
        ActivitySeverity::Warning => "warning",
        ActivitySeverity::Error => "error",
    }
}

fn severity_from(name: &str) -> ActivitySeverity {
    match name {
        "success" => ActivitySeverity::Success,
        "warning" => ActivitySeverity::Warning,
        "error" => ActivitySeverity::Error,
        _ => ActivitySeverity::Info,
    }
}

fn category_name(category: ActivityCategory) -> &'static str {
    match category {
        ActivityCategory::Api => "api",
        ActivityCategory::Dns => "dns",
        ActivityCategory::TunnelConfig => "tunnel-config",
        ActivityCategory::Tunnel => "tunnel",
        ActivityCategory::Ssh => "ssh",
        ActivityCategory::RemoteService => "remote-service",
        ActivityCategory::App => "app",
    }
}

fn category_from(name: &str) -> ActivityCategory {
    match name {
        "dns" => ActivityCategory::Dns,
        "tunnel-config" => ActivityCategory::TunnelConfig,
        "tunnel" => ActivityCategory::Tunnel,
        "ssh" => ActivityCategory::Ssh,
        "remote-service" => ActivityCategory::RemoteService,
        "app" => ActivityCategory::App,
        _ => ActivityCategory::Api,
    }
}

fn row_to_event(row: &Row<'_>) -> rusqlite::Result<ActivityEvent> {
    let at: String = row.get(1)?;
    let severity: String = row.get(2)?;
    let category: String = row.get(3)?;

    Ok(ActivityEvent {
        id: row.get::<_, i64>(0)? as u64,
        at: DateTime::parse_from_rfc3339(&at)
            .map(|time| time.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        severity: severity_from(&severity),
        category: category_from(&category),
        summary: row.get(4)?,
        detail: row.get(5)?,
        target: row.get(6)?,
    })
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
    fn an_event_round_trips_with_every_field() {
        let connection = memory();
        let event = ActivityEvent::success(ActivityCategory::Dns, "Created a CNAME")
            .with_detail("app.example.com -> tid.cfargotunnel.com")
            .with_target("app.example.com");

        record(&connection, &event).unwrap();

        let stored = &recent(&connection, 10).unwrap()[0];
        assert_eq!(stored.summary, "Created a CNAME");
        assert_eq!(stored.severity, ActivitySeverity::Success);
        assert_eq!(stored.category, ActivityCategory::Dns);
        assert_eq!(stored.target.as_deref(), Some("app.example.com"));
        assert!(stored.detail.as_deref().unwrap().contains("cfargotunnel"));
    }

    #[test]
    fn every_severity_and_category_survives_the_round_trip() {
        let connection = memory();
        for severity in ActivitySeverity::ALL {
            for category in ActivityCategory::ALL {
                record(&connection, &ActivityEvent::new(*severity, *category, "x")).unwrap();
            }
        }

        let events = recent(&connection, 1000).unwrap();
        assert_eq!(
            events.len(),
            ActivitySeverity::ALL.len() * ActivityCategory::ALL.len()
        );
        for event in events {
            // A round trip that silently collapsed a category into the default
            // would be invisible in the UI; check the mapping is total.
            assert_eq!(category_from(category_name(event.category)), event.category);
            assert_eq!(severity_from(severity_name(event.severity)), event.severity);
        }
    }

    #[test]
    fn events_come_back_newest_first() {
        let connection = memory();
        for index in 0..5 {
            record(
                &connection,
                &ActivityEvent::info(ActivityCategory::App, format!("event {index}")),
            )
            .unwrap();
        }

        let events = recent(&connection, 3).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].summary, "event 4");
        assert_eq!(events[2].summary, "event 2");
    }

    #[test]
    fn a_secret_that_slipped_through_is_redacted_on_the_way_in() {
        let connection = memory();

        // Bypasses ActivityEvent's own redaction to simulate a future call
        // site that built the struct by hand.
        let leaky = ActivityEvent {
            id: 0,
            at: Utc::now(),
            severity: ActivitySeverity::Error,
            category: ActivityCategory::Api,
            summary: "failed: Authorization: Bearer AbCd3fGh1jKlMn0pQrStUvWxYz012345678901".into(),
            detail: Some(r#"{"api_token":"AbCd3fGh1jKlMn0pQrStUvWxYz012345678901"}"#.into()),
            target: None,
        };
        record(&connection, &leaky).unwrap();

        let dump: String = connection
            .query_row(
                "SELECT summary || coalesce(detail, '') FROM activity",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!dump.contains("AbCd3fGh1jKl"), "got {dump}");
    }

    #[test]
    fn pruning_keeps_the_most_recent_events() {
        let connection = memory();
        // Insert directly to avoid 2000+ round trips through `record`.
        let transaction = connection.unchecked_transaction().unwrap();
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO activity (at, severity, category, summary)
                     VALUES ('2026-01-01T00:00:00Z', 'info', 'app', ?1)",
                )
                .unwrap();
            for index in 0..(RETAIN + 100) {
                statement.execute([format!("event {index}")]).unwrap();
            }
        }
        transaction.commit().unwrap();

        assert_eq!(count(&connection).unwrap(), RETAIN + 100);
        prune(&connection).unwrap();
        assert_eq!(count(&connection).unwrap(), RETAIN);

        let newest = &recent(&connection, 1).unwrap()[0];
        assert_eq!(newest.summary, format!("event {}", RETAIN + 99));
    }

    #[test]
    fn clearing_empties_the_table() {
        let connection = memory();
        record(
            &connection,
            &ActivityEvent::info(ActivityCategory::App, "hello"),
        )
        .unwrap();
        clear(&connection).unwrap();
        assert_eq!(count(&connection).unwrap(), 0);
    }
}
