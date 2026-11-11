//! The shared application state and the events that announce a change.
//!
//! This is the Tauri replacement for the GTK build's main-thread state object.
//! The data is the same — one session, one tunnel list, one host list, one
//! activity log — but it lives behind a `tokio::sync::Mutex` because commands
//! run concurrently, and every mutation ends by emitting the whole snapshot to
//! the webview. The frontend replaces its copy rather than trying to apply a
//! diff, which is what keeps a dropped event from leaving the UI permanently
//! stale.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};

use rusqlite::Connection;
use secrecy::SecretString;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::domain::activity::ActivityEvent;
use crate::domain::host::{RemoteHost, RemoteStatus};
use crate::domain::Account;
use crate::remote::{HostKey, RemoteConnectorController, SshController};
use crate::services::session::{ConnectionState, Session};
use crate::services::tunnels::TunnelRow;
use crate::storage::{self, Settings};

/// The event name the webview listens on.
pub const STATE_EVENT: &str = "state-changed";

/// A handle to the SQLite connection.
///
/// `Arc<Mutex<…>>` because it is moved into blocking tasks on Tokio's blocking
/// pool. `rusqlite::Connection` is `Send` but not `Sync`, so the mutex is what
/// makes the handle shareable, and holding it across an await is impossible by
/// construction because [`Store::with`] takes a closure.
#[derive(Clone)]
pub struct Store {
    connection: Arc<StdMutex<Option<Connection>>>,
}

impl Store {
    fn new(connection: Option<Connection>) -> Self {
        Self {
            connection: Arc::new(StdMutex::new(connection)),
        }
    }

    /// Runs `work` against the database. Returns `None` when the database
    /// could not be opened at startup, which is a degraded but usable state:
    /// settings fall back to defaults and nothing is persisted.
    pub fn with<T>(&self, work: impl FnOnce(&Connection) -> T) -> Option<T> {
        let guard = self.connection.lock().ok()?;
        let connection = guard.as_ref()?;
        Some(work(connection))
    }

