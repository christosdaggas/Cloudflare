//! One tunnel, in detail.
//!
//! The page is organised around a distinction the Cloudflare dashboard blurs
//! and this application does not: **the tunnel's health at the edge and a
//! connector's service state on a machine are different facts**, gathered from
//! different places, and either can be true without the other. They therefore
//! live in different sections, are labelled differently, and are never
//! combined into one "enabled" switch.
//!
//! For the same reason there is no generic Enable/Disable control. Every
//! action names exactly what it does — Start Connector, Publish Hostname,
//! Delete Tunnel — because "disable" would have to mean one of four different
//! things depending on what the user was looking at.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::cloudflare::Cancel;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::{RemoteStatus, TransportProtocol};
use crate::domain::hostname::PublicHostname;
use crate::domain::tunnel::{Connector, Tunnel};
use crate::remote::commands::ServiceAction;
use crate::services::local::LocalState;
use crate::services::{hostnames, hosts, local, runtime, tunnels};
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TunnelDetailPage {
        pub state: RefCell<Option<AppState>>,
        pub tunnel: RefCell<Option<Tunnel>>,
        pub hostnames: RefCell<Vec<PublicHostname>>,
        pub connectors: RefCell<Vec<Connector>>,
        pub hostnames_loading: std::cell::Cell<bool>,
        pub hostnames_error: RefCell<Option<String>>,

        pub stack: RefCell<Option<adw::ViewStack>>,
        pub overview_box: RefCell<Option<gtk4::Box>>,
        pub hostnames_box: RefCell<Option<gtk4::Box>>,
        pub connectors_box: RefCell<Option<gtk4::Box>>,
        pub config_box: RefCell<Option<gtk4::Box>>,
        pub hosts_box: RefCell<Option<gtk4::Box>>,
        pub activity_box: RefCell<Option<gtk4::Box>>,

        /// The last local inspection. `None` means "check" has not been
        /// pressed on this page — the service is never probed behind the
        /// user's back on every rebuild.
        pub local: RefCell<Option<LocalState>>,

        pub cancel: RefCell<Option<Cancel>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TunnelDetailPage {
        const NAME: &'static str = "TunnelManagerTunnelDetailPage";
        type Type = super::TunnelDetailPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TunnelDetailPage {
        fn dispose(&self) {
            if let Some(cancel) = self.cancel.borrow().as_ref() {
                cancel.cancel();
            }
        }
    }

    impl WidgetImpl for TunnelDetailPage {}
    impl BinImpl for TunnelDetailPage {}
}

glib::wrapper! {
    pub struct TunnelDetailPage(ObjectSubclass<imp::TunnelDetailPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl TunnelDetailPage {
    pub fn new(state: &AppState, tunnel: &Tunnel) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.imp().tunnel.replace(Some(tunnel.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                // Settings matters because the local connector's association
                // lives there; Busy matters because the local buttons disable
                // themselves while an operation runs.
                if !matches!(
                    event,
                    Event::Tunnels | Event::Hosts | Event::Activity | Event::Settings | Event::Busy
                ) {
                    return;
                }
                if let Some(page) = page.upgrade() {
                    page.rebuild_sections();
                }
            }
        });

        page.load();
        page
    }

    fn state(&self) -> AppState {
        self.imp()
            .state
            .borrow()
            .clone()
            .expect("every page is constructed with a state before it can be shown")
    }

    fn tunnel(&self) -> Tunnel {
        self.imp()
            .tunnel
            .borrow()
            .clone()
            .expect("the detail page is constructed with a tunnel")
    }

    fn build(&self) {
        let imp = self.imp();
        let stack = adw::ViewStack::new();
        stack.set_vexpand(true);
        // A view stack is homogeneous by default, so its minimum size is the
        // maximum over all its pages, not just the visible one. Left on, the
        // widest section would set this page's floor.
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);

        let sections: &[(&str, &str, &str)] = &[
            ("overview", "Overview", "view-grid-symbolic"),
            // These titles are the switcher's labels as well as the
            // sections' names. Kept to one word where the surrounding page
            // already supplies the context, because six icon-and-label
            // items did not fit the default window width: "Public
            // Hostnames" was rendering as "Public Ho…".
            ("hostnames", "Hostnames", "web-browser-symbolic"),
            (
                "connectors",
                "Connectors",
                "network-transmit-receive-symbolic",
            ),
            ("config", "Configuration", "document-edit-symbolic"),
            ("hosts", "Hosts", "network-server-symbolic"),
            ("activity", "Activity", "document-open-recent-symbolic"),
        ];

        let mut boxes = Vec::new();
        for (name, title, icon) in sections {
            let column = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Vertical)
                .spacing(24)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(12)
                .margin_end(12)
                .build();

            let clamp = adw::Clamp::builder()
                .maximum_size(crate::ui::page::CONTENT_WIDTH)
                .tightening_threshold(crate::ui::page::CONTENT_WIDTH - 200)
                .child(&column)
                .build();

            let scroller = gtk4::ScrolledWindow::builder()
                .hscrollbar_policy(gtk4::PolicyType::Never)
                .vexpand(true)
                .hexpand(true)
                .child(&clamp)
                .build();

            stack.add_titled_with_icon(&scroller, Some(name), title, icon);
            boxes.push(column);
        }

        imp.overview_box.replace(Some(boxes[0].clone()));
        imp.hostnames_box.replace(Some(boxes[1].clone()));
        imp.connectors_box.replace(Some(boxes[2].clone()));
        imp.config_box.replace(Some(boxes[3].clone()));
        imp.hosts_box.replace(Some(boxes[4].clone()));
        imp.activity_box.replace(Some(boxes[5].clone()));
        imp.stack.replace(Some(stack.clone()));

        // An inline switcher rather than a header one: the header already
        // carries the back button and the tunnel's name, and six sections do
        // not fit beside them on a narrow window.
        let switcher = adw::InlineViewSwitcher::builder()
            .stack(&stack)
            .display_mode(adw::InlineViewSwitcherDisplayMode::Both)
            .halign(gtk4::Align::Center)
            .margin_top(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        // Width deliberately does not propagate: six icon+label items would
        // set the page's minimum width and break the 360px floor.
        let switcher_scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_width(false)
            .propagate_natural_height(true)
            .child(&switcher)
            .build();

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        root.append(&switcher_scroller);
        root.append(&stack);

        // Below this width six icon-and-label tabs cannot fit, and Adwaita
        // shrinks each label to an ellipsis rather than letting the scroller
        // scroll. Icons alone are readable at that size and keep every
        // section one click away; above it the labels are shown in full.
        // The threshold is the widest the switcher measured, not the window
        // width, because the switcher sits inside the page's own margins.
        let frame = adw::BreakpointBin::builder()
            .width_request(200)
            .height_request(200)
            .vexpand(true)
            .hexpand(true)
            .child(&root)
            .build();
        if let Ok(condition) = adw::BreakpointCondition::parse("max-width: 760sp") {
            let breakpoint = adw::Breakpoint::new(condition);
            breakpoint.add_setter(
                &switcher,
                "display-mode",
                Some(&adw::InlineViewSwitcherDisplayMode::Icons.to_value()),
            );
            frame.add_breakpoint(breakpoint);
        }

        self.set_child(Some(&frame));
        self.rebuild_sections();
    }

    // ── Loading ──────────────────────────────────────────────────────────

    fn load(&self) {
        self.load_hostnames();
        self.load_connectors();
    }

    fn load_hostnames(&self) {
        let state = self.state();
        let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
            return;
        };
        let tunnel = self.tunnel();
        if !tunnel.config_source.is_remotely_managed() {
            return;
        }

        let cancel = Cancel::new();
        self.imp().cancel.replace(Some(cancel.clone()));
        self.imp().hostnames_loading.set(true);
        self.imp().hostnames_error.replace(None);
        self.rebuild_sections();

        let client = session.client.clone();
        let zones = session.zones.clone();
        let page = self.downgrade();
        let name = tunnel.name.clone();
        let id = tunnel.id.clone();

        runtime::spawn(
            async move {
                hostnames::load_hostnames(&client, &account_id, &id, &name, &zones, &cancel).await
            },
            move |result| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                match result {
                    Ok(hostnames) => {
                        page.imp().hostnames.replace(hostnames);
                        page.imp().hostnames_loading.set(false);
                        page.imp().hostnames_error.replace(None);
                        page.rebuild_sections();
                    }
                    Err(error) if error.is_cancelled() => {
                        page.imp().hostnames_loading.set(false);
                    }
                    Err(error) => {
                        page.imp().hostnames_loading.set(false);
                        page.imp()
                            .hostnames_error
                            .replace(Some(error.user_message()));
                        page.state().record(ActivityEvent::error(
                            ActivityCategory::Api,
                            error.user_message(),
                        ));
                        page.rebuild_sections();
                    }
                }
            },
        );
    }

    fn load_connectors(&self) {
        let state = self.state();
        let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
            return;
        };
        let tunnel = self.tunnel();
        let cancel = Cancel::new();
        let client = session.client.clone();
        let page = self.downgrade();

        runtime::spawn(
            async move { tunnels::connectors(&client, &account_id, &tunnel.id, &cancel).await },
            move |result| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if let Ok(connectors) = result {
                    page.imp().connectors.replace(connectors);
                    page.rebuild_sections();
                }
            },
        );
    }

    // ── Sections ─────────────────────────────────────────────────────────

    fn rebuild_sections(&self) {
        let imp = self.imp();

        // Keep the tunnel in step with the list, so a rename or a status
        // change made elsewhere shows here without reopening the page.
        if let Some(row) = self.state().tunnel(&self.tunnel().id) {
            imp.tunnel.replace(Some(row.tunnel));
        }

        if let Some(column) = imp.overview_box.borrow().clone() {
            clear(&column);
            self.build_overview(&column);
        }
        if let Some(column) = imp.hostnames_box.borrow().clone() {
            clear(&column);
            self.build_hostnames(&column);
        }
        if let Some(column) = imp.connectors_box.borrow().clone() {
            clear(&column);
            self.build_connectors(&column);
        }
        if let Some(column) = imp.config_box.borrow().clone() {
            clear(&column);
            self.build_configuration(&column);
        }
        if let Some(column) = imp.hosts_box.borrow().clone() {
            clear(&column);
            self.build_managed_hosts(&column);
        }
        if let Some(column) = imp.activity_box.borrow().clone() {
            clear(&column);
            self.build_activity(&column);
        }
    }

    fn build_overview(&self, column: &gtk4::Box) {
        let tunnel = self.tunnel();

        if let Some(refusal) = tunnel.config_source.edit_refusal() {
            let banner = adw::Banner::builder().title(refusal).revealed(true).build();
            column.append(&w::wrappable(&banner));
        }

        let group = w::group(
            "Cloudflare",
            "What Cloudflare's edge reports about this tunnel. This is not the same as \
             whether cloudflared is running on any particular machine.",
        );
        group.add(&w::status_row(
            "Tunnel status",
            tunnel.status.icon_name(),
            tunnel.status.label(),
            tunnel.status.css_class(),
            Some(tunnel.status.explanation()),
        ));
        group.add(&w::property_row(
            "Configuration",
            tunnel.config_source.label(),
        ));
        group.add(&w::property_row("Kind", tunnel.kind.label()));
        group.add(&w::copyable_row("Tunnel ID", &tunnel.id));
        group.add(&w::copyable_row("CNAME target", &tunnel.cname_target()));
        group.add(&w::property_row(
            "Created",
            &w::format_time(tunnel.created_at),
        ));
        group.add(&w::property_row(
            "Last active",
            &w::format_time(tunnel.last_active_at()),
        ));
        group.add(&w::property_row(
            "Connectors registered",
            &tunnel.connector_count.to_string(),
        ));
        column.append(&group);

        column.append(&self.build_actions());
    }

    fn build_actions(&self) -> adw::PreferencesGroup {
        let tunnel = self.tunnel();
        let editable = tunnel.is_editable();

        let group = w::group(
            "Actions",
            "Every action names exactly what it changes. There is no single \
             enable/disable control, because a tunnel, its hostnames and each of its \
             connectors are separate things.",
        );

        let publish = adw::ButtonRow::builder()
            .title("Publish Hostname")
            .start_icon_name("list-add-symbolic")
            .build();
        publish.set_sensitive(editable);
        {
            let page = self.downgrade();
            publish.connect_activated(move |_| {
                if let Some(page) = page.upgrade() {
                    crate::ui::hostname_dialog::present_new(
                        &page,
                        &page.state(),
                        &page.tunnel(),
                        move |page| page.load_hostnames(),
                    );
                }
            });
        }
        group.add(&publish);

        let rename = adw::ButtonRow::builder()
            .title("Rename Tunnel")
            .start_icon_name("document-edit-symbolic")
            .build();
        {
            let page = self.downgrade();
            rename.connect_activated(move |row| {
                if let Some(page) = page.upgrade() {
                    page.rename(row);
                }
            });
        }
        group.add(&rename);

        let reinstall = adw::ButtonRow::builder()
            .title("Install Connector Again")
            .start_icon_name("system-software-install-symbolic")
            .build();
        {
            let page = self.downgrade();
            reinstall.connect_activated(move |_| {
                if let Some(page) = page.upgrade() {
                    crate::ui::reinstall_dialog::present(&page, &page.state(), &page.tunnel());
                }
            });
        }
        group.add(&reinstall);

        let refresh = adw::ButtonRow::builder()
            .title("Refresh")
            .start_icon_name("view-refresh-symbolic")
            .build();
        {
            let page = self.downgrade();
            refresh.connect_activated(move |_| {
                if let Some(page) = page.upgrade() {
                    page.load();
                    crate::ui::refresh_tunnels(&page.state());
                }
            });
        }
        group.add(&refresh);

        let delete = adw::ButtonRow::builder()
            .title("Delete Tunnel")
            .start_icon_name("user-trash-symbolic")
            .build();
        delete.add_css_class("destructive-action");
        {
            let page = self.downgrade();
            delete.connect_activated(move |row| {
                if let Some(page) = page.upgrade() {
                    page.confirm_delete(row);
                }
            });
        }
        group.add(&delete);

        group
    }

    fn build_hostnames(&self, column: &gtk4::Box) {
        let tunnel = self.tunnel();
        let hostnames = self.imp().hostnames.borrow().clone();

        if !tunnel.config_source.is_remotely_managed() {
            column.append(&w::error_state(
                "dialog-information-symbolic",
                "Configured elsewhere",
                tunnel
                    .config_source
                    .edit_refusal()
                    .unwrap_or("This tunnel's configuration is not managed from here."),
                None,
            ));
            return;
        }

        if self.imp().hostnames_loading.get() {
            column.append(&w::loading_state("Loading hostnames…"));
            return;
        }

        if let Some(message) = self.imp().hostnames_error.borrow().clone() {
            if hostnames.is_empty() {
                let button = gtk4::Button::builder().label("Try Again").build();
                {
                    let page = self.downgrade();
                    button.connect_clicked(move |_| {
                        if let Some(page) = page.upgrade() {
                            page.load_hostnames();
                        }
                    });
                }
                column.append(&w::error_state(
                    "network-error-symbolic",
                    "Could not load hostnames",
                    &message,
                    Some(button),
                ));
                return;
            }
        }

        if hostnames.is_empty() {
            let button = gtk4::Button::builder().label("Publish Hostname").build();
            {
                let page = self.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(page) = page.upgrade() {
                        crate::ui::hostname_dialog::present_new(
                            &page,
                            &page.state(),
                            &page.tunnel(),
                            move |page| page.load_hostnames(),
                        );
                    }
                });
            }
            column.append(&w::empty_state(
                "web-browser-symbolic",
                "No public hostnames",
                "Publishing a hostname adds an ingress rule to this tunnel and a proxied \
                 CNAME to the matching zone.",
                Some(button),
            ));
            return;
        }

        let group = w::group(
            "Published hostnames",
            "Each is an ingress rule on this tunnel plus a DNS record in its zone. Both \
             halves have to be right for the hostname to work.",
        );

        for hostname in &hostnames {
            group.add(&self.build_hostname_row(hostname));
        }

        column.append(&group);
    }

    fn build_hostname_row(&self, hostname: &PublicHostname) -> adw::ExpanderRow {
        let tunnel = self.tunnel();

        let row = adw::ExpanderRow::builder()
            .title(hostname.display_name())
            .subtitle(&hostname.raw_service)
            .build();

        if hostname.disabled {
            row.add_suffix(&w::status_badge(
                "media-playback-pause-symbolic",
                "Disabled",
                "status-neutral",
            ));
        } else {
            row.add_suffix(&w::status_badge(
                hostname.dns.icon_name(),
                hostname.dns.label(),
                hostname.dns.css_class(),
            ));
        }

        row.add_row(&w::property_row("Origin service", &hostname.raw_service));
        if hostname.disabled {
            row.add_row(&w::property_row(
                "Status",
                "Disabled — traffic answers 404 until you enable it again",
            ));
        }
        // How cloudflared talks to the origin, when the rule says anything
        // about it. Only what is actually set — an empty list means all
        // defaults, and no rows are shown at all.
        if let Some(origin) = &hostname.origin_request {
            for (label, value) in origin.summary() {
                row.add_row(&w::property_row(&label, &value));
            }
        }
        row.add_row(&w::property_row(
            "Zone",
            hostname
                .zone_name
                .as_deref()
                .unwrap_or("Not on this account"),
        ));
        row.add_row(&w::property_row(
            "Proxy state",
            match &hostname.dns {
                crate::domain::hostname::DnsState::Correct { .. } => "Proxied",
                crate::domain::hostname::DnsState::NotProxied { .. } => "Not proxied",
                _ => "—",
            },
        ));

        let detail = adw::ActionRow::builder()
            .title("DNS")
            .subtitle(hostname.dns.detail(&tunnel.name))
            .build();
        detail.add_prefix(&gtk4::Image::from_icon_name(hostname.dns.icon_name()));
        row.add_row(&detail);

        let actions = adw::ActionRow::builder().title("Actions").build();
        let wrap = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .valign(gtk4::Align::Center)
            .build();

        let edit = gtk4::Button::builder().label("Edit Origin").build();
        edit.set_sensitive(tunnel.is_editable() && !hostname.disabled);
        {
            let page = self.downgrade();
            let hostname = hostname.clone();
            edit.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    crate::ui::hostname_dialog::present_edit(
                        &page,
                        &page.state(),
                        &page.tunnel(),
                        &hostname,
                        move |page| page.load_hostnames(),
                    );
                }
            });
        }
        wrap.append(&edit);

        if hostname.disabled {
            let enable = gtk4::Button::builder()
                .label("Enable")
                .css_classes(["suggested-action"])
                .build();
            enable.set_sensitive(tunnel.is_editable());
            {
                let page = self.downgrade();
                let hostname = hostname.clone();
                enable.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        page.enable_hostname(button, &hostname);
                    }
                });
            }
            wrap.append(&enable);
        } else {
            if hostname.dns.is_repairable() {
                let repair = gtk4::Button::builder()
                    .label("Repair DNS")
                    .css_classes(["suggested-action"])
                    .build();
                {
                    let page = self.downgrade();
                    let hostname = hostname.clone();
                    repair.connect_clicked(move |button| {
                        if let Some(page) = page.upgrade() {
                            page.repair_dns(button, &hostname);
                        }
                    });
                }
                wrap.append(&repair);
            }

            let disable = gtk4::Button::builder().label("Disable").build();
            disable.set_sensitive(tunnel.is_editable());
            disable.set_tooltip_text(Some(
                "Traffic will answer 404 until you enable it again; the origin is \
                 remembered.",
            ));
            {
                let page = self.downgrade();
                let hostname = hostname.clone();
                disable.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        page.confirm_disable_hostname(button, &hostname);
                    }
                });
            }
            wrap.append(&disable);
        }

        let unpublish = gtk4::Button::builder()
            .label("Unpublish")
            .css_classes(["destructive-action"])
            .build();
        unpublish.set_sensitive(tunnel.is_editable());
        {
            let page = self.downgrade();
            let hostname = hostname.clone();
            unpublish.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    page.confirm_unpublish(button, &hostname);
                }
            });
        }
        wrap.append(&unpublish);

        actions.add_suffix(&wrap);
        row.add_row(&actions);
        row
    }

    fn build_connectors(&self, column: &gtk4::Box) {
        let connectors = self.imp().connectors.borrow().clone();
        let tunnel = self.tunnel();

        let explanation = w::inline_message(
            "dialog-information-symbolic",
            "A connector is one cloudflared process. Cloudflare reports each connector's \
             connections to its edge; whether the process is running on a machine is a \
             separate fact, shown under Managed Hosts.",
            "dimmed",
        );
        column.append(&explanation);

        if connectors.is_empty() {
            let reinstall = gtk4::Button::builder()
                .label("Install Connector Again")
                .css_classes(["pill", "suggested-action"])
                .build();
            {
                let page = self.downgrade();
                reinstall.connect_clicked(move |_| {
                    if let Some(page) = page.upgrade() {
                        crate::ui::reinstall_dialog::present(&page, &page.state(), &page.tunnel());
                    }
                });
            }
            column.append(&w::empty_state(
                "network-transmit-receive-symbolic",
                "No connectors registered",
                "Nothing is currently serving this tunnel. Install the connector again \
                 on the machine, or deploy it to a host.",
                Some(reinstall),
            ));
            return;
        }

        let group = w::group(
            &w::plural(connectors.len(), "Connector", "Connectors"),
            "Each row is one cloudflared process and the edge connections it holds open. \
             A healthy connector has four.",
        );

        for connector in &connectors {
            let row = adw::ExpanderRow::builder()
                .title(format!(
                    "cloudflared {}",
                    if connector.version.is_empty() {
                        "(unknown version)"
                    } else {
                        &connector.version
                    }
                ))
                .subtitle(connector.origin_ip().unwrap_or("origin IP not reported"))
                .build();

            let established = connector.established_count();
            let (icon, class) = if established >= 4 {
                ("object-select-symbolic", "success")
            } else if established > 0 {
                ("dialog-warning-symbolic", "warning")
            } else {
                ("dialog-error-symbolic", "error")
            };
            row.add_suffix(&w::status_badge(
                icon,
                &format!("{established}/{}", connector.connections.len().max(4)),
                class,
            ));

            row.add_row(&w::copyable_row("Connector ID", &connector.id));
            row.add_row(&w::property_row("Architecture", &connector.arch));
            row.add_row(&w::property_row(
                "Started",
                &w::format_time(connector.run_at),
            ));
            row.add_row(&w::property_row(
                "Colocations",
                &connector.colos().join(", "),
            ));
            if let Some(version) = connector.config_version {
                row.add_row(&w::property_row(
                    "Configuration version",
                    &version.to_string(),
                ));
            }
            if !connector.features.is_empty() {
                row.add_row(&w::property_row("Features", &connector.features.join(", ")));
            }

            let disconnect = adw::ActionRow::builder()
                .title("Deregister at the edge")
                .subtitle(
                    "Removes this connector's registration. It does not stop cloudflared \
                     on the machine — the process will reconnect. To stop the process, \
                     use Managed Hosts.",
                )
                .build();
            let button = gtk4::Button::builder()
                .label("Deregister")
                .valign(gtk4::Align::Center)
                .css_classes(["destructive-action"])
                .build();
            {
                let page = self.downgrade();
                let connector_id = connector.id.clone();
                let others = connectors.len().saturating_sub(1);
                button.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        page.confirm_deregister(button, &connector_id, others);
                    }
                });
            }
            disconnect.add_suffix(&button);
            row.add_row(&disconnect);

            group.add(&row);
        }

        column.append(&group);

        if connectors.len() > 1 {
            column.append(&w::inline_message(
                "dialog-information-symbolic",
                &format!(
                    "{} are serving “{}”. Stopping one of them leaves the others serving \
                     traffic; the tunnel only goes down when the last one stops.",
                    w::plural(connectors.len(), "connector", "connectors"),
                    tunnel.name
                ),
                "accent",
            ));
        }
    }

    fn build_configuration(&self, column: &gtk4::Box) {
        let tunnel = self.tunnel();

        if !tunnel.config_source.is_remotely_managed() {
            column.append(&w::error_state(
                "dialog-information-symbolic",
                "Locally configured",
                tunnel.config_source.edit_refusal().unwrap_or_default(),
                None,
            ));
            return;
        }

        let hostnames = self.imp().hostnames.borrow().clone();

        let group = w::group(
            "Ingress rules",
            "Written by this application in one read-modify-write. Rules it does not \
             manage — and fields inside them it does not understand — are preserved \
             exactly as they were.",
        );

        for (index, hostname) in hostnames.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(format!("{}. {}", index + 1, hostname.display_name()))
                .subtitle(&hostname.raw_service)
                .build();
            row.add_css_class("property");
            group.add(&row);
        }

        let catch_all = adw::ActionRow::builder()
            .title(format!("{}. Everything else", hostnames.len() + 1))
            .subtitle(crate::domain::config::CATCH_ALL_SERVICE)
            .build();
        catch_all.add_css_class("property");
        catch_all.add_prefix(&gtk4::Image::from_icon_name("view-more-symbolic"));
        group.add(&catch_all);

        column.append(&group);

        column.append(&w::inline_message(
            "dialog-information-symbolic",
            "cloudflared stops at the first rule that matches, and requires the last \
             rule to match everything. This application keeps that rule last \
             automatically and will not let it be removed.",
            "dimmed",
        ));

        let token_group = w::group(
            "Run token",
            "The credential a connector needs to serve this tunnel. Anyone holding it \
             can serve the tunnel, so it is fetched only when asked for and never \
             stored by this application.",
        );

        let copy = adw::ButtonRow::builder()
            .title("Copy Run Token")
            .start_icon_name("edit-copy-symbolic")
            .build();
        {
            let page = self.downgrade();
            copy.connect_activated(move |row| {
                if let Some(page) = page.upgrade() {
                    page.copy_run_token(row);
                }
            });
        }
        token_group.add(&copy);
        column.append(&token_group);
    }

    fn build_managed_hosts(&self, column: &gtk4::Box) {
        let state = self.state();
        let tunnel = self.tunnel();

        let hosts: Vec<_> = state
            .hosts()
            .into_iter()
            .filter(|host| host.tunnel_id.as_deref() == Some(tunnel.id.as_str()))
            .collect();

        if hosts.is_empty() {
            let button = gtk4::Button::builder()
                .label("Set Up Remote Control")
                .action_name("win.add-host")
                .build();

            column.append(&w::empty_state(
                "network-server-symbolic",
                "Remote control not configured",
                "No registered host is associated with this tunnel. Start, Stop, Restart \
                 and Logs are unavailable until one is — every other feature on this page \
                 works regardless.",
                Some(button),
            ));

            column.append(&w::copyable_code_block(
                &crate::services::hosts::manual_setup_instructions(&tunnel.id),
                "Or run the connector yourself:",
            ));

            let guide = gtk4::Button::builder()
                .label("Open Reinstall Guide")
                .halign(gtk4::Align::Center)
                .css_classes(["pill"])
                .build();
            {
                let page = self.downgrade();
                guide.connect_clicked(move |_| {
                    if let Some(page) = page.upgrade() {
                        crate::ui::reinstall_dialog::present(&page, &page.state(), &page.tunnel());
                    }
                });
            }
            column.append(&guide);
            self.build_local_connector(column);
            return;
        }

        let group = w::group(
            "Hosts running this tunnel",
            "The service state below comes from systemd on each machine. It is a \
             different fact from the tunnel status on the Overview tab.",
        );

        for host in &hosts {
            let status = state.host_status(host.id);
            let row = adw::ActionRow::builder()
                .title(&host.name)
                .subtitle(format!("{} · {}", host.display_target(), host.service_unit))
                .build();
            row.add_prefix(&gtk4::Image::from_icon_name("network-server-symbolic"));
            row.add_suffix(&w::status_badge(
                status.icon_name(),
                &status.label(),
                status.css_class(),
            ));

            let open = gtk4::Button::builder()
                .icon_name("go-next-symbolic")
                .valign(gtk4::Align::Center)
                .tooltip_text("Open Remote Hosts")
                .action_name("win.page")
                .action_target(&"hosts".to_variant())
                .css_classes(["flat"])
                .build();
            open.update_property(&[gtk4::accessible::Property::Label("Open Remote Hosts")]);
            row.add_suffix(&open);
            row.set_activatable_widget(Some(&open));

            group.add(&row);
        }

        column.append(&group);

        let more = w::group(
            "Another machine",
            "The connector can serve this tunnel from any number of machines at once.",
        );
        let reinstall = adw::ButtonRow::builder()
            .title("Install the Connector Again")
            .start_icon_name("system-software-install-symbolic")
            .build();
        {
            let page = self.downgrade();
            reinstall.connect_activated(move |_| {
                if let Some(page) = page.upgrade() {
                    crate::ui::reinstall_dialog::present(&page, &page.state(), &page.tunnel());
                }
            });
        }
        more.add(&reinstall);
        column.append(&more);

        self.build_local_connector(column);
    }

    /// The connector on this machine, when it has been associated with this
    /// tunnel on the Remote Hosts page.
    ///
    /// This is the tunnel-settings home of the two things users reach for
    /// most: restarting the local connector, and pinning its transport
    /// protocol. The service is only ever inspected when "Check" is pressed
    /// — never as a side effect of rebuilding the page.
    fn build_local_connector(&self, column: &gtk4::Box) {
        let state = self.state();
        let tunnel = self.tunnel();
        if state.settings().local_tunnel_id.as_deref() != Some(tunnel.id.as_str()) {
            return;
        }
        let unit = state.settings().local_service_unit.clone();
        let cached = self.imp().local.borrow().clone();
        let checking = state.is_busy("local-check");
        let acting = state.is_busy("local-action");

        let group = w::group(
            "This machine",
            "The connector on the computer running this application. Full controls \
             — including which service unit this is — live on the Remote Hosts page.",
        );

        let status = match (&cached, checking) {
            (None, true) => RemoteStatus::Checking,
            (None, false) => RemoteStatus::Unknown,
            (Some(found), _) => RemoteStatus::Reachable {
                service: found.service_state.clone(),
                cloudflared_version: found.cloudflared_version.clone(),
                systemd_available: found.systemd_available,
                can_control_service: found.can_control(),
            },
        };

        let head = adw::ActionRow::builder()
            .title("Connector service")
            .subtitle(format!(
                "{} · {}",
                unit,
                cached
                    .as_ref()
                    .and_then(|found| found.cloudflared_version.clone())
                    .map(|version| format!("cloudflared {version}"))
                    .unwrap_or_else(|| "not checked yet".into())
            ))
            .build();
        head.add_prefix(&gtk4::Image::from_icon_name("computer-symbolic"));
        head.add_suffix(&w::status_badge(
            status.icon_name(),
            &status.label(),
            status.css_class(),
        ));
        group.add(&head);

        if let Some(reason) = cached.as_ref().and_then(|found| found.blocking_reason()) {
            let note = w::inline_message("dialog-warning-symbolic", &reason, "dimmed");
            note.set_margin_top(6);
            group.add(&note);
        }

        let protocol = cached.as_ref().and_then(|found| found.protocol);
        let transport = adw::ComboRow::builder()
            .title("Transport protocol")
            .subtitle(match protocol {
                Some(choice) => choice.explanation().to_string(),
                None => "cloudflared negotiates QUIC over UDP and falls back on its own. \
                         Pin HTTP/2 over TCP when UDP flows stall."
                    .to_string(),
            })
            .model(&gtk4::StringList::new(&[
                "Auto",
                TransportProtocol::Quic.label(),
                TransportProtocol::Http2.label(),
            ]))
            .sensitive(status.can_control() && !acting)
            .build();
        if !status.can_control() {
            transport.set_tooltip_text(Some("Not available until this machine can be controlled — press Check above, or see the Remote Hosts page."));
        }
        transport.set_selected(match protocol {
            None => 0,
            Some(TransportProtocol::Quic) => 1,
            Some(TransportProtocol::Http2) => 2,
        });
        {
            let page = self.downgrade();
            transport.connect_selected_notify(move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let choice = match row.selected() {
                    1 => Some(TransportProtocol::Quic),
                    2 => Some(TransportProtocol::Http2),
                    _ => None,
                };
                let current = page
                    .imp()
                    .local
                    .borrow()
                    .clone()
                    .and_then(|found| found.protocol);
                if current == choice {
                    return;
                }
                detail_apply_transport(&page, row, choice);
            });
        }
        group.add(&transport);

        let actions = adw::ActionRow::builder().title("Actions").build();
        let wrap = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .valign(gtk4::Align::Center)
            .build();

        let check = gtk4::Button::builder()
            .label("Check")
            .sensitive(!checking && !acting)
            .build();
        {
            let page = self.downgrade();
            check.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    detail_check_local(&page);
                }
            });
        }
        wrap.append(&check);

        let can_control = status.can_control();
        let restart = gtk4::Button::builder()
            .label("Restart")
            .sensitive(can_control && !checking && !acting)
            .build();
        if !can_control {
            restart.set_tooltip_text(Some(
                cached
                    .as_ref()
                    .map(|found| found.control_hint())
                    .unwrap_or("Not available until this machine has been checked."),
            ));
        }
        {
            let page = self.downgrade();
            restart.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    detail_restart_local(&page, button);
                }
            });
        }
        wrap.append(&restart);

        let logs = gtk4::Button::builder()
            .label("Logs")
            .sensitive(cached.as_ref().is_some_and(|found| found.can_inspect()) && !checking)
            .build();
        {
            let page = self.downgrade();
            logs.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    detail_logs_local(&page, button);
                }
            });
        }
        wrap.append(&logs);

        actions.add_suffix(&wrap);
        group.add(&actions);

        column.append(&group);
    }

    fn build_activity(&self, column: &gtk4::Box) {
        let tunnel = self.tunnel();
        let events: Vec<_> = self
            .state()
            .activity()
            .into_iter()
            .filter(|event| {
                event.target.as_deref() == Some(tunnel.name.as_str())
                    || event.summary.contains(&tunnel.name)
                    || event.summary.contains(&tunnel.id)
            })
            .take(50)
            .collect();

        if events.is_empty() {
            column.append(&w::empty_state(
                "document-open-recent-symbolic",
                "Nothing yet",
                "Operations this application performs on this tunnel will be listed here.",
                None,
            ));
            return;
        }

        let group = w::group(
            "Recent operations",
            "This application's own record. Not Cloudflare's audit log.",
        );

        for event in events {
            let row = adw::ActionRow::builder()
                .title(&event.summary)
                .subtitle(format!(
                    "{} · {}",
                    event.category.label(),
                    w::format_time(Some(event.at))
                ))
                .build();
            let wrapper = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
            wrapper.add_css_class(event.severity.css_class());
            wrapper.set_valign(gtk4::Align::Center);
            wrapper.append(&gtk4::Image::from_icon_name(event.severity.icon_name()));
            row.add_prefix(&wrapper);
            group.add(&row);
        }

        column.append(&group);
    }

    // ── Operations ───────────────────────────────────────────────────────

    fn rename(&self, anchor: &impl IsA<gtk4::Widget>) {
        let tunnel = self.tunnel();
        let state = self.state();

        let entry = gtk4::Entry::builder()
            .text(&tunnel.name)
            .activates_default(true)
            .build();
        entry.update_property(&[gtk4::accessible::Property::Label("Tunnel name")]);

        let dialog = adw::AlertDialog::new(
            Some("Rename tunnel"),
            Some(
                "The name is only a label. It does not change the tunnel's ID, its \
                 hostnames, or anything a running connector depends on.",
            ),
        );
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("rename", "Rename");
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        // Invalid names never reach the API: the response stays disabled
        // until the entry validates, as in the create wizard.
        dialog.set_response_enabled(
            "rename",
            crate::domain::validation::tunnel_name(&tunnel.name).is_ok(),
        );
        {
            let dialog = dialog.clone();
            entry.connect_changed(move |entry| {
                match crate::domain::validation::tunnel_name(&entry.text()) {
                    Ok(_) => {
                        entry.set_tooltip_text(None);
                        dialog.set_response_enabled("rename", true);
                    }
                    Err(error) => {
                        entry.set_tooltip_text(Some(error.message()));
                        dialog.set_response_enabled("rename", false);
                    }
                }
            });
        }

        let page = self.downgrade();
        dialog.connect_response(None, move |_, response| {
            if response != "rename" {
                return;
            }
            let Some(page) = page.upgrade() else {
                return;
            };
            let name = entry.text().to_string();
            let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
                return;
            };

            let client = session.client.clone();
            let id = tunnel.id.clone();
            let state_clone = state.clone();
            let page_ref = page.downgrade();
            let cancel = Cancel::new();

            runtime::spawn(
                async move { tunnels::rename(&client, &account_id, &id, &name, &cancel).await },
                move |result| match result {
                    Ok(tunnel) => {
                        state_clone.record(
                            ActivityEvent::success(
                                ActivityCategory::Tunnel,
                                format!("Renamed a tunnel to “{}”", tunnel.name),
                            )
                            .with_target(tunnel.name.clone()),
                        );
                        if let Some(mut row) = state_clone.tunnel(&tunnel.id) {
                            row.tunnel = tunnel;
                            state_clone.update_tunnel(row);
                        }
                        if let Some(page) = page_ref.upgrade() {
                            w::toast(&page, "Renamed.");
                        }
                    }
                    Err(error) => {
                        state_clone.record(ActivityEvent::error(
                            ActivityCategory::Tunnel,
                            error.user_message(),
                        ));
                        if let Some(page) = page_ref.upgrade() {
                            w::sticky_toast(&page, &error.user_message(), None);
                        }
                    }
                },
            );
        });

        dialog.present(Some(anchor.as_ref()));
    }

    fn confirm_delete(&self, anchor: &impl IsA<gtk4::Widget>) {
        let state = self.state();
        let tunnel = self.tunnel();
        let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
            return;
        };

        let managed: Vec<String> = state
            .hosts()
            .into_iter()
            .filter(|host| host.tunnel_id.as_deref() == Some(tunnel.id.as_str()))
            .map(|host| host.name)
            .collect();

        let client = session.client.clone();
        let anchor_ref = anchor.as_ref().downgrade();
        let page = self.downgrade();
        let cancel = Cancel::new();
        let tunnel_for_impact = tunnel.clone();

        // The confirmation is worth a round trip: "this will break two
        // hostnames and leave their DNS records pointing at nothing" is a
        // different decision from "delete an unused tunnel".
        runtime::spawn(
            async move {
                tunnels::delete_impact(&client, &account_id, &tunnel_for_impact, &managed, &cancel)
                    .await
            },
            move |impact| {
                let (Some(anchor), Some(page)) = (anchor_ref.upgrade(), page.upgrade()) else {
                    return;
                };

                let page_ref = page.downgrade();
                w::confirm_destructive(
                    &anchor,
                    &format!("Delete “{}”?", tunnel.name),
                    "Deleting a tunnel cannot be undone from here. Its DNS records are \
                     not removed — unpublish its hostnames first if you want them gone.",
                    &impact.warnings(),
                    "Delete Tunnel",
                    move || {
                        if let Some(page) = page_ref.upgrade() {
                            page.delete_now();
                        }
                    },
                );
            },
        );
    }

    fn delete_now(&self) {
        let state = self.state();
        let tunnel = self.tunnel();
        let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
            return;
        };

        let client = session.client.clone();
        let id = tunnel.id.clone();
        let name = tunnel.name.clone();
        let store = state.store();
        let state_clone = state.clone();
        let page = self.downgrade();
        let cancel = Cancel::new();

        runtime::spawn(
            async move { tunnels::delete(&client, &account_id, &id, &cancel).await },
            move |result| match result {
                Ok(()) => {
                    state_clone.record(
                        ActivityEvent::success(
                            ActivityCategory::Tunnel,
                            format!("Deleted the tunnel “{name}”"),
                        )
                        .with_target(name.clone()),
                    );
                    state_clone.remove_tunnel(&tunnel.id);

                    // The machines still exist and may serve another tunnel
                    // tomorrow, so they are detached rather than deleted.
                    let tunnel_id = tunnel.id.clone();
                    let after = state_clone.clone();
                    runtime::spawn_blocking(
                        move || {
                            store.with(|connection| {
                                storage::hosts::detach_tunnel(connection, &tunnel_id)
                            })
                        },
                        move |_| crate::ui::hosts_page::refresh_hosts(&after),
                    );

                    if let Some(page) = page.upgrade() {
                        w::toast(&page, &format!("Deleted “{name}”."));
                        if let Some(root) = page.root().and_downcast::<gtk4::Window>() {
                            let _ = root.activate_action("win.page", Some(&"tunnels".to_variant()));
                        }
                    }
                }
                Err(error) => {
                    state_clone.record(ActivityEvent::error(
                        ActivityCategory::Tunnel,
                        error.user_message(),
                    ));
                    if let Some(page) = page.upgrade() {
                        w::sticky_toast(&page, &error.user_message(), None);
                    }
                }
            },
        );
    }

    fn confirm_deregister(
        &self,
        anchor: &gtk4::Button,
        connector_id: &str,
        other_connectors: usize,
    ) {
        let state = self.state();
        let tunnel = self.tunnel();
        let connector_id = connector_id.to_string();
        let page = self.downgrade();

        let body = if other_connectors == 0 {
            format!(
                "This is the only connector registered for “{}”. Deregistering it takes \
                 the tunnel down until something serves it again. The cloudflared \
                 process is not stopped and will reconnect on its own.",
                tunnel.name
            )
        } else {
            format!(
                "{} will still be serving “{}”. The cloudflared process on this machine \
                 is not stopped and will reconnect on its own.",
                w::plural(other_connectors, "connector", "connectors"),
                tunnel.name
            )
        };

        w::confirm_destructive(
            anchor,
            "Deregister this connector?",
            &body,
            &[],
            "Deregister",
            move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let (Some(session), Some(account_id)) = (state.session(), state.account_id())
                else {
                    return;
                };

                let client = session.client.clone();
                let tunnel_id = tunnel.id.clone();
                let connector_id = connector_id.clone();
                let state_clone = state.clone();
                let page_ref = page.downgrade();
                let cancel = Cancel::new();

                runtime::spawn(
                    async move {
                        tunnels::disconnect_connector(
                            &client,
                            &account_id,
                            &tunnel_id,
                            &connector_id,
                            &cancel,
                        )
                        .await
                    },
                    move |result| {
                        match result {
                            Ok(()) => state_clone.record(ActivityEvent::success(
                                ActivityCategory::Tunnel,
                                "Deregistered a connector",
                            )),
                            Err(error) => state_clone.record(ActivityEvent::error(
                                ActivityCategory::Tunnel,
                                error.user_message(),
                            )),
                        }
                        if let Some(page) = page_ref.upgrade() {
                            page.load_connectors();
                        }
                    },
                );
            },
        );
    }

    fn confirm_unpublish(&self, anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
        let tunnel = self.tunnel();
        let hostname = hostname.clone();
        let page = self.downgrade();

        let mut warnings = vec![format!(
            "The ingress rule for {} will be removed. Every other rule on this tunnel is \
             left exactly as it is.",
            hostname.display_name()
        )];
        match &hostname.dns {
            crate::domain::hostname::DnsState::Correct { .. }
            | crate::domain::hostname::DnsState::NotProxied { .. } => warnings.push(
                "The CNAME pointing at this tunnel will be removed. No other DNS record \
                 for this hostname will be touched."
                    .into(),
            ),
            _ => warnings.push(
                "No DNS record pointing at this tunnel was found, so none will be \
                 removed."
                    .into(),
            ),
        }

        w::confirm_destructive(
            anchor,
            &format!("Unpublish {}?", hostname.display_name()),
            "The hostname will stop being served by this tunnel.",
            &warnings,
            "Unpublish",
            move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let (Some(session), Some(account_id)) = (state.session(), state.account_id())
                else {
                    return;
                };

                let client = session.client.clone();
                let tunnel_id = tunnel.id.clone();
                let host = hostname.hostname.clone();
                let path = hostname.path.clone();
                let zone_id = hostname.zone_id.clone();
                let state_clone = state.clone();
                let page_ref = page.downgrade();
                let cancel = Cancel::new();
                let forget_tunnel_id = tunnel_id.clone();
                let forget_host = host.clone();
                let forget_path = path.clone();

                runtime::spawn(
                    async move {
                        hostnames::unpublish(
                            &client,
                            &account_id,
                            &tunnel_id,
                            &host,
                            path.as_deref(),
                            zone_id.as_deref(),
                            &cancel,
                        )
                        .await
                    },
                    move |outcome| {
                        record_outcome(&state_clone, &outcome, ActivityCategory::TunnelConfig);
                        if outcome.succeeded() {
                            state_clone.forget_disabled_origin(
                                &forget_tunnel_id,
                                &forget_host,
                                forget_path.as_deref(),
                                || {},
                            );
                        }
                        if let Some(page) = page_ref.upgrade() {
                            crate::ui::hostname_dialog::show_outcome(&page, &outcome);
                            page.load_hostnames();
                        }
                    },
                );
            },
        );
    }

    /// Disables a hostname on this tunnel: the rule stays, its service becomes
    /// 404, and the origin it served is remembered for Enable.
    fn confirm_disable_hostname(&self, anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
        let tunnel = self.tunnel();
        let hostname = hostname.clone();
        let page = self.downgrade();

        w::confirm(
            anchor,
            &format!("Disable {}?", hostname.display_name()),
            &format!(
                "This tunnel will answer 404 for “{}” until you enable it again. The \
                 ingress rule stays in place, the origin it serves is remembered, and \
                 no DNS record is touched.",
                hostname.display_name()
            ),
            "Disable",
            move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let (Some(session), Some(account_id)) = (state.session(), state.account_id())
                else {
                    return;
                };
                let client = session.client.clone();
                let state_for_call = state.clone();
                let hostname_for_call = hostname.clone();
                let page = page.downgrade();
                let tunnel_id = tunnel.id.clone();
                let host = hostname_for_call.hostname.clone();
                let path = hostname_for_call.path.clone();
                let service = hostname_for_call.raw_service.clone();

                // The confirm closure is `Fn`, so the callback below owns its
                // own clones rather than moving anything out of it.
                let cb_state = state.clone();
                let cb_hostname = hostname_for_call.clone();
                let cb_client = client.clone();
                let cb_account_id = account_id.clone();
                let cb_page = page.clone();
                let cb_tunnel_id = tunnel_id.clone();

                state_for_call.remember_disabled_origin(
                    &tunnel_id,
                    &host,
                    path.as_deref(),
                    &service,
                    move |remembered| {
                        let Some(page) = cb_page.upgrade() else {
                            return;
                        };
                        if !remembered {
                            w::toast(
                                &page,
                                "The origin could not be remembered, so the hostname was \
                                 not disabled.",
                            );
                            return;
                        }

                        let state = cb_state.clone();
                        let hostname = cb_hostname.clone();
                        let client = cb_client.clone();
                        let account_id = cb_account_id.clone();
                        let page = page.downgrade();
                        let cancel = Cancel::new();

                        runtime::spawn(
                            async move {
                                hostnames::disable_hostname(
                                    &client,
                                    &account_id,
                                    &cb_tunnel_id,
                                    &hostname.hostname,
                                    hostname.path.as_deref(),
                                    &cancel,
                                )
                                .await
                            },
                            move |outcome| {
                                record_outcome(&state, &outcome, ActivityCategory::TunnelConfig);
                                if let Some(page) = page.upgrade() {
                                    crate::ui::hostname_dialog::show_outcome(&page, &outcome);
                                    page.load_hostnames();
                                }
                            },
                        );
                    },
                );
            },
        );
    }

    /// Re-enables a disabled hostname on this tunnel.
    fn enable_hostname(&self, _anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
        let tunnel = self.tunnel();
        let hostname = hostname.clone();
        let page = self.downgrade();

        let Some(session) = state.session() else {
            return;
        };
        let Some(account_id) = state.account_id() else {
            return;
        };
        let client = session.client.clone();
        let state_for_call = state.clone();
        let hostname_for_call = hostname.clone();
        let tunnel_id = tunnel.id.clone();
        let host = hostname_for_call.hostname.clone();
        let path = hostname_for_call.path.clone();
        let cb_page = page.clone();
        let cb_state = state.clone();
        let cb_client = client.clone();
        let cb_account_id = account_id.clone();
        let cb_tunnel_id = tunnel_id.clone();
        let cb_host = host.clone();
        let cb_path = path.clone();

        state_for_call.remembered_disabled_origin(
            &tunnel_id,
            &host,
            path.as_deref(),
            move |original| {
                let Some(page) = cb_page.upgrade() else {
                    return;
                };
                let state = cb_state.clone();
                let client = cb_client.clone();
                let account_id = cb_account_id.clone();
                let tunnel_id = cb_tunnel_id.clone();
                let host = cb_host.clone();
                let path = cb_path.clone();
                let page = page.downgrade();
                let cancel = Cancel::new();
                let forget_tunnel_id = tunnel_id.clone();
                let forget_host = host.clone();
                let forget_path = path.clone();

                runtime::spawn(
                    async move {
                        hostnames::enable_hostname(
                            &client,
                            &account_id,
                            &tunnel_id,
                            &host,
                            path.as_deref(),
                            original,
                            &cancel,
                        )
                        .await
                    },
                    move |outcome| {
                        record_outcome(&state, &outcome, ActivityCategory::TunnelConfig);
                        if outcome.succeeded() {
                            state.forget_disabled_origin(
                                &forget_tunnel_id,
                                &forget_host,
                                forget_path.as_deref(),
                                || {},
                            );
                        }
                        if let Some(page) = page.upgrade() {
                            crate::ui::hostname_dialog::show_outcome(&page, &outcome);
                            page.load_hostnames();
                        }
                    },
                );
            },
        );
    }

    fn repair_dns(&self, anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
        let tunnel = self.tunnel();
        let Some(session) = state.session() else {
            return;
        };
        let Some(zone_id) = hostname.zone_id.clone() else {
            w::inform(
                anchor,
                "No zone",
                "This hostname is not in a zone on the connected account, so its DNS \
                 record has to be repaired wherever that domain is managed.",
            );
            return;
        };

        let client = session.client.clone();
        let host = hostname.hostname.clone();
        let tunnel_id = tunnel.id.clone();
        let state_clone = state.clone();
        let page = self.downgrade();
        let cancel = Cancel::new();

        runtime::spawn(
            async move { hostnames::repair_dns(&client, &zone_id, &tunnel_id, &host, &cancel).await },
            move |outcome| {
                record_outcome(&state_clone, &outcome, ActivityCategory::Dns);
                if let Some(page) = page.upgrade() {
                    crate::ui::hostname_dialog::show_outcome(&page, &outcome);
                    page.load_hostnames();
                }
            },
        );
    }

    fn copy_run_token(&self, anchor: &impl IsA<gtk4::Widget>) {
        let state = self.state();
        let tunnel = self.tunnel();
        copy_tunnel_run_token(anchor, &state, &tunnel.id);
    }
}

