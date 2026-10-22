//! Saved remote host profiles.
//!
//! A profile is a *reference* to how to reach a machine, never a credential.
//! The private key stays wherever the user keeps it, named by path; an
//! optional passphrase lives in the OS credential store keyed by the host's
//! row ID. Deleting a host therefore has two halves, and
//! [`crate::services::hosts`] does both.

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row};

use crate::domain::host::{RemoteAuth, RemoteHost, RemoteHostDraft};

use super::db::Result;

pub fn list(connection: &Connection) -> Result<Vec<RemoteHost>> {
    let mut statement = connection.prepare(
        "SELECT id, name, address, port, username, auth_kind, key_path, has_passphrase,
                host_key_fingerprint, service_unit, tunnel_id, notes, created_at, last_seen_at
         FROM remote_hosts
         ORDER BY name COLLATE NOCASE",
    )?;

    let rows = statement.query_map([], row_to_host)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn get(connection: &Connection, id: i64) -> Result<Option<RemoteHost>> {
    let mut statement = connection.prepare(
        "SELECT id, name, address, port, username, auth_kind, key_path, has_passphrase,
                host_key_fingerprint, service_unit, tunnel_id, notes, created_at, last_seen_at
         FROM remote_hosts WHERE id = ?1",
    )?;

    let mut rows = statement.query_map([id], row_to_host)?;
    match rows.next() {
        Some(row) => Ok(Some(row?)),
        None => Ok(None),
    }
}

/// Hosts registered as running a connector for one tunnel.
pub fn for_tunnel(connection: &Connection, tunnel_id: &str) -> Result<Vec<RemoteHost>> {
    let mut statement = connection.prepare(
        "SELECT id, name, address, port, username, auth_kind, key_path, has_passphrase,
                host_key_fingerprint, service_unit, tunnel_id, notes, created_at, last_seen_at
         FROM remote_hosts WHERE tunnel_id = ?1 ORDER BY name COLLATE NOCASE",
    )?;

    let rows = statement.query_map([tunnel_id], row_to_host)?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn insert(connection: &Connection, draft: &RemoteHostDraft) -> Result<i64> {
    let (auth_kind, key_path) = auth_columns(&draft.auth);

    connection.execute(
        "INSERT INTO remote_hosts
           (name, address, port, username, auth_kind, key_path, has_passphrase,
            host_key_fingerprint, service_unit, tunnel_id, notes, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?11)",
        rusqlite::params![
            draft.name,
            draft.address,
            draft.port,
            draft.username,
            auth_kind,
            key_path,
            i64::from(has_passphrase(&draft.auth)),
            draft.service_unit,
            draft.tunnel_id,
            draft.notes,
            Utc::now().to_rfc3339(),
        ],
    )?;

    Ok(connection.last_insert_rowid())
}

pub fn update(connection: &Connection, host: &RemoteHost) -> Result<()> {
    let (auth_kind, key_path) = auth_columns(&host.auth);

    connection.execute(
        "UPDATE remote_hosts SET
           name = ?2, address = ?3, port = ?4, username = ?5,
           auth_kind = ?6, key_path = ?7, has_passphrase = ?8,
           host_key_fingerprint = ?9, service_unit = ?10, tunnel_id = ?11,
           notes = ?12, last_seen_at = ?13
         WHERE id = ?1",
        rusqlite::params![
            host.id,
            host.name,
            host.address,
            host.port,
            host.username,
            auth_kind,
            key_path,
            i64::from(has_passphrase(&host.auth)),
            host.host_key_fingerprint,
            host.service_unit,
            host.tunnel_id,
            host.notes,
            host.last_seen_at.map(|at| at.to_rfc3339()),
        ],
    )?;

    Ok(())
}

/// Records the fingerprint the user explicitly trusted.
///
/// Separate from [`update`] on purpose: pinning a host key is a security
/// decision, and it should be obvious in a diff which code path can make one.
pub fn pin_host_key(connection: &Connection, id: i64, fingerprint: &str) -> Result<()> {
    connection.execute(
        "UPDATE remote_hosts SET host_key_fingerprint = ?2 WHERE id = ?1",
        rusqlite::params![id, fingerprint],
    )?;
    Ok(())
}

pub fn touch(connection: &Connection, id: i64) -> Result<()> {
    connection.execute(
        "UPDATE remote_hosts SET last_seen_at = ?2 WHERE id = ?1",
        rusqlite::params![id, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

pub fn delete(connection: &Connection, id: i64) -> Result<()> {
    connection.execute("DELETE FROM remote_hosts WHERE id = ?1", [id])?;
    Ok(())
}

/// Detaches every host from a tunnel that has been deleted, without deleting
/// the hosts themselves — the machines still exist and may serve another
/// tunnel tomorrow.
pub fn detach_tunnel(connection: &Connection, tunnel_id: &str) -> Result<usize> {
    let changed = connection.execute(
        "UPDATE remote_hosts SET tunnel_id = NULL WHERE tunnel_id = ?1",
        [tunnel_id],
    )?;
    Ok(changed)
}

pub fn count(connection: &Connection) -> Result<usize> {
    let count: i64 =
        connection.query_row("SELECT count(*) FROM remote_hosts", [], |row| row.get(0))?;
    Ok(count as usize)
}

fn auth_columns(auth: &RemoteAuth) -> (&'static str, Option<String>) {
    match auth {
        RemoteAuth::Agent => ("agent", None),
        RemoteAuth::KeyFile { path, .. } => ("key-file", Some(path.to_string_lossy().into_owned())),
    }
}

fn has_passphrase(auth: &RemoteAuth) -> bool {
    matches!(
        auth,
        RemoteAuth::KeyFile {
            has_passphrase: true,
            ..
        }
    )
}

fn row_to_host(row: &Row<'_>) -> rusqlite::Result<RemoteHost> {
    let auth_kind: String = row.get(5)?;
    let key_path: Option<String> = row.get(6)?;
    let has_passphrase: i64 = row.get(7)?;

    let auth = match (auth_kind.as_str(), key_path) {
        ("key-file", Some(path)) => RemoteAuth::KeyFile {
            path: std::path::PathBuf::from(path),
            has_passphrase: has_passphrase != 0,
        },
        // A `key-file` row that lost its path is broken; falling back to the
        // agent is the safe reading, and the host page shows the auth method
        // so the user can see it changed.
        _ => RemoteAuth::Agent,
    };

    Ok(RemoteHost {
        id: row.get(0)?,
        name: row.get(1)?,
        address: row.get(2)?,
        port: row.get(3)?,
        username: row.get(4)?,
        auth,
        host_key_fingerprint: row.get(8)?,
        service_unit: row.get(9)?,
        tunnel_id: row.get(10)?,
        notes: row.get(11)?,
        created_at: parse_time(row.get::<_, String>(12)?).unwrap_or_else(Utc::now),
        last_seen_at: row.get::<_, Option<String>>(13)?.and_then(parse_time),
    })
}

fn parse_time(value: String) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
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

    fn draft() -> RemoteHostDraft {
        RemoteHostDraft {
            name: "edge".into(),
            address: "edge.example.com".into(),
            port: 22,
            username: "deploy".into(),
            auth: RemoteAuth::Agent,
            service_unit: "cloudflared.service".into(),
            tunnel_id: None,
            notes: None,
        }
    }

    #[test]
    fn a_host_round_trips() {
        let connection = memory();
        let id = insert(&connection, &draft()).unwrap();

        let host = get(&connection, id).unwrap().unwrap();
        assert_eq!(host.name, "edge");
        assert_eq!(host.port, 22);
        assert!(host.auth.is_agent());
        assert!(!host.is_trusted(), "a new host has no pinned key");
        assert_eq!(count(&connection).unwrap(), 1);
    }

    #[test]
    fn a_key_file_host_stores_the_path_and_the_passphrase_flag_only() {
        let connection = memory();
        let id = insert(
            &connection,
            &RemoteHostDraft {
                auth: RemoteAuth::KeyFile {
                    path: "/home/ada/.ssh/id_ed25519".into(),
                    has_passphrase: true,
                },
                ..draft()
            },
        )
        .unwrap();

        let host = get(&connection, id).unwrap().unwrap();
        match &host.auth {
            RemoteAuth::KeyFile {
                path,
                has_passphrase,
            } => {
                assert_eq!(path.to_str().unwrap(), "/home/ada/.ssh/id_ed25519");
                assert!(has_passphrase);
            }
            other => panic!("expected a key file, got {other:?}"),
        }

        // The passphrase itself is not in the database — only that one exists.
        let dump: String = connection
            .query_row(
                "SELECT group_concat(key_path || '|' || has_passphrase) FROM remote_hosts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(dump, "/home/ada/.ssh/id_ed25519|1");
    }

    #[test]
    fn pinning_a_host_key_makes_the_host_trusted() {
        let connection = memory();
        let id = insert(&connection, &draft()).unwrap();
        assert!(!get(&connection, id).unwrap().unwrap().is_trusted());

        pin_host_key(&connection, id, "SHA256:abcdef").unwrap();
        let host = get(&connection, id).unwrap().unwrap();
        assert!(host.is_trusted());
        assert_eq!(host.host_key_fingerprint.as_deref(), Some("SHA256:abcdef"));
    }

    #[test]
    fn updating_preserves_the_pinned_key_unless_it_is_changed() {
        let connection = memory();
        let id = insert(&connection, &draft()).unwrap();
        pin_host_key(&connection, id, "SHA256:one").unwrap();

        let mut host = get(&connection, id).unwrap().unwrap();
        host.name = "renamed".into();
        update(&connection, &host).unwrap();

        let after = get(&connection, id).unwrap().unwrap();
        assert_eq!(after.name, "renamed");
        assert_eq!(after.host_key_fingerprint.as_deref(), Some("SHA256:one"));
    }

    #[test]
    fn hosts_can_be_listed_for_one_tunnel() {
        let connection = memory();
        insert(
            &connection,
            &RemoteHostDraft {
                name: "a".into(),
                tunnel_id: Some("tunnel-1".into()),
                ..draft()
            },
        )
        .unwrap();
        insert(
            &connection,
            &RemoteHostDraft {
                name: "b".into(),
                tunnel_id: Some("tunnel-2".into()),
                ..draft()
            },
        )
        .unwrap();
        insert(
            &connection,
            &RemoteHostDraft {
                name: "c".into(),
                tunnel_id: None,
                ..draft()
            },
        )
        .unwrap();

        assert_eq!(for_tunnel(&connection, "tunnel-1").unwrap().len(), 1);
        assert_eq!(list(&connection).unwrap().len(), 3);
    }

    #[test]
    fn deleting_a_tunnel_detaches_hosts_rather_than_deleting_them() {
        let connection = memory();
        insert(
            &connection,
            &RemoteHostDraft {
                tunnel_id: Some("tunnel-1".into()),
                ..draft()
            },
        )
        .unwrap();

        assert_eq!(detach_tunnel(&connection, "tunnel-1").unwrap(), 1);
        assert_eq!(count(&connection).unwrap(), 1, "the machine still exists");
        assert!(list(&connection).unwrap()[0].tunnel_id.is_none());
    }

    #[test]
    fn deleting_removes_the_row() {
        let connection = memory();
        let id = insert(&connection, &draft()).unwrap();
        delete(&connection, id).unwrap();
        assert!(get(&connection, id).unwrap().is_none());
        assert_eq!(count(&connection).unwrap(), 0);
    }

    #[test]
    fn a_key_file_row_with_no_path_degrades_to_the_agent() {
        let connection = memory();
        connection
            .execute(
                "INSERT INTO remote_hosts (name, address, username, auth_kind, key_path, created_at)
                 VALUES ('broken', 'example.test', 'root', 'key-file', NULL, '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();

        let host = &list(&connection).unwrap()[0];
        assert!(host.auth.is_agent(), "never guess at a key path");
    }

    #[test]
    fn touch_records_the_last_contact() {
        let connection = memory();
        let id = insert(&connection, &draft()).unwrap();
        assert!(get(&connection, id)
            .unwrap()
            .unwrap()
            .last_seen_at
            .is_none());

        touch(&connection, id).unwrap();
        assert!(get(&connection, id)
            .unwrap()
            .unwrap()
            .last_seen_at
            .is_some());
    }

    #[test]
    fn hosts_are_listed_case_insensitively_by_name() {
        let connection = memory();
        for name in ["zulu", "Alpha", "mike"] {
            insert(
                &connection,
                &RemoteHostDraft {
                    name: name.into(),
                    ..draft()
                },
            )
            .unwrap();
        }

        let names: Vec<String> = list(&connection)
            .unwrap()
            .into_iter()
            .map(|host| host.name)
            .collect();
        assert_eq!(names, vec!["Alpha", "mike", "zulu"]);
    }
}