    pub fn is_available(&self) -> bool {
        self.connection
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

struct Inner {
    store: Store,
    settings: Settings,
    session: Option<Session>,
    connection: ConnectionState,
    tunnels: Vec<TunnelRow>,
    /// The last tunnel-list failure, if the list is stale or empty because of
    /// it. Cleared on the next successful refresh.
    tunnels_error: Option<String>,
    hosts: Vec<RemoteHost>,
    host_status: HashMap<i64, RemoteStatus>,
    activity: Vec<ActivityEvent>,
    controller: Arc<dyn RemoteConnectorController>,
    busy: HashSet<String>,
    /// Whether the OS credential store answered at startup.
    secrets_available: bool,
    /// Set when the database could not be opened, for the banner.
    storage_problem: Option<String>,
    /// Host keys that have been probed but not yet trusted. Keyed by host ID.
    pending_host_keys: HashMap<i64, HostKey>,
    /// Whether changes are written through to storage.
    ///
    /// False only in tests, where there is no reason to touch the user's real
    /// database.
    persist: bool,
}

impl Inner {
    /// Fills each row's `managed_hosts` from the registered hosts.
    fn attach_hosts_to_tunnels(&mut self) {
        for row in &mut self.tunnels {
            row.managed_hosts = self
                .hosts
                .iter()
                .filter(|host| host.tunnel_id.as_deref() == Some(row.tunnel.id.as_str()))
                .map(|host| host.name.clone())
                .collect();
        }
    }

    fn snapshot(&self) -> AppStateDto {
        AppStateDto {
            connection: self.connection.clone(),
            account: self.session.as_ref().map(|session| session.account.clone()),
            zones: self
                .session
                .as_ref()
                .map(|session| session.zones.clone())
                .unwrap_or_default(),
            capabilities: self
                .session
                .as_ref()
                .map(|session| session.capabilities)
                .unwrap_or_default(),
            tunnels: self.tunnels.clone(),
            tunnels_error: self.tunnels_error.clone(),
            hosts: self.hosts.clone(),
            host_status: self
                .host_status
                .iter()
                .map(|(id, status)| HostStatusDto {
                    id: *id,
                    status: status.clone(),
                })
                .collect(),
            activity: self.activity.clone(),
            settings: self.settings.clone(),
            busy: {
                let mut keys: Vec<String> = self.busy.iter().cloned().collect();
                keys.sort();
                keys
            },
            secrets_available: self.secrets_available,
            storage_problem: self.storage_problem.clone(),
            database_path: crate::storage::db::database_path().display().to_string(),
            schema_version: crate::storage::SCHEMA_VERSION,
        }
    }
}

/// The whole state, in one message. Sent after every mutation.
#[derive(Debug, Clone, Serialize)]
pub struct AppStateDto {
    pub connection: ConnectionState,
    pub account: Option<Account>,
    pub zones: Vec<crate::domain::Zone>,
    pub capabilities: crate::domain::account::TokenCapabilities,
    pub tunnels: Vec<TunnelRow>,
    pub tunnels_error: Option<String>,
    pub hosts: Vec<RemoteHost>,
    pub host_status: Vec<HostStatusDto>,
    pub activity: Vec<ActivityEvent>,
    pub settings: Settings,
    pub busy: Vec<String>,
    pub secrets_available: bool,
    pub storage_problem: Option<String>,
    pub database_path: String,
    pub schema_version: i64,
}

/// A host's last known reachability, as a list because JSON object keys are
/// strings and an integer-keyed map would arrive as `{ "1": … }`.
#[derive(Debug, Clone, Serialize)]
pub struct HostStatusDto {
    pub id: i64,
    pub status: RemoteStatus,
}

/// The shared state. Cloning is cheap and gives the same underlying state.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Mutex<Inner>>,
    app: Arc<StdMutex<Option<AppHandle>>>,
}

impl AppState {
    /// Opens the database and loads settings.
    ///
    /// Called once, before the window is shown, so the synchronous reads here
    /// happen while nothing is on screen to stutter. Everything afterwards runs
    /// through a command.
    pub fn load() -> Self {
        let (connection, storage_problem) = match storage::db::open_default() {
            Ok(connection) => (Some(connection), None),
            Err(error) => {
                tracing::error!(%error, "could not open the database");
                (None, Some(error.user_message()))
            }
        };

        let store = Store::new(connection);
        let settings = store
            .with(|connection| storage::settings::load(connection).unwrap_or_default())
            .unwrap_or_default();
        let hosts = store
            .with(|connection| storage::hosts::list(connection).unwrap_or_default())
            .unwrap_or_default();
        let activity = store
            .with(|connection| storage::activity::recent(connection, 500).unwrap_or_default())
            .unwrap_or_default();

        // A saved token with a chosen account means startup will attempt a
        // silent reconnect. Starting in `Connecting` keeps the window on the
        // main shell while that runs, instead of flashing the connection page
        // for the second it takes the credential store and the API to answer.
        let initial = if settings.can_restore() {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };
        Self {
            inner: Arc::new(Mutex::new(Inner {
                store,
                settings,
                session: None,
                connection: initial,
                tunnels: Vec::new(),
                tunnels_error: None,
                hosts,
                host_status: HashMap::new(),
                activity,
                controller: Arc::new(SshController::new()),
                busy: HashSet::new(),
                secrets_available: storage::secrets::store_available(),
                storage_problem,
                pending_host_keys: HashMap::new(),
                persist: true,
            })),
            app: Arc::new(StdMutex::new(None)),
        }
    }

    /// A state with an injected controller and an in-memory database, for
    /// tests and for a future headless mode.
    #[cfg(test)]
    pub fn for_test(controller: Arc<dyn RemoteConnectorController>) -> Self {
        let connection = Connection::open_in_memory().ok().and_then(|connection| {
            storage::db::migrate(&connection).ok()?;
            Some(connection)
        });

        Self {
            inner: Arc::new(Mutex::new(Inner {
                store: Store::new(connection),
                settings: Settings::default(),
                session: None,
                connection: ConnectionState::Disconnected,
                tunnels: Vec::new(),
                tunnels_error: None,
                hosts: Vec::new(),
                host_status: HashMap::new(),
                activity: Vec::new(),
                controller,
                busy: HashSet::new(),
                secrets_available: false,
                storage_problem: None,
                pending_host_keys: HashMap::new(),
                persist: false,
            })),
            app: Arc::new(StdMutex::new(None)),
        }
    }

