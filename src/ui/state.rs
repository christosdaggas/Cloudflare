//! The application state the pages read, and the notifications they react to.
//!
//! This is the one object every page holds. It lives on the GTK main thread —
//! hence `Rc`/`RefCell` rather than `Arc`/`Mutex` — and nothing inside it
//! blocks: the SQLite connection is behind a handle that is only ever used
//! from [`crate::services::runtime::spawn_blocking`], and every network call
//! is spawned rather than awaited.
//!
//! Notification is a small observer list rather than GObject signals, for one
//! reason: a subscriber is held by a **weak reference to the widget it belongs
//! to**, and a dead one is pruned on the next emit. A page that is destroyed
//! while a request is in flight therefore stops receiving updates without
//! anybody having to remember to disconnect anything.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk4::prelude::*;
use rusqlite::Connection;
use secrecy::SecretString;

use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::{RemoteHost, RemoteStatus};
use crate::domain::Zone;
use crate::remote::{RemoteConnectorController, SshController};
use crate::services::session::{ConnectionState, Session};
use crate::services::tunnels::TunnelRow;
use crate::storage::{self, Settings};

/// What changed. Pages match on this rather than rebuilding on every event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    /// The token, account or capabilities changed.
    Connection,
    Tunnels,
    Zones,
    Hosts,
    Activity,
    Settings,
    /// A long-running operation started or finished. The payload is the key
    /// passed to [`AppState::set_busy`].
    Busy,
}

/// A handle to the SQLite connection.
///
/// `Arc<Mutex<…>>` because it is moved into blocking tasks on the Tokio
/// blocking pool. `rusqlite::Connection` is `Send` but not `Sync`, so the
/// mutex is what makes the handle shareable, and holding it across an await
/// is impossible by construction because [`Store::with`] takes a closure.
#[derive(Clone)]
pub struct Store {
    connection: Arc<Mutex<Option<Connection>>>,
}

