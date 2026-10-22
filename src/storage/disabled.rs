//! Where a disabled hostname's real origin is remembered.
//!
//! Disabling a public hostname does not remove anything: the ingress rule
//! stays in the tunnel's configuration with its service rewritten to
//! `http_status:404`, so the hostname remains visible in the list and
//! Cloudflare answers 404 instead of contacting the origin. What the rule
//! served before cannot live in the rule itself — Cloudflare keeps no
//! per-rule metadata we could read back — so the service it served is kept
//! here, keyed by tunnel, hostname and path. Enabling is then exact.

use rusqlite::Connection;

use crate::storage::db::Result;

/// Remembers the service a disabled hostname's rule served.
pub fn save(
    connection: &Connection,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
    service: &str,
) -> Result<()> {
    connection
        .execute(
            "INSERT INTO disabled_hostnames (tunnel_id, hostname, path, service) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT (tunnel_id, hostname, path) DO UPDATE SET service = excluded.service",
            rusqlite::params![tunnel_id, hostname, path.unwrap_or(""), service],
        )
        .map(|_| ())
        .map_err(Into::into)
}

/// The service the rule served before it was disabled, if it was disabled
/// here.
pub fn get(
    connection: &Connection,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
) -> Result<Option<String>> {
    let mut statement = connection
        .prepare_cached("SELECT service FROM disabled_hostnames WHERE tunnel_id = ?1 AND hostname = ?2 AND path = ?3")?;
    let mut rows = statement.query(rusqlite::params![tunnel_id, hostname, path.unwrap_or("")])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Forgets a hostname's remembered origin: it is serving again, or the rule
/// is gone.
pub fn delete(
    connection: &Connection,
    tunnel_id: &str,
    hostname: &str,
    path: Option<&str>,
) -> Result<()> {
    connection
        .execute(
            "DELETE FROM disabled_hostnames WHERE tunnel_id = ?1 AND hostname = ?2 AND path = ?3",
            rusqlite::params![tunnel_id, hostname, path.unwrap_or("")],
        )
        .map(|_| ())
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn connection(dir: &TempDir) -> Connection {
        crate::storage::db::open(&dir.path().join("test.sqlite3")).expect("database opens")
    }

    #[test]
    fn save_get_delete_round_trips_per_tunnel_and_path() {
        let dir = TempDir::new().unwrap();
        let connection = connection(&dir);

        assert_eq!(get(&connection, "t1", "a.example.com", None).unwrap(), None);

        save(
            &connection,
            "t1",
            "a.example.com",
            None,
            "http://localhost:80",
        )
        .unwrap();
        save(
            &connection,
            "t1",
            "a.example.com",
            Some("/api"),
            "tcp://localhost:22",
        )
        .unwrap();
        save(
            &connection,
            "t2",
            "a.example.com",
            None,
            "https://10.0.0.5:8443",
        )
        .unwrap();

        assert_eq!(
            get(&connection, "t1", "a.example.com", None)
                .unwrap()
                .as_deref(),
            Some("http://localhost:80")
        );
        assert_eq!(
            get(&connection, "t1", "a.example.com", Some("/api"))
                .unwrap()
                .as_deref(),
            Some("tcp://localhost:22")
        );
        assert_eq!(
            get(&connection, "t1", "a.example.com", Some("/other")).unwrap(),
            None
        );

        // Re-saving the same key replaces the remembered origin.
        save(
            &connection,
            "t1",
            "a.example.com",
            None,
            "http://localhost:8080",
        )
        .unwrap();
        assert_eq!(
            get(&connection, "t1", "a.example.com", None)
                .unwrap()
                .as_deref(),
            Some("http://localhost:8080")
        );

        delete(&connection, "t1", "a.example.com", None).unwrap();
        assert_eq!(get(&connection, "t1", "a.example.com", None).unwrap(), None);
        // The other rows are untouched by a single delete.
        assert_eq!(
            get(&connection, "t1", "a.example.com", Some("/api"))
                .unwrap()
                .as_deref(),
            Some("tcp://localhost:22")
        );
    }
}