    /// Gives the state an `AppHandle` to emit through. Called once in `setup`.
    pub fn attach(&self, app: AppHandle) {
        if let Ok(mut slot) = self.app.lock() {
            *slot = Some(app);
        }
    }

    fn emitter(&self) -> Option<AppHandle> {
        self.app.lock().ok().and_then(|slot| slot.clone())
    }

    // ── Notification ─────────────────────────────────────────────────────

    pub async fn snapshot(&self) -> AppStateDto {
        self.inner.lock().await.snapshot()
    }

    /// Sends the whole state to the webview.
    ///
    /// Emitting after releasing the lock keeps a slow webview from blocking
    /// the next command; the cost is that two rapid changes can arrive out of
    /// order, which is harmless because each message is a complete snapshot.
    pub async fn emit_state(&self) {
        let Some(app) = self.emitter() else {
            return;
        };
        let snapshot = self.snapshot().await;
        let _ = app.emit(STATE_EVENT, snapshot);
    }

    // ── Connection ───────────────────────────────────────────────────────

    pub async fn connection(&self) -> ConnectionState {
        self.inner.lock().await.connection.clone()
    }

    pub async fn set_connection(&self, state: ConnectionState) {
        self.inner.lock().await.connection = state;
        self.emit_state().await;
    }

    pub async fn session(&self) -> Option<Session> {
        self.inner.lock().await.session.clone()
    }

    pub async fn set_session(&self, session: Option<Session>) {
        self.inner.lock().await.session = session;
        self.emit_state().await;
    }

    /// The live session, or the "not connected" error the commands return.
    pub async fn require_session(&self) -> Result<Session, crate::bridge::dto::CommandError> {
        self.session()
            .await
            .ok_or_else(crate::bridge::dto::CommandError::not_connected)
    }

    /// The account ID, from the live session or from saved settings.
    pub async fn account_id(&self) -> Option<String> {
        if let Some(session) = self.session().await {
            return Some(session.account.id);
        }
        self.settings().await.account_id
    }

    pub async fn is_connected(&self) -> bool {
        self.inner.lock().await.connection.is_connected()
    }

    /// Whether a silent reconnect is currently pending.
    pub async fn pending_restore(&self) -> bool {
        let inner = self.inner.lock().await;
        matches!(inner.connection, ConnectionState::Connecting) && inner.settings.can_restore()
    }

    pub async fn zones(&self) -> Vec<crate::domain::Zone> {
        self.session()
            .await
            .map(|session| session.zones)
            .unwrap_or_default()
    }

    pub async fn set_zones(&self, zones: Vec<crate::domain::Zone>) {
        if let Some(session) = self.inner.lock().await.session.as_mut() {
            session.zones = zones;
        }
        self.emit_state().await;
    }

    /// Records the evidence a capability checklist row needs: a call that
    /// requires Tunnel:Edit succeeded, so the token provably has it.
    pub async fn note_tunnel_access(&self, granted: bool) {
        if let Some(session) = self.inner.lock().await.session.as_mut() {
            session.note_tunnel_access(granted);
        }
        self.emit_state().await;
    }

    /// The DNS half of the same idea, recorded when a DNS write succeeded or
    /// was refused.
    pub async fn note_dns_access(&self, granted: bool) {
        if let Some(session) = self.inner.lock().await.session.as_mut() {
            session.note_dns_access(granted);
        }
        self.emit_state().await;
    }

    // ── Tunnels ──────────────────────────────────────────────────────────

    pub async fn tunnels(&self) -> Vec<TunnelRow> {
        self.inner.lock().await.tunnels.clone()
    }

    pub async fn set_tunnels(&self, tunnels: Vec<TunnelRow>) {
        {
            let mut inner = self.inner.lock().await;
            inner.tunnels = tunnels;
            inner.tunnels_error = None;
            inner.attach_hosts_to_tunnels();
        }
        self.emit_state().await;
    }