impl Store {
    fn new(connection: Option<Connection>) -> Self {
        Self {
            connection: Arc::new(Mutex::new(connection)),
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

type Callback = Rc<dyn Fn(&AppState, Event)>;

struct Subscriber {
    /// Weak, so a destroyed page stops being called without unsubscribing.
    widget: gtk4::glib::WeakRef<gtk4::Widget>,
    callback: Callback,
}

struct Inner {
    store: Store,
    settings: RefCell<Settings>,
    session: RefCell<Option<Session>>,
    connection: RefCell<ConnectionState>,
    tunnels: RefCell<Vec<TunnelRow>>,
    /// The last tunnel-list failure, if the list is stale or empty because
    /// of it. Cleared on the next successful refresh.
    tunnels_error: RefCell<Option<String>>,
    hosts: RefCell<Vec<RemoteHost>>,
    host_status: RefCell<HashMap<i64, RemoteStatus>>,
    activity: RefCell<Vec<ActivityEvent>>,
    controller: Arc<dyn RemoteConnectorController>,
    subscribers: RefCell<Vec<Subscriber>>,
    busy: RefCell<HashMap<&'static str, bool>>,
    /// Whether the OS credential store answered at startup.
    secrets_available: Cell<bool>,
    /// Set when the database could not be opened, for the banner.
    storage_problem: RefCell<Option<String>>,
    /// Re-entrancy guard: a subscriber that changes state would otherwise
    /// notify from inside a notify.
    notifying: Cell<bool>,
    /// Whether changes are written through to storage.
    ///
    /// False only in tests. Persistence hops to the GLib main context to
    /// deliver its result, and a unit test has no main loop to hop to — the
    /// tests here assert on the in-memory behaviour, which is the part that
    /// has logic in it.
    persist: Cell<bool>,
}

/// The shared state. Cloning is cheap and gives the same underlying state.
#[derive(Clone)]
pub struct AppState {
    inner: Rc<Inner>,
}

impl AppState {
    /// Opens the database and loads settings.
    ///
    /// Called once, before the window exists, so the synchronous reads here
    /// happen while nothing is on screen to stutter. Everything afterwards
    /// goes through the runtime.
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
        // `restore_session` flips this to `Connected` or `Failed` once done.
        let initial = if settings.can_restore() {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };

        Self {
            inner: Rc::new(Inner {
                store,
                settings: RefCell::new(settings),
                session: RefCell::new(None),
                connection: RefCell::new(initial),
                tunnels: RefCell::new(Vec::new()),
                tunnels_error: RefCell::new(None),
                hosts: RefCell::new(hosts),
                host_status: RefCell::new(HashMap::new()),
                activity: RefCell::new(activity),
                controller: Arc::new(SshController::new()),
                subscribers: RefCell::new(Vec::new()),
                busy: RefCell::new(HashMap::new()),
                secrets_available: Cell::new(storage::secrets::store_available()),
                storage_problem: RefCell::new(storage_problem),
                notifying: Cell::new(false),
                persist: Cell::new(true),
            }),
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
            inner: Rc::new(Inner {
                store: Store::new(connection),
                settings: RefCell::new(Settings::default()),
                session: RefCell::new(None),
                connection: RefCell::new(ConnectionState::Disconnected),
                tunnels: RefCell::new(Vec::new()),
                tunnels_error: RefCell::new(None),
                hosts: RefCell::new(Vec::new()),
                host_status: RefCell::new(HashMap::new()),
                activity: RefCell::new(Vec::new()),
                controller,
                subscribers: RefCell::new(Vec::new()),
                busy: RefCell::new(HashMap::new()),
                secrets_available: Cell::new(false),
                storage_problem: RefCell::new(None),
                notifying: Cell::new(false),
                persist: Cell::new(false),
            }),
        }
    }

    // ── Notification ─────────────────────────────────────────────────────

    /// Calls `callback` whenever something changes, for as long as `widget`
    /// is alive.
    pub fn subscribe(
        &self,
        widget: &impl IsA<gtk4::Widget>,
        callback: impl Fn(&AppState, Event) + 'static,
    ) {
        let weak = widget.as_ref().downgrade();
        self.inner.subscribers.borrow_mut().push(Subscriber {
            widget: weak,
            callback: Rc::new(callback),
        });
    }

    pub fn notify(&self, event: Event) {
        // A subscriber that changes state during a notify would recurse; the
        // second notification is dropped rather than deepening the stack.
        if self.inner.notifying.get() {
            return;
        }
        self.inner.notifying.set(true);

        // Clone the list out before any callback runs. A callback is free to
        // subscribe, or to change state — the borrow is already released, so
        // neither panics on an outstanding `RefCell` borrow.
        let live: Vec<(gtk4::glib::WeakRef<gtk4::Widget>, Callback)> = {
            let mut subscribers = self.inner.subscribers.borrow_mut();
            subscribers.retain(|subscriber| subscriber.widget.upgrade().is_some());
            subscribers
                .iter()
                .map(|subscriber| (subscriber.widget.clone(), Rc::clone(&subscriber.callback)))
                .collect()
        };

        for (widget, callback) in live {
            // Re-check: an earlier callback may have destroyed a later
            // subscriber's widget.
            if widget.upgrade().is_some() {
                callback(self, event);
            }
        }

        self.inner.notifying.set(false);
    }

    // ── Connection ───────────────────────────────────────────────────────

    pub fn connection(&self) -> ConnectionState {
        self.inner.connection.borrow().clone()
    }

    pub fn set_connection(&self, state: ConnectionState) {
        *self.inner.connection.borrow_mut() = state;
        self.notify(Event::Connection);
    }

    pub fn session(&self) -> Option<Session> {
        self.inner.session.borrow().clone()
    }

    pub fn set_session(&self, session: Option<Session>) {
        *self.inner.session.borrow_mut() = session;
        self.notify(Event::Connection);
    }

    /// The account ID, from the live session or from saved settings.
    pub fn account_id(&self) -> Option<String> {
        self.session()
            .map(|session| session.account.id)
            .or_else(|| self.settings().account_id)
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connection.borrow().is_connected()
    }

    /// Whether a silent reconnect is currently pending.
    pub fn pending_restore(&self) -> bool {
        matches!(self.connection(), ConnectionState::Connecting) && self.settings().can_restore()
    }

    pub fn zones(&self) -> Vec<Zone> {
        self.session()
            .map(|session| session.zones)
            .unwrap_or_default()
    }

    pub fn set_zones(&self, zones: Vec<Zone>) {
        if let Some(session) = self.inner.session.borrow_mut().as_mut() {
            session.zones = zones;
        }
        self.notify(Event::Zones);
    }

    // ── Tunnels ──────────────────────────────────────────────────────────

    pub fn tunnels(&self) -> Vec<TunnelRow> {
        self.inner.tunnels.borrow().clone()
    }

    pub fn set_tunnels(&self, tunnels: Vec<TunnelRow>) {
        *self.inner.tunnels.borrow_mut() = tunnels;
        *self.inner.tunnels_error.borrow_mut() = None;
        self.attach_hosts_to_tunnels();
        self.notify(Event::Tunnels);
    }

    /// The last tunnel-list failure, if any.
    pub fn tunnels_error(&self) -> Option<String> {
        self.inner.tunnels_error.borrow().clone()
    }

    pub fn set_tunnels_error(&self, message: impl Into<String>) {
        *self.inner.tunnels_error.borrow_mut() = Some(message.into());
        self.notify(Event::Tunnels);
    }

    pub fn tunnel(&self, id: &str) -> Option<TunnelRow> {
        self.inner
            .tunnels
            .borrow()
            .iter()
            .find(|row| row.tunnel.id == id)
            .cloned()
    }

    /// Replaces one row without disturbing the rest, so a per-tunnel refresh
    /// does not make the whole list flicker.
    pub fn update_tunnel(&self, row: TunnelRow) {
        {
            let mut tunnels = self.inner.tunnels.borrow_mut();
            match tunnels
                .iter_mut()
                .find(|existing| existing.tunnel.id == row.tunnel.id)
            {
                Some(existing) => *existing = row,
                None => tunnels.push(row),
            }
        }
        self.attach_hosts_to_tunnels();
        self.notify(Event::Tunnels);
    }

    pub fn remove_tunnel(&self, id: &str) {
        self.inner
            .tunnels
            .borrow_mut()
            .retain(|row| row.tunnel.id != id);
        self.notify(Event::Tunnels);
    }

    /// Fills each row's `managed_hosts` from the registered hosts.
    fn attach_hosts_to_tunnels(&self) {
        let hosts = self.inner.hosts.borrow();
        for row in self.inner.tunnels.borrow_mut().iter_mut() {
            row.managed_hosts = hosts
                .iter()
                .filter(|host| host.tunnel_id.as_deref() == Some(row.tunnel.id.as_str()))
                .map(|host| host.name.clone())
                .collect();
        }
    }

    // ── Hosts ────────────────────────────────────────────────────────────

    pub fn hosts(&self) -> Vec<RemoteHost> {
        self.inner.hosts.borrow().clone()
    }

    pub fn host(&self, id: i64) -> Option<RemoteHost> {
        self.inner
            .hosts
            .borrow()
            .iter()
            .find(|host| host.id == id)
            .cloned()
    }

    pub fn set_hosts(&self, hosts: Vec<RemoteHost>) {
        *self.inner.hosts.borrow_mut() = hosts;
        self.attach_hosts_to_tunnels();
        self.notify(Event::Hosts);
    }

    /// Whether any remote host has been registered at all. Everything about
    /// remote control is hidden or explained rather than disabled when this is
    /// false.
    pub fn has_hosts(&self) -> bool {
        !self.inner.hosts.borrow().is_empty()
    }

    pub fn host_status(&self, id: i64) -> RemoteStatus {
        self.inner
            .host_status
            .borrow()
            .get(&id)
            .cloned()
            .unwrap_or(RemoteStatus::Unknown)
    }

    pub fn set_host_status(&self, id: i64, status: RemoteStatus) {
        self.inner.host_status.borrow_mut().insert(id, status);
        self.notify(Event::Hosts);
    }

    pub fn unreachable_host_count(&self) -> usize {
        self.inner
            .host_status
            .borrow()
            .values()
            .filter(|status| status.is_unreachable())
            .count()
    }

    pub fn controller(&self) -> Arc<dyn RemoteConnectorController> {
        Arc::clone(&self.inner.controller)
    }

    // ── Activity ─────────────────────────────────────────────────────────

    pub fn activity(&self) -> Vec<ActivityEvent> {
        self.inner.activity.borrow().clone()
    }

    /// Records an event, in memory now and in the database shortly.
    pub fn record(&self, event: ActivityEvent) {
        tracing::info!(
            category = ?event.category,
            severity = ?event.severity,
            "{}",
            event.summary
        );

        self.inner.activity.borrow_mut().insert(0, event.clone());
        // Bounded in memory; the database keeps a longer tail.
        self.inner.activity.borrow_mut().truncate(500);
        self.notify(Event::Activity);

        if !self.inner.persist.get() {
            return;
        }

        let store = self.store();
        crate::services::runtime::spawn_blocking(
            move || {
                store.with(|connection| {
                    let _ = storage::activity::record(connection, &event);
                });
            },
            |()| {},
        );
    }

    pub fn clear_activity(&self) {
        self.inner.activity.borrow_mut().clear();
        self.notify(Event::Activity);

        if !self.inner.persist.get() {
            return;
        }

        let store = self.store();
        crate::services::runtime::spawn_blocking(
            move || {
                store.with(|connection| {
                    let _ = storage::activity::clear(connection);
                });
            },
            |()| {},
        );
    }

    /// The most recent few events, for the dashboard.
    pub fn recent_activity(&self, limit: usize) -> Vec<ActivityEvent> {
        self.inner
            .activity
            .borrow()
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    // ── Settings ─────────────────────────────────────────────────────────

    pub fn settings(&self) -> Settings {
        self.inner.settings.borrow().clone()
    }

    /// Applies a change to settings and persists it.
    pub fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        let settings = {
            let mut settings = self.inner.settings.borrow_mut();
            change(&mut settings);
            settings.clone()
        };

        self.notify(Event::Settings);

        if !self.inner.persist.get() {
            return;
        }

        let store = self.store();
        crate::services::runtime::spawn_blocking(
            move || {
                store.with(|connection| {
                    if let Err(error) = storage::settings::save(connection, &settings) {
                        tracing::error!(%error, "could not save settings");
                    }
                });
            },
            |()| {},
        );
    }

    pub fn store(&self) -> Store {
        self.inner.store.clone()
    }

    pub fn storage_problem(&self) -> Option<String> {
        self.inner.storage_problem.borrow().clone()
    }

    pub fn secrets_available(&self) -> bool {
        self.inner.secrets_available.get()
    }

    pub fn set_secrets_available(&self, available: bool) {
        self.inner.secrets_available.set(available);
        self.notify(Event::Settings);
    }

    // ── Busy state ───────────────────────────────────────────────────────

    /// Marks a named operation as running, so pages can show a spinner and
    /// disable the button that started it.
    pub fn set_busy(&self, key: &'static str, busy: bool) {
        self.inner.busy.borrow_mut().insert(key, busy);
        self.notify(Event::Busy);
    }

    pub fn is_busy(&self, key: &str) -> bool {
        self.inner.busy.borrow().get(key).copied().unwrap_or(false)
    }

    pub fn is_anything_busy(&self) -> bool {
        self.inner.busy.borrow().values().any(|busy| *busy)
    }

    // ── Disconnecting ────────────────────────────────────────────────────

    /// Forgets the token, the account and everything derived from them.
    ///
    /// Preferences — theme, refresh interval, SSH defaults — and registered
    /// hosts survive: they are not credentials, and a user who reconnects
    /// should not have to set them up again.
    pub fn disconnect(&self) {
        self.set_busy("connect", false);
        self.set_session(None);
        self.set_connection(ConnectionState::Disconnected);
        self.set_tunnels(Vec::new());

        self.update_settings(|settings| {
            settings.account_id = None;
            settings.account_name = None;
            settings.default_zone_id = None;
            settings.default_zone_name = None;
            settings.token_saved = false;
        });

        let store = self.store();
        crate::services::runtime::spawn_blocking(
            move || -> Result<(), storage::secrets::SecretError> {
                let cleared = storage::secrets::delete_api_token();
                store.with(|connection| {
                    let _ = storage::settings::clear_connection(connection);
                });
                cleared
            },
            {
                let state = self.clone();
                move |result: Result<(), storage::secrets::SecretError>| {
                    let event = match result {
                        Ok(()) => ActivityEvent::info(
                            ActivityCategory::App,
                            "Disconnected from Cloudflare and removed the saved API token.",
                        ),
                        Err(error) => ActivityEvent::warning(
                            ActivityCategory::App,
                            format!(
                                "Disconnected, but the saved API token could not be \
                                 removed from the credential store: {}",
                                error.user_message()
                            ),
                        ),
                    };
                    state.record(event);
                }
            },
        );
    }

    /// Saves a token to the OS credential store.
    pub fn save_token(&self, token: SecretString, on_done: impl FnOnce(bool) + 'static) {
        let state = self.clone();
        crate::services::runtime::spawn_blocking(
            move || storage::secrets::save_api_token(&token),
            move |result| {
                let saved = result.is_ok();
                if let Err(error) = &result {
                    state.record(ActivityEvent::warning(
                        ActivityCategory::App,
                        error.user_message(),
                    ));
                    state.set_secrets_available(false);
                }
                state.update_settings(|settings| settings.token_saved = saved);
                on_done(saved);
            },
        );
    }

    /// Remembers the origin a disabled hostname's rule served, so Enable can
    /// restore it exactly. See `storage::disabled`.
    pub fn remember_disabled_origin(
        &self,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
        service: &str,
        on_done: impl FnOnce(bool) + 'static,
    ) {
        let store = self.store();
        let tunnel_id = tunnel_id.to_string();
        let hostname = hostname.to_string();
        let path = path.map(str::to_string);
        let service = service.to_string();
        crate::services::runtime::spawn_blocking(
            move || {
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
                    // `None` means the database never opened: the origin cannot
                    // be remembered, and the caller rightly refuses to disable.
                    .map(|saved| saved.is_ok())
                    .unwrap_or(false)
            },
            on_done,
        );
    }

    /// The origin a disabled hostname's rule served, if one is remembered.
    pub fn remembered_disabled_origin(
        &self,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
        on_done: impl FnOnce(Option<String>) + 'static,
    ) {
        let store = self.store();
        let tunnel_id = tunnel_id.to_string();
        let hostname = hostname.to_string();
        let path = path.map(str::to_string);
        crate::services::runtime::spawn_blocking(
            move || {
                store
                    .with(|connection| {
                        storage::disabled::get(connection, &tunnel_id, &hostname, path.as_deref())
                    })
                    // A closed database reports "no origin remembered", and the
                    // enable step then says so instead of inventing one.
                    .and_then(|result| result.ok().flatten())
            },
            on_done,
        );
    }

    /// Forgets a disabled hostname's remembered origin: it is serving again,
    /// or its rule is gone.
    pub fn forget_disabled_origin(
        &self,
        tunnel_id: &str,
        hostname: &str,
        path: Option<&str>,
        on_done: impl FnOnce() + 'static,
    ) {
        let store = self.store();
        let tunnel_id = tunnel_id.to_string();
        let hostname = hostname.to_string();
        let path = path.map(str::to_string);
        crate::services::runtime::spawn_blocking(
            move || {
                let _ = store.with(|connection| {
                    storage::disabled::delete(connection, &tunnel_id, &hostname, path.as_deref())
                });
            },
            move |_| on_done(),
        );
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

    #[test]
    fn a_fresh_state_is_disconnected_and_empty() {
        let state = state();
        assert!(!state.is_connected());
        assert!(!state.pending_restore());
        assert!(state.tunnels().is_empty());
        assert!(!state.has_hosts());
        assert!(state.zones().is_empty());
        assert!(state.account_id().is_none());
    }

    #[test]
    fn restore_is_pending_only_while_connecting_with_a_saved_token_and_an_account() {
        let state = state();
        assert!(!state.pending_restore());

        state.update_settings(|settings| settings.token_saved = true);
        assert!(
            !state.pending_restore(),
            "a token without a chosen account still shows the connection page"
        );

        state.update_settings(|settings| {
            settings.account_id = Some("a".repeat(32));
            settings.account_name = Some("Acme".into());
        });
        state.set_connection(ConnectionState::Connecting);
        assert!(state.pending_restore());

        state.set_connection(ConnectionState::Failed {
            title: "Rejected".into(),
            message: "The token was rejected.".into(),
        });
        assert!(!state.pending_restore());
    }

    #[test]
    fn hosts_are_attached_to_the_tunnels_they_serve() {
        let state = state();
        state.set_hosts(vec![
            host(1, Some("tunnel-a")),
            host(2, Some("tunnel-a")),
            host(3, Some("tunnel-b")),
            host(4, None),
        ]);
        state.set_tunnels(vec![tunnel("tunnel-a", "a"), tunnel("tunnel-b", "b")]);

        let rows = state.tunnels();
        assert_eq!(rows[0].managed_hosts, vec!["host-1", "host-2"]);
        assert_eq!(rows[1].managed_hosts, vec!["host-3"]);
    }

    #[test]
    fn updating_one_tunnel_leaves_the_others_alone() {
        let state = state();
        state.set_tunnels(vec![tunnel("a", "first"), tunnel("b", "second")]);

        let mut changed = tunnel("b", "renamed");
        changed.hostname_count = Some(4);
        state.update_tunnel(changed);

        let rows = state.tunnels();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].tunnel.name, "first");
        assert_eq!(rows[1].tunnel.name, "renamed");
        assert_eq!(rows[1].hostname_count, Some(4));
    }

    #[test]
    fn updating_an_unknown_tunnel_appends_it() {
        let state = state();
        state.set_tunnels(vec![tunnel("a", "first")]);
        state.update_tunnel(tunnel("b", "second"));
        assert_eq!(state.tunnels().len(), 2);
    }

    #[test]
    fn busy_keys_are_independent() {
        let state = state();
        assert!(!state.is_anything_busy());

        state.set_busy("tunnels", true);
        assert!(state.is_busy("tunnels"));
        assert!(!state.is_busy("hosts"));
        assert!(state.is_anything_busy());

        state.set_busy("tunnels", false);
        assert!(!state.is_anything_busy());
    }

    #[test]
    fn recorded_activity_is_newest_first_and_bounded() {
        let state = state();
        for index in 0..600 {
            state.record(ActivityEvent::info(
                ActivityCategory::App,
                format!("event {index}"),
            ));
        }

        let activity = state.activity();
        assert_eq!(activity.len(), 500);
        assert_eq!(activity[0].summary, "event 599");
        assert_eq!(state.recent_activity(3).len(), 3);
    }

    #[test]
    fn unreachable_hosts_are_counted() {
        let state = state();
        state.set_hosts(vec![host(1, None), host(2, None)]);
        state.set_host_status(
            1,
            RemoteStatus::Unreachable {
                message: "no route".into(),
            },
        );
        state.set_host_status(
            2,
            RemoteStatus::Reachable {
                service: crate::domain::host::RemoteServiceState::Active,
                cloudflared_version: None,
                systemd_available: true,
                can_control_service: true,
            },
        );

        assert_eq!(state.unreachable_host_count(), 1);
        assert!(state.host_status(3) == RemoteStatus::Unknown);
    }

    #[test]
    fn settings_changes_are_visible_immediately() {
        let state = state();
        assert_eq!(state.settings().theme, "system");

        state.update_settings(|settings| settings.theme = "dark".into());
        assert_eq!(state.settings().theme, "dark");
    }

    #[test]
    fn the_account_id_falls_back_to_saved_settings() {
        let state = state();
        state.update_settings(|settings| settings.account_id = Some("a".repeat(32)));
        assert_eq!(state.account_id(), Some("a".repeat(32)));
    }

    #[test]
    fn an_unopenable_database_leaves_a_usable_state() {
        // `Store::with` returning None is the degraded path; nothing should
        // panic and defaults should apply.
        let store = Store::new(None);
        assert!(!store.is_available());
        assert!(store.with(|_| 1).is_none());
    }
}