/// Fetches a tunnel's run token and copies it to the clipboard.
///
/// A free function rather than a method so the reinstall guide can offer the
/// same button without owning a detail page. The token is only ever placed on
/// the clipboard — never into a label, a log line, or an on-screen command.
pub fn copy_tunnel_run_token(anchor: &impl IsA<gtk4::Widget>, state: &AppState, tunnel_id: &str) {
    let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
        return;
    };

    let client = session.client.clone();
    let id = tunnel_id.to_string();
    let anchor_ref = anchor.as_ref().downgrade();
    let state_clone = state.clone();
    let cancel = Cancel::new();

    runtime::spawn(
        async move { tunnels::fetch_run_token(&client, &account_id, &id, &cancel).await },
        move |result| {
            let Some(anchor) = anchor_ref.upgrade() else {
                return;
            };
            match result {
                Ok(token) => {
                    use secrecy::ExposeSecret;
                    anchor.clipboard().set_text(token.expose_secret());
                    // The event says a token was copied; it does not say
                    // which, and the token never reaches the log.
                    state_clone.record(ActivityEvent::warning(
                        ActivityCategory::Tunnel,
                        "Copied a tunnel run token to the clipboard",
                    ));
                    w::sticky_toast(
                        &anchor,
                        "Run token copied. Treat it like a password — anyone holding \
                         it can serve this tunnel. Clear your clipboard afterwards.",
                        None,
                    );
                }
                Err(error) => {
                    state_clone.record(ActivityEvent::error(
                        ActivityCategory::Tunnel,
                        error.user_message(),
                    ));
                    w::inform(&anchor, error.title(), &error.user_message());
                }
            }
        },
    );
}

