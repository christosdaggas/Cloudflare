//! The SQLite database: schema, migrations, and where it lives.
//!
//! **No secret is ever written here.** The schema is designed so that it
//! cannot be: there is no column that could hold a token, a password or a
//! passphrase, only a boolean recording that a passphrase exists in the
//! credential store. A copy of this file is safe to attach to a bug report,
//! which is exactly what the diagnostic export does.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::branding::DATA_DIR_NAME;

/// Bumped whenever [`migrate`] gains a step.
pub const SCHEMA_VERSION: i64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("could not open the database: {0}")]
    Open(String),
    #[error("database error: {0}")]
    Query(String),
    #[error("could not create {path}: {detail}")]
    Directory { path: String, detail: String },
    #[error("this database was written by a newer version of Tunnel Manager (schema {found}, this build understands {supported})")]
    FromTheFuture { found: i64, supported: i64 },
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Query(crate::domain::activity::redact(&error.to_string()))
    }
}

impl StorageError {
    pub fn user_message(&self) -> String {
        match self {
            Self::FromTheFuture { .. } => {
                "This application's data was written by a newer version. Update Tunnel \
                 Manager, or move the data file aside to start fresh."
                    .into()
            }
            Self::Directory { path, .. } => {
                format!("Could not create the data directory at {path}.")
            }
            _ => "The local database could not be opened. Settings and saved hosts are \
                  unavailable for this session."
                .into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// `$XDG_DATA_HOME/tunnel-manager`, or the platform equivalent.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from(".").join(".local").join("share"))
        .join(DATA_DIR_NAME)
}

pub fn database_path() -> PathBuf {
    data_dir().join("tunnel-manager.sqlite3")
}

/// Opens the database at the standard location, creating the directory if it
/// is not there.
pub fn open_default() -> Result<Connection> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir).map_err(|error| StorageError::Directory {
        path: dir.display().to_string(),
        detail: error.to_string(),
    })?;

    // The data directory holds no secrets, but it does hold a list of the
    // user's hosts and hostnames, which is nobody else's business on a shared
    // machine.
    restrict_permissions(&dir);

    open(&database_path())
}

/// Opens (and migrates) a database at an explicit path. Tests use this with a
/// temporary directory; `:memory:` also works.
pub fn open(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)
        .map_err(|error| StorageError::Open(crate::domain::activity::redact(&error.to_string())))?;

    // WAL survives a crash without losing the last write, and lets a read
    // proceed while a write is in flight — which matters because settings are
    // written from a background thread while the UI reads them.
    let _: String =
        connection.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;

    migrate(&connection)?;
    Ok(connection)
}

/// Applies every migration the database has not seen.
///
/// `user_version` is used rather than a table of applied migrations: it is a
/// single integer in the file header, it cannot get out of step with the
/// schema, and reading it needs no schema at all.
pub fn migrate(connection: &Connection) -> Result<()> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if version > SCHEMA_VERSION {
        return Err(StorageError::FromTheFuture {
            found: version,
            supported: SCHEMA_VERSION,
        });
    }

    if version < 1 {
        connection.execute_batch(MIGRATION_1)?;
    }

    if version < 2 {
        connection.execute_batch(MIGRATION_2)?;
    }

    // Add further steps here as `if version < 3 { … }`, and bump
    // SCHEMA_VERSION. Never edit a migration that has shipped.

    connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// The initial schema.
///
/// `remote_hosts` holds a *reference* to a key, never a key. `has_passphrase`
/// records that one is filed in the OS credential store; the passphrase
/// itself is not, and cannot be, in this file.
const MIGRATION_1: &str = r#"
CREATE TABLE IF NOT EXISTS settings (
    key    TEXT PRIMARY KEY NOT NULL,
    value  TEXT NOT NULL
) STRICT;