    pub async fn tunnels_error(&self) -> Option<String> {
        self.inner.lock().await.tunnels_error.clone()
    }

    pub async fn set_tunnels_error(&self, message: impl Into<String>) {
        self.inner.lock().await.tunnels_error = Some(message.into());
        self.emit_state().await;
    }

    pub async fn tunnel(&self, id: &str) -> Option<TunnelRow> {
        self.inner
            .lock()
            .await
            .tunnels
            .iter()
            .find(|row| row.tunnel.id == id)
            .cloned()
    }

    /// Replaces one row without disturbing the rest, so a per-tunnel refresh
    /// does not make the whole list flicker.
    pub async fn update_tunnel(&self, row: TunnelRow) {
        {
            let mut inner = self.inner.lock().await;
            match inner
                .tunnels
                .iter_mut()
                .find(|existing| existing.tunnel.id == row.tunnel.id)
            {
                Some(existing) => *existing = row,
                None => inner.tunnels.push(row),
            }
            inner.attach_hosts_to_tunnels();
        }
        self.emit_state().await;
    }

    pub async fn remove_tunnel(&self, id: &str) {
        self.inner
            .lock()
            .await
            .tunnels
            .retain(|row| row.tunnel.id != id);
        self.emit_state().await;
    }

    /// Fills in hostname counts collected after the list was already visible.
    /// One emit for the whole batch, so a large account does not flicker.
    pub async fn set_hostname_counts(&self, counts: Vec<(String, usize)>) {
        {
            let mut inner = self.inner.lock().await;
            for (id, count) in counts {
                if let Some(row) = inner.tunnels.iter_mut().find(|row| row.tunnel.id == id) {
                    row.hostname_count = Some(count);
                }
            }
        }
        self.emit_state().await;
    }

    // ── Hosts ────────────────────────────────────────────────────────────

    pub async fn hosts(&self) -> Vec<RemoteHost> {
        self.inner.lock().await.hosts.clone()
    }

    pub async fn host(&self, id: i64) -> Option<RemoteHost> {
        self.inner
            .lock()
            .await
            .hosts
            .iter()
            .find(|host| host.id == id)
            .cloned()
    }

    pub async fn set_hosts(&self, hosts: Vec<RemoteHost>) {
        {
            let mut inner = self.inner.lock().await;
            inner.hosts = hosts;
            inner.attach_hosts_to_tunnels();
        }
        self.emit_state().await;
    }

    /// Whether any remote host has been registered at all. Everything about
    /// remote control is hidden or explained rather than disabled when this is
    /// false.
    pub async fn has_hosts(&self) -> bool {
        !self.inner.lock().await.hosts.is_empty()
    }

    pub async fn host_status(&self, id: i64) -> RemoteStatus {
        self.inner
            .lock()
            .await
            .host_status
            .get(&id)
            .cloned()
            .unwrap_or(RemoteStatus::Unknown)
    }

    pub async fn set_host_status(&self, id: i64, status: RemoteStatus) {
        self.inner.lock().await.host_status.insert(id, status);
        self.emit_state().await;
    }

    pub async fn unreachable_host_count(&self) -> usize {
        self.inner
            .lock()
            .await
            .host_status
            .values()
            .filter(|status| status.is_unreachable())
            .count()
    }

    pub async fn controller(&self) -> Arc<dyn RemoteConnectorController> {
        Arc::clone(&self.inner.lock().await.controller)
    }

    pub async fn set_pending_host_key(&self, id: i64, key: HostKey) {
        self.inner.lock().await.pending_host_keys.insert(id, key);
    }

    pub async fn pending_host_key(&self, id: i64) -> Option<HostKey> {
        self.inner.lock().await.pending_host_keys.get(&id).cloned()
    }

    pub async fn clear_pending_host_key(&self, id: i64) {
        self.inner.lock().await.pending_host_keys.remove(&id);
    }

    // ── Activity ─────────────────────────────────────────────────────────

    pub async fn activity(&self) -> Vec<ActivityEvent> {
        self.inner.lock().await.activity.clone()
    }