fn clear(column: &gtk4::Box) {
    while let Some(child) = column.first_child() {
        column.remove(&child);
    }
}

// ── The local connector ──────────────────────────────────────────────
//
// The same operations the Remote Hosts page offers for this machine,
// rebuilt here against the detail page: inspect once on demand, restart and
// read logs, and pin the transport protocol. Each finishes by rebuilding the
// sections, which re-renders the cached inspection.

/// Runs the read-only local checks and caches the result on the page.
fn detail_check_local(page: &TunnelDetailPage) {
    let state = page.state();
    if state.is_busy("local-check") {
        return;
    }
    state.set_busy("local-check", true);
    page.rebuild_sections();

    let unit = state.settings().local_service_unit.clone();
    let page_ref = page.downgrade();
    let state_clone = state.clone();

    runtime::spawn(async move { local::inspect(&unit).await }, move |result| {
        state_clone.set_busy("local-check", false);
        match result {
            Ok(found) => {
                if let Some(page) = page_ref.upgrade() {
                    page.imp().local.replace(Some(found));
                    page.rebuild_sections();
                }
            }
            Err(error) => {
                state_clone.record(ActivityEvent::error(
                    ActivityCategory::RemoteService,
                    format!(
                        "Could not inspect this machine's connector: {}",
                        error.user_message()
                    ),
                ));
                if let Some(page) = page_ref.upgrade() {
                    page.rebuild_sections();
                }
            }
        }
    });
}

