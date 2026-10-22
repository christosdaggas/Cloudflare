//! `GObject` wrappers for the list models.
//!
//! `GtkColumnView` and `GtkListView` are model-driven: the model is the data,
//! a factory makes the row, and nothing in between keeps a copy. That model
//! has to be a `GListModel` of `GObject`s, so each domain type the application
//! shows in a list gets a thin wrapper here.
//!
//! The wrappers carry *display* strings as properties rather than the domain
//! values, for one reason: `GtkStringSorter` and `GtkStringFilter` work on a
//! `GtkPropertyExpression`, so a column is sortable and searchable for free
//! once the text it sorts on is a property. The domain value is kept alongside
//! for anything that needs the real thing.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::domain::activity::ActivityEvent;
use crate::domain::host::{RemoteHost, RemoteStatus};
use crate::domain::hostname::PublicHostname;
use crate::services::tunnels::TunnelRow;
use crate::ui::widgets as w;

// ── Tunnels ──────────────────────────────────────────────────────────────

mod tunnel_imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::TunnelObject)]
    pub struct TunnelObject {
        #[property(get, set)]
        pub id: RefCell<String>,
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub status: RefCell<String>,
        #[property(get, set)]
        pub source: RefCell<String>,
        #[property(get, set)]
        pub created: RefCell<String>,
        #[property(get, set)]
        pub last_active: RefCell<String>,
        #[property(get, set)]
        pub hostnames: RefCell<String>,
        #[property(get, set)]
        pub connectors: RefCell<String>,
        #[property(get, set)]
        pub hosts: RefCell<String>,
        /// Everything the search box matches against, pre-lowercased.
        #[property(get, set)]
        pub search: RefCell<String>,
        /// Sort keys, kept separate from the display strings so that
        /// "2 minutes ago" sorts by its timestamp and not alphabetically.
        #[property(get, set)]
        pub created_key: RefCell<String>,
        #[property(get, set)]
        pub active_key: RefCell<String>,
        #[property(get, set)]
        pub status_key: RefCell<String>,
        #[property(get, set)]
        pub hostname_count: std::cell::Cell<i32>,
        #[property(get, set)]
        pub connector_count: std::cell::Cell<i32>,

        pub row: RefCell<Option<TunnelRow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TunnelObject {
        const NAME: &'static str = "TunnelManagerTunnelObject";
        type Type = super::TunnelObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for TunnelObject {}
}

glib::wrapper! {
    pub struct TunnelObject(ObjectSubclass<tunnel_imp::TunnelObject>);
}

impl TunnelObject {
    pub fn new(row: &TunnelRow) -> Self {
        let tunnel = &row.tunnel;

        let object: Self = glib::Object::builder()
            .property("id", &tunnel.id)
            .property("name", &tunnel.name)
            .property("status", tunnel.status.label())
            .property("source", tunnel.config_source.label())
            .property("created", w::format_time(tunnel.created_at))
            .property("last-active", w::format_time(tunnel.last_active_at()))
            .property(
                "hostnames",
                match row.hostname_count {
                    Some(count) => count.to_string(),
                    None if tunnel.config_source.is_remotely_managed() => "…".to_string(),
                    None => "—".to_string(),
                },
            )
            .property("connectors", tunnel.connector_count.to_string())
            .property(
                "hosts",
                if row.managed_hosts.is_empty() {
                    "—".to_string()
                } else {
                    row.managed_hosts.join(", ")
                },
            )
            .property("search", row.search_text())
            .property("created-key", sort_key(tunnel.created_at))
            .property("active-key", sort_key(tunnel.last_active_at()))
            // Prefixed so the worst state sorts first: a list ordered by
            // status should put what needs attention at the top.
            .property(
                "status-key",
                format!("{}{}", status_rank(tunnel), tunnel.status.label()),
            )
            .property("hostname-count", row.hostname_count.unwrap_or(0) as i32)
            .property("connector-count", tunnel.connector_count as i32)
            .build();

        object.imp().row.replace(Some(row.clone()));
        object
    }

    pub fn row(&self) -> Option<TunnelRow> {
        self.imp().row.borrow().clone()
    }
}

/// Sorts newest-first when descending: an ISO timestamp sorts lexically, and
/// a missing one goes last either way.
fn sort_key(time: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match time {
        Some(time) => time.to_rfc3339(),
        None => "0000".into(),
    }
}

fn status_rank(tunnel: &crate::domain::tunnel::Tunnel) -> u8 {
    use crate::domain::tunnel::TunnelStatus::*;
    match tunnel.status {
        Down => 0,
        Degraded => 1,
        Inactive => 2,
        Other(_) => 3,
        Healthy => 4,
    }
}

// ── Public hostnames ─────────────────────────────────────────────────────

mod hostname_imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::HostnameObject)]
    pub struct HostnameObject {
        #[property(get, set)]
        pub hostname: RefCell<String>,
        #[property(get, set)]
        pub service: RefCell<String>,
        #[property(get, set)]
        pub tunnel: RefCell<String>,
        #[property(get, set)]
        pub zone: RefCell<String>,
        #[property(get, set)]
        pub dns: RefCell<String>,
        #[property(get, set)]
        pub search: RefCell<String>,

        pub value: RefCell<Option<PublicHostname>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HostnameObject {
        const NAME: &'static str = "TunnelManagerHostnameObject";
        type Type = super::HostnameObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for HostnameObject {}
}

glib::wrapper! {
    pub struct HostnameObject(ObjectSubclass<hostname_imp::HostnameObject>);
}

impl HostnameObject {
    pub fn new(hostname: &PublicHostname) -> Self {
        let object: Self = glib::Object::builder()
            .property("hostname", hostname.display_name())
            .property("service", &hostname.raw_service)
            .property("tunnel", &hostname.tunnel_name)
            .property(
                "zone",
                hostname.zone_name.clone().unwrap_or_else(|| "—".into()),
            )
            .property("dns", hostname.dns.label())
            .property(
                "search",
                format!(
                    "{} {} {} {}",
                    hostname.display_name(),
                    hostname.raw_service,
                    hostname.tunnel_name,
                    hostname.zone_name.clone().unwrap_or_default()
                )
                .to_lowercase(),
            )
            .build();

        object.imp().value.replace(Some(hostname.clone()));
        object
    }

    pub fn value(&self) -> Option<PublicHostname> {
        self.imp().value.borrow().clone()
    }
}

// ── Remote hosts ─────────────────────────────────────────────────────────

mod host_imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::HostObject)]
    pub struct HostObject {
        #[property(get, set)]
        pub id: std::cell::Cell<i64>,
        #[property(get, set)]
        pub name: RefCell<String>,
        #[property(get, set)]
        pub target: RefCell<String>,
        #[property(get, set)]
        pub status: RefCell<String>,
        #[property(get, set)]
        pub unit: RefCell<String>,
        #[property(get, set)]
        pub search: RefCell<String>,

        pub value: RefCell<Option<RemoteHost>>,
        pub state: RefCell<Option<RemoteStatus>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HostObject {
        const NAME: &'static str = "TunnelManagerHostObject";
        type Type = super::HostObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for HostObject {}
}

glib::wrapper! {
    pub struct HostObject(ObjectSubclass<host_imp::HostObject>);
}

impl HostObject {
    pub fn new(host: &RemoteHost, status: &RemoteStatus) -> Self {
        let object: Self = glib::Object::builder()
            .property("id", host.id)
            .property("name", &host.name)
            .property("target", host.display_target())
            .property("status", status.label())
            .property("unit", &host.service_unit)
            .property(
                "search",
                format!(
                    "{} {} {}",
                    host.name,
                    host.display_target(),
                    host.service_unit
                )
                .to_lowercase(),
            )
            .build();

        object.imp().value.replace(Some(host.clone()));
        object.imp().state.replace(Some(status.clone()));
        object
    }

    pub fn value(&self) -> Option<RemoteHost> {
        self.imp().value.borrow().clone()
    }

    pub fn state(&self) -> RemoteStatus {
        self.imp()
            .state
            .borrow()
            .clone()
            .unwrap_or(RemoteStatus::Unknown)
    }
}

// ── Activity ─────────────────────────────────────────────────────────────

mod activity_imp {
    use super::*;

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::ActivityObject)]
    pub struct ActivityObject {
        #[property(get, set)]
        pub summary: RefCell<String>,
        #[property(get, set)]
        pub detail: RefCell<String>,
        #[property(get, set)]
        pub target: RefCell<String>,
        #[property(get, set)]
        pub category: RefCell<String>,
        #[property(get, set)]
        pub severity: RefCell<String>,
        #[property(get, set)]
        pub when: RefCell<String>,
        #[property(get, set)]
        pub search: RefCell<String>,

        pub value: RefCell<Option<ActivityEvent>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ActivityObject {
        const NAME: &'static str = "TunnelManagerActivityObject";
        type Type = super::ActivityObject;
    }

    #[glib::derived_properties]
    impl ObjectImpl for ActivityObject {}
}