    /// Records an event, in memory now and in the database shortly.
    pub async fn record(&self, event: ActivityEvent) {
        tracing::info!(
            category = ?event.category,
            severity = ?event.severity,
            "{}",
            event.summary
        );

        let (persist, store) = {
            let mut inner = self.inner.lock().await;
            inner.activity.insert(0, event.clone());
            // Bounded in memory; the database keeps a longer tail.
            inner.activity.truncate(500);
            (inner.persist, inner.store.clone())
        };

        if persist {
            tokio::task::spawn_blocking(move || {
                store.with(|connection| {
                    let _ = storage::activity::record(connection, &event);
                });
            })
            .await
            .ok();
        }

        self.emit_state().await;
    }

    pub async fn clear_activity(&self) {
        let (persist, store) = {
            let mut inner = self.inner.lock().await;
            inner.activity.clear();
            (inner.persist, inner.store.clone())
        };

        if persist {
            tokio::task::spawn_blocking(move || {
                store.with(|connection| {
                    let _ = storage::activity::clear(connection);
                });
            })
            .await
            .ok();
        }

        self.emit_state().await;
    }

    /// The most recent few events, for the dashboard.
    pub async fn recent_activity(&self, limit: usize) -> Vec<ActivityEvent> {
        self.inner
            .lock()
            .await
            .activity
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    // ── Settings ─────────────────────────────────────────────────────────

    pub async fn settings(&self) -> Settings {
        self.inner.lock().await.settings.clone()
    }

    /// Applies a change to settings and persists it.
    pub async fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        let (settings, persist, store) = {
            let mut inner = self.inner.lock().await;
            change(&mut inner.settings);
            (inner.settings.clone(), inner.persist, inner.store.clone())
        };

        if persist {
            tokio::task::spawn_blocking(move || {
                store.with(|connection| {
                    if let Err(error) = storage::settings::save(connection, &settings) {
                        tracing::error!(%error, "could not save settings");
                    }
                });
            })
            .await
            .ok();
        }

        self.emit_state().await;
    }

    pub async fn store(&self) -> Store {
        self.inner.lock().await.store.clone()
    }

    pub async fn storage_problem(&self) -> Option<String> {
        self.inner.lock().await.storage_problem.clone()
    }

    pub async fn secrets_available(&self) -> bool {
        self.inner.lock().await.secrets_available
    }

    pub async fn set_secrets_available(&self, available: bool) {
        self.inner.lock().await.secrets_available = available;
        self.emit_state().await;
    }

    // ── Busy state ───────────────────────────────────────────────────────

    /// Marks a named operation as running, so pages can show a spinner and
    /// disable the button that started it.
    pub async fn set_busy(&self, key: &str, busy: bool) {
        {
            let mut inner = self.inner.lock().await;
            if busy {
                inner.busy.insert(key.to_string());
            } else {
                inner.busy.remove(key);
            }
        }
        self.emit_state().await;
    }

    pub async fn is_busy(&self, key: &str) -> bool {
        self.inner.lock().await.busy.contains(key)
    }

    pub async fn is_anything_busy(&self) -> bool {
        !self.inner.lock().await.busy.is_empty()
    }

    // ── Disconnecting ────────────────────────────────────────────────────

    /// Forgets the token, the account and everything derived from them.
    ///
    /// Preferences — theme, refresh interval, SSH defaults — and registered
    /// hosts survive: they are not credentials, and a user who reconnects
    /// should not have to set them up again.
    pub async fn disconnect(&self) {
        {
            let mut inner = self.inner.lock().await;
            inner.busy.remove("connect");
            inner.session = None;
            inner.connection = ConnectionState::Disconnected;
            inner.tunnels.clear();
            inner.settings.account_id = None;
            inner.settings.account_name = None;
            inner.settings.default_zone_id = None;
            inner.settings.default_zone_name = None;
            inner.settings.token_saved = false;
        }

        let store = self.store().await;
        let result = tokio::task::spawn_blocking(move || {
            let cleared = storage::secrets::delete_api_token();
            store.with(|connection| {
                let _ = storage::settings::clear_connection(connection);
            });
            cleared
        })
        .await;

        let event = match result {
            Ok(Ok(())) => ActivityEvent::info(
                crate::domain::activity::ActivityCategory::App,
                "Disconnected from Cloudflare and removed the saved API token.",
            ),
            Ok(Err(error)) => ActivityEvent::warning(
                crate::domain::activity::ActivityCategory::App,
                format!(
                    "Disconnected, but the saved API token could not be removed from the \
                     credential store: {}",
                    error.user_message()
                ),
            ),
            Err(_) => ActivityEvent::warning(
                crate::domain::activity::ActivityCategory::App,
                "Disconnected, but the token removal task did not finish.",
            ),
        };
        self.record(event).await;
    }