/// Restarts the local connector, confirming first: this tunnel's traffic is
/// interrupted for a few seconds while it reconnects.
fn detail_restart_local(page: &TunnelDetailPage, anchor: &gtk4::Button) {
    let state = page.state();
    let tunnel = page.tunnel();
    let others = tunnel.connector_count.saturating_sub(1);

    let page_ref = page.downgrade();
    let state_clone = state.clone();
    let confirm = move || {
        let Some(page) = page_ref.upgrade() else {
            return;
        };
        let unit = page.state().settings().local_service_unit.clone();
        let page_again = page.downgrade();
        let state_inner = state_clone.clone();

        state_inner.set_busy("local-action", true);
        page.rebuild_sections();

        runtime::spawn(
            async move { local::service_action(&unit, ServiceAction::Restart).await },
            move |result| {
                state_inner.set_busy("local-action", false);
                match result {
                    Ok(outcome) => {
                        let message = if outcome.succeeded() {
                            format!(
                                "Restart Connector — the service on this machine is now {}.",
                                outcome.state_after.label().to_lowercase()
                            )
                        } else {
                            format!(
                                "Restart Connector did not take effect: the service on this \
                                 machine is {}.",
                                outcome.state_after.label().to_lowercase()
                            )
                        };
                        state_inner.record(if outcome.succeeded() {
                            ActivityEvent::success(ActivityCategory::RemoteService, message.clone())
                        } else {
                            ActivityEvent::warning(ActivityCategory::RemoteService, message.clone())
                        });
                        if let Some(page) = page_again.upgrade() {
                            if outcome.succeeded() {
                                w::toast(&page, &message);
                            } else {
                                w::sticky_toast(&page, &message, None);
                            }
                            detail_check_local(&page);
                        }
                    }
                    Err(error) => {
                        state_inner.record(ActivityEvent::error(
                            ActivityCategory::RemoteService,
                            error.user_message(),
                        ));
                        if let Some(page) = page_again.upgrade() {
                            w::sticky_toast(&page, &error.user_message(), None);
                            page.rebuild_sections();
                        }
                    }
                }
            },
        );
    };

    match hosts::action_warning(ServiceAction::Restart, &tunnel.name, others) {
        Some(warning) => w::confirm(
            anchor,
            "Restart Connector?",
            &warning,
            "Restart Connector",
            confirm,
        ),
        // The warning always exists for a restart; this arm is unreachable
        // in practice but keeps the call total rather than partial.
        None => confirm(),
    }
}