glib::wrapper! {
    pub struct ActivityObject(ObjectSubclass<activity_imp::ActivityObject>);
}

impl ActivityObject {
    pub fn new(event: &ActivityEvent) -> Self {
        let object: Self = glib::Object::builder()
            .property("summary", &event.summary)
            .property("detail", event.detail.clone().unwrap_or_default())
            .property("target", event.target.clone().unwrap_or_default())
            .property("category", event.category.label())
            .property("severity", event.severity.label())
            .property("when", w::format_time(Some(event.at)))
            .property(
                "search",
                format!(
                    "{} {} {} {}",
                    event.summary,
                    event.detail.clone().unwrap_or_default(),
                    event.target.clone().unwrap_or_default(),
                    event.category.label()
                )
                .to_lowercase(),
            )
            .build();

        object.imp().value.replace(Some(event.clone()));
        object
    }

    pub fn value(&self) -> Option<ActivityEvent> {
        self.imp().value.borrow().clone()
    }
}

/// Fills a `gio::ListStore` from a slice, replacing whatever was there.
///
/// `splice` rather than remove-all-then-append: it emits one `items-changed`
/// instead of two, so the view does not flash empty during a refresh.
pub fn replace<T: IsA<glib::Object>>(store: &gtk4::gio::ListStore, items: Vec<T>) {
    let objects: Vec<glib::Object> = items.into_iter().map(|item| item.upcast()).collect();
    store.splice(0, store.n_items(), &objects);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_sort_newest_last_lexically() {
        let older = sort_key(Some(
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ));
        let newer = sort_key(Some(
            chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        ));
        let never = sort_key(None);

        assert!(older < newer);
        assert!(
            never < older,
            "a tunnel that never ran sorts before any that did"
        );
    }

    #[test]
    fn status_ranking_puts_problems_first() {
        use crate::domain::tunnel::{ConfigSource, Tunnel, TunnelKind, TunnelStatus};

        let make = |status: TunnelStatus| Tunnel {
            id: "id".into(),
            account_tag: String::new(),
            name: String::new(),
            status,
            config_source: ConfigSource::Cloudflare,
            kind: TunnelKind::Cfd,
            remote_config: true,
            created_at: None,
            deleted_at: None,
            conns_active_at: None,
            conns_inactive_at: None,
            connector_count: 0,
            hostname_count: None,
        };

        assert!(
            status_rank(&make(TunnelStatus::Down)) < status_rank(&make(TunnelStatus::Degraded))
        );
        assert!(
            status_rank(&make(TunnelStatus::Degraded)) < status_rank(&make(TunnelStatus::Inactive))
        );
        assert!(
            status_rank(&make(TunnelStatus::Inactive)) < status_rank(&make(TunnelStatus::Healthy))
        );
    }
}