    /// Saves a token to the OS credential store.
    pub async fn save_token(&self, token: SecretString) {
        let result = tokio::task::spawn_blocking(move || storage::secrets::save_api_token(&token))
            .await
            .unwrap_or_else(|_| {
                Err(storage::secrets::SecretError::Unavailable {
                    detail: "the credential task did not finish".into(),
                })
            });

        let saved = result.is_ok();
        if let Err(error) = &result {
            self.record(ActivityEvent::warning(
                crate::domain::activity::ActivityCategory::App,
                error.user_message(),
            ))
            .await;
            self.set_secrets_available(false).await;
        }
        self.update_settings(|settings| settings.token_saved = saved)
            .await;
    }

    /// Remembers the origin a disabled hostname's rule served, so Enable can
    /// restore it exactly. See `storage::disabled`.
    pub async fn remember_disabled_origin(
        &self,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
        service: &str,
    ) -> bool {
        let store = self.store().await;
        let tunnel_id = tunnel_id.to_string();
        let hostname = hostname.to_string();
        let path = path.map(str::to_string);
        let service = service.to_string();
        tokio::task::spawn_blocking(move || {
            store
                .with(|connection| {
                    storage::disabled::save(
                        connection,
                        &tunnel_id,
                        &hostname,
                        path.as_deref(),
                        &service,
                    )
                })
                // `None` means the database never opened: the origin cannot be
                // remembered, and the caller rightly refuses to disable.
                .map(|saved| saved.is_ok())
                .unwrap_or(false)
        })
        .await
        .unwrap_or(false)
    }