fn detail_logs_local(page: &TunnelDetailPage, anchor: &gtk4::Button) {
    let state = page.state();
    let unit = state.settings().local_service_unit.clone();
    let unit_for_run = unit.clone();
    let anchor_ref = anchor.downgrade();
    let state_clone = state.clone();

    runtime::spawn(
        async move { local::logs(&unit_for_run, crate::remote::commands::DEFAULT_LOG_LINES).await },
        move |result| {
            let Some(anchor) = anchor_ref.upgrade() else {
                return;
            };
            match result {
                Ok(lines) => {
                    let dialog = adw::Dialog::builder()
                        .title(format!("{unit} — recent log"))
                        .content_width(720)
                        .content_height(480)
                        .build();

                    let body = gtk4::Box::builder()
                        .orientation(gtk4::Orientation::Vertical)
                        .spacing(12)
                        .margin_top(12)
                        .margin_bottom(18)
                        .margin_start(18)
                        .margin_end(18)
                        .build();
                    body.append(&w::code_block(&lines.join("\n")));

                    let toolbar = adw::ToolbarView::new();
                    toolbar.add_top_bar(&adw::HeaderBar::new());
                    toolbar.set_content(Some(&body));
                    dialog.set_child(Some(&toolbar));
                    dialog.present(Some(&anchor));
                }
                Err(error) => {
                    state_clone.record(ActivityEvent::error(
                        ActivityCategory::RemoteService,
                        error.user_message(),
                    ));
                    w::inform(&anchor, error.title(), &error.user_message());
                }
            }
        },
    );
}