CREATE TABLE IF NOT EXISTS remote_hosts (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    name                 TEXT    NOT NULL,
    address              TEXT    NOT NULL,
    port                 INTEGER NOT NULL DEFAULT 22,
    username             TEXT    NOT NULL,
    auth_kind            TEXT    NOT NULL CHECK (auth_kind IN ('agent', 'key-file')),
    key_path             TEXT,
    has_passphrase       INTEGER NOT NULL DEFAULT 0,
    host_key_fingerprint TEXT,
    service_unit         TEXT    NOT NULL DEFAULT 'cloudflared.service',
    tunnel_id            TEXT,
    notes                TEXT,
    created_at           TEXT    NOT NULL,
    last_seen_at         TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS remote_hosts_tunnel ON remote_hosts (tunnel_id);

CREATE TABLE IF NOT EXISTS activity (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    at        TEXT NOT NULL,
    severity  TEXT NOT NULL,
    category  TEXT NOT NULL,
    summary   TEXT NOT NULL,
    detail    TEXT,
    target    TEXT
) STRICT;

CREATE INDEX IF NOT EXISTS activity_at ON activity (at DESC);
"#;

/// Where a disabled hostname's real origin is remembered.
///
/// Disabling a public hostname rewrites its ingress rule to
/// `http_status:404`; this table keeps the service the rule served before, so
/// enabling it again is exact rather than a guess. One row per tunnel +
/// hostname + path, because a hostname may appear more than once with
/// different paths.
const MIGRATION_2: &str = r#"
CREATE TABLE IF NOT EXISTS disabled_hostnames (
    tunnel_id TEXT NOT NULL,
    hostname  TEXT NOT NULL,
    path      TEXT NOT NULL DEFAULT '',
    service   TEXT NOT NULL,
    PRIMARY KEY (tunnel_id, hostname, path)
) STRICT;
"#;

/// Makes the data directory owner-only. Best effort: a filesystem that does
/// not carry Unix modes is not an error worth failing startup over.
fn restrict_permissions(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;

    if let Ok(metadata) = std::fs::metadata(dir) {
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o077 != 0 {
            permissions.set_mode(0o700);
            let _ = std::fs::set_permissions(dir, permissions);
        }
    }
    // The database file itself inherits the process umask (often 022, i.e.
    // 0644) regardless of the directory mode, so lock it down too.
    let db = dir.join("tunnel-manager.sqlite3");
    if let Ok(metadata) = std::fs::metadata(&db) {
        let mut permissions = metadata.permissions();
        if permissions.mode() & 0o077 != 0 {
            permissions.set_mode(0o600);
            let _ = std::fs::set_permissions(&db, permissions);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrate(&connection).unwrap();
        connection
    }

    #[test]
    fn migration_creates_every_table() {
        let connection = memory();
        let tables: Vec<String> = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();

        for expected in ["activity", "remote_hosts", "settings"] {
            assert!(tables.contains(&expected.to_string()), "missing {expected}");
        }
    }

    #[test]
    fn migration_is_idempotent() {
        let connection = memory();
        migrate(&connection).unwrap();
        migrate(&connection).unwrap();

        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_is_refused_rather_than_downgraded() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .pragma_update(None, "user_version", SCHEMA_VERSION + 5)
            .unwrap();

        let error = migrate(&connection).unwrap_err();
        assert!(matches!(error, StorageError::FromTheFuture { .. }));
        assert!(error.user_message().contains("newer version"));
    }

    #[test]
    fn no_column_anywhere_could_hold_a_secret() {
        // A structural guarantee, checked rather than asserted in a comment:
        // if someone adds a `token` column, this fails.
        let connection = memory();
        let columns: Vec<String> = connection
            .prepare(
                "SELECT lower(m.name) || '.' || lower(p.name)
                 FROM sqlite_master m
                 JOIN pragma_table_info(m.name) p
                 WHERE m.type = 'table'",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();

        for column in &columns {
            let name = column.split('.').nth(1).unwrap_or_default();
            for forbidden in ["token", "password", "secret", "credential"] {
                assert!(
                    !name.contains(forbidden),
                    "{column} looks like it could hold a secret; secrets belong in the \
                     OS credential store"
                );
            }
        }
        // `has_passphrase` is allowed: it is a boolean, not a passphrase.
        assert!(columns.contains(&"remote_hosts.has_passphrase".to_string()));
    }

    #[test]
    fn the_auth_kind_check_constraint_holds() {
        let connection = memory();
        let insert = "INSERT INTO remote_hosts
             (name, address, username, auth_kind, created_at)
             VALUES ('h', 'example.test', 'root', ?1, '2026-01-01T00:00:00Z')";

        connection.execute(insert, ["agent"]).unwrap();
        connection.execute(insert, ["key-file"]).unwrap();
        assert!(connection.execute(insert, ["password"]).is_err());
    }

    #[test]
    fn the_database_path_is_under_the_app_data_directory() {
        let path = database_path();
        assert!(path.to_string_lossy().contains(DATA_DIR_NAME));
        assert_eq!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("sqlite3")
        );
    }

    #[test]
    fn opening_a_file_creates_and_migrates_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");

        {
            let connection = open(&path).unwrap();
            connection
                .execute("INSERT INTO settings (key, value) VALUES ('a', 'b')", [])
                .unwrap();
        }

        // Reopening finds the row and does not re-run the migration.
        let connection = open(&path).unwrap();
        let value: String = connection
            .query_row("SELECT value FROM settings WHERE key = 'a'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(value, "b");
    }
}