    /// The origin a disabled hostname's rule served, if one is remembered.
    pub async fn remembered_disabled_origin(
        &self,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
    ) -> Option<String> {
        let store = self.store().await;
        let tunnel_id = tunnel_id.to_string();
        let hostname = hostname.to_string();
        let path = path.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            store
                .with(|connection| {
                    storage::disabled::get(connection, &tunnel_id, &hostname, path.as_deref())
                })
                .and_then(|result| result.ok().flatten())
        })
        .await
        .ok()
        .flatten()
    }

    /// Forgets a disabled hostname's remembered origin: it is serving again,
    /// or its rule is gone.
    pub async fn forget_disabled_origin(
        &self,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
    ) {
        let store = self.store().await;
        let tunnel_id = tunnel_id.to_string();
        let hostname = hostname.to_string();
        let path = path.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let _ = store.with(|connection| {
                storage::disabled::delete(connection, &tunnel_id, &hostname, path.as_deref())
            });
        })
        .await
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::tunnel::{ConfigSource, Tunnel, TunnelKind, TunnelStatus};
    use crate::remote::MockController;

    fn state() -> AppState {
        AppState::for_test(Arc::new(MockController::with_defaults()))
    }

    fn tunnel(id: &str, name: &str) -> TunnelRow {
        TunnelRow::new(Tunnel {
            id: id.into(),
            account_tag: "a".repeat(32),
            name: name.into(),
            status: TunnelStatus::Healthy,
            config_source: ConfigSource::Cloudflare,
            kind: TunnelKind::Cfd,
            remote_config: true,
            created_at: None,
            deleted_at: None,
            conns_active_at: None,
            conns_inactive_at: None,
            connector_count: 1,
            hostname_count: None,
        })
    }

    fn host(id: i64, tunnel_id: Option<&str>) -> RemoteHost {
        RemoteHost {
            id,
            name: format!("host-{id}"),
            address: "edge.example.test".into(),
            port: 22,
            username: "deploy".into(),
            auth: crate::domain::host::RemoteAuth::Agent,
            host_key_fingerprint: Some("SHA256:x".into()),
            service_unit: "cloudflared.service".into(),
            tunnel_id: tunnel_id.map(str::to_string),
            created_at: chrono::Utc::now(),
            last_seen_at: None,
            notes: None,
        }
    }

    #[tokio::test]
    async fn a_fresh_state_is_disconnected_and_empty() {
        let state = state();
        assert!(!state.is_connected().await);
        assert!(!state.pending_restore().await);
        assert!(state.tunnels().await.is_empty());
        assert!(!state.has_hosts().await);
        assert!(state.zones().await.is_empty());
        assert!(state.account_id().await.is_none());
    }

    #[tokio::test]
    async fn hosts_are_attached_to_the_tunnels_they_serve() {
        let state = state();
        state
            .set_hosts(vec![
                host(1, Some("tunnel-a")),
                host(2, Some("tunnel-a")),
                host(3, Some("tunnel-b")),
                host(4, None),
            ])
            .await;
        state
            .set_tunnels(vec![tunnel("tunnel-a", "a"), tunnel("tunnel-b", "b")])
            .await;

        let rows = state.tunnels().await;
        assert_eq!(rows[0].managed_hosts, vec!["host-1", "host-2"]);
        assert_eq!(rows[1].managed_hosts, vec!["host-3"]);
    }

    #[tokio::test]
    async fn updating_one_tunnel_leaves_the_others_alone() {
        let state = state();
        state
            .set_tunnels(vec![tunnel("a", "first"), tunnel("b", "second")])
            .await;

        let mut changed = tunnel("b", "renamed");
        changed.hostname_count = Some(4);
        state.update_tunnel(changed).await;

        let rows = state.tunnels().await;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tunnel.name, "first");
        assert_eq!(rows[1].tunnel.name, "renamed");
        assert_eq!(rows[1].hostname_count, Some(4));
    }

    #[tokio::test]
    async fn busy_keys_are_independent() {
        let state = state();
        assert!(!state.is_anything_busy().await);

        state.set_busy("tunnels", true).await;
        assert!(state.is_busy("tunnels").await);
        assert!(!state.is_busy("hosts").await);
        assert!(state.is_anything_busy().await);

        state.set_busy("tunnels", false).await;
        assert!(!state.is_anything_busy().await);
    }

    #[tokio::test]
    async fn recorded_activity_is_newest_first_and_bounded() {
        let state = state();
        for index in 0..600 {
            state
                .record(ActivityEvent::info(
                    crate::domain::activity::ActivityCategory::App,
                    format!("event {index}"),
                ))
                .await;
        }

        let activity = state.activity().await;
        assert_eq!(activity.len(), 500);
        assert_eq!(activity[0].summary, "event 599");
        assert_eq!(state.recent_activity(3).await.len(), 3);
    }

    #[tokio::test]
    async fn unreachable_hosts_are_counted() {
        let state = state();
        state.set_hosts(vec![host(1, None), host(2, None)]).await;
        state
            .set_host_status(
                1,
                RemoteStatus::Unreachable {
                    message: "no route".into(),
                },
            )
            .await;
        state
            .set_host_status(
                2,
                RemoteStatus::Reachable {
                    service: crate::domain::host::RemoteServiceState::Active,
                    cloudflared_version: None,
                    systemd_available: true,
                    can_control_service: true,
                },
            )
            .await;

        assert_eq!(state.unreachable_host_count().await, 1);
        assert!(state.host_status(3).await == RemoteStatus::Unknown);
    }

    #[tokio::test]
    async fn settings_changes_are_visible_immediately() {
        let state = state();
        assert_eq!(state.settings().await.theme, "system");

        state
            .update_settings(|settings| settings.theme = "dark".into())
            .await;
        assert_eq!(state.settings().await.theme, "dark");
    }

    #[tokio::test]
    async fn the_account_id_falls_back_to_saved_settings() {
        let state = state();
        state
            .update_settings(|settings| settings.account_id = Some("a".repeat(32)))
            .await;
        assert_eq!(state.account_id().await, Some("a".repeat(32)));
    }
}