/// Pins or clears the local connector's transport protocol. The change only
/// takes effect on restart, so this always confirms first.
fn detail_apply_transport(
    page: &TunnelDetailPage,
    anchor: &impl IsA<gtk4::Widget>,
    protocol: Option<TransportProtocol>,
) {
    let state = page.state();
    let tunnel = page.tunnel();
    let others = tunnel.connector_count.saturating_sub(1);

    let change = match protocol {
        Some(choice) => format!(
            "The transport override will be written to the unit's drop-in file, \
             systemd reloaded, and the service restarted — the restart is what makes \
             the switch to {} take effect, and it interrupts “{}” for a few seconds \
             while the connector reconnects.",
            choice.label(),
            tunnel.name
        ),
        None => "The transport override will be removed from the unit's drop-in \
                 file, systemd reloaded, and the service restarted so cloudflared \
                 chooses its own transport again. The restart interrupts traffic \
                 for a few seconds."
            .into(),
    };
    let warning = match hosts::action_warning(ServiceAction::Restart, &tunnel.name, others) {
        Some(traffic) => format!("{change}\n\n{traffic}"),
        None => change,
    };
    let title = match protocol {
        Some(choice) => format!("Switch this machine to {}?", choice.label()),
        None => "Return to automatic transport?".into(),
    };

    let page_ref = page.downgrade();
    let state_clone = state.clone();
    w::confirm(anchor, &title, &warning, "Restart and Switch", move || {
        let Some(page) = page_ref.upgrade() else {
            return;
        };
        let unit = page.state().settings().local_service_unit.clone();
        let page_again = page.downgrade();
        let state_inner = state_clone.clone();

        state_inner.set_busy("local-action", true);
        page.rebuild_sections();

        runtime::spawn(
            async move { local::set_transport(&unit, protocol).await },
            move |result| {
                state_inner.set_busy("local-action", false);
                match result {
                    Ok(outcome) => {
                        let message = match outcome.protocol {
                            Some(choice) => {
                                format!("This machine's connector now uses {}.", choice.label())
                            }
                            None => "The transport override is removed; cloudflared \
                                     chooses its own transport again."
                                .into(),
                        };
                        state_inner.record(ActivityEvent::success(
                            ActivityCategory::RemoteService,
                            message.clone(),
                        ));
                        if let Some(page) = page_again.upgrade() {
                            w::toast(&page, &message);
                            detail_check_local(&page);
                        }
                    }
                    Err(error) => {
                        state_inner.record(ActivityEvent::error(
                            ActivityCategory::RemoteService,
                            error.user_message(),
                        ));
                        if let Some(page) = page_again.upgrade() {
                            w::sticky_toast(&page, &error.user_message(), None);
                            page.rebuild_sections();
                        }
                    }
                }
            },
        );
    });
}

/// Turns a workflow outcome into activity entries, one per step.
pub fn record_outcome(state: &AppState, outcome: &hostnames::Outcome, category: ActivityCategory) {
    for step in &outcome.steps {
        let event = match step.status {
            hostnames::Status::Done => {
                ActivityEvent::success(category, format!("{}: {}", step.step.label(), step.detail))
            }
            hostnames::Status::Skipped => ActivityEvent::info(
                category,
                format!("{} skipped: {}", step.step.label(), step.detail),
            ),
            hostnames::Status::Failed => ActivityEvent::error(
                category,
                format!("{} failed: {}", step.step.label(), step.detail),
            ),
        };
        state.record(event.with_target(outcome.hostname.clone()));
    }
}

use crate::storage;
