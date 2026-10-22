//! Remote hosts — the optional half of the application.
//!
//! Everything here has to work when there are no hosts, and the page has to
//! say so in a way that reads as a choice rather than as a missing feature.
//! When remote control is not configured the page is a status page explaining
//! what registering a host would add and what it would not, with the manual
//! `cloudflared` instructions one click away.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::host::{RemoteHost, RemoteStatus, TransportProtocol};
use crate::remote::commands::ServiceAction;
use crate::services::local::LocalState;
use crate::services::{hosts, local, runtime};
use crate::storage;
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct HostsPage {
        pub state: RefCell<Option<AppState>>,
        pub stack: RefCell<Option<gtk4::Stack>>,
        pub list: RefCell<Option<gtk4::Box>>,
        /// The last local inspection, if one has finished. `None` means this
        /// machine has not been looked at yet — not that it has nothing.
        pub local: RefCell<Option<LocalState>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HostsPage {
        const NAME: &'static str = "TunnelManagerHostsPage";
        type Type = super::HostsPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for HostsPage {}
    impl WidgetImpl for HostsPage {}
    impl BinImpl for HostsPage {}
}

glib::wrapper! {
    pub struct HostsPage(ObjectSubclass<imp::HostsPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl HostsPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                if !matches!(event, Event::Hosts | Event::Tunnels | Event::Busy) {
                    return;
                }
                if let Some(page) = page.upgrade() {
                    page.refresh();
                }
            }
        });

        page
    }

    fn state(&self) -> AppState {
        self.imp()
            .state
            .borrow()
            .clone()
            .expect("every page is constructed with a state before it can be shown")
    }

    fn build(&self) {
        let list = gtk4::Box::builder()
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
            .child(&list)
            .build();

        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&clamp)
            .build();

        let stack = gtk4::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.add_named(&scroller, Some("content"));
        stack.add_named(&self.build_not_configured(), Some("empty"));

        self.imp().list.replace(Some(list));
        self.imp().stack.replace(Some(stack.clone()));
        self.set_child(Some(&stack));

        self.refresh();
        // The one automatic look at this machine. Afterwards it only runs
        // when asked — a refresh timer that forked systemctl every minute
        // would be rude, and a service state only changes when someone
        // changes it.
        refresh_local(self);
    }

    /// The state the application is in for most users, most of the time.
    ///
    /// Worded as an offer rather than a warning: nothing is broken, and every
    /// Cloudflare feature works exactly the same without a host.
    fn build_not_configured(&self) -> gtk4::Widget {
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(18)
            .valign(gtk4::Align::Center)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();

        let add = gtk4::Button::builder()
            .label("Set Up Remote Control")
            .action_name("win.add-host")
            .build();

        let pitch = w::inline_empty_state(
            "network-server-symbolic",
            "Remote control not configured",
            "Register a Linux host over SSH to start, stop and inspect cloudflared from \
             here. This is optional — creating tunnels, publishing hostnames and \
             everything else on the Cloudflare side works without it.",
            Some(add),
        );
        // The local connector needs no host profile, so it is offered above
        // the remote pitch rather than hidden behind it.
        column.append(&self.build_local_group());
        column.append(&pitch);

        let group = w::group("What registering a host does and does not do", "");
        for (icon, title, subtitle) in [
            (
                "object-select-symbolic",
                "It runs a fixed set of commands",
                "systemctl start, stop, restart and status on one unit, plus a bounded \
                 journal tail. There is no terminal and no way to run anything else.",
            ),
            (
                "channel-secure-symbolic",
                "It verifies the host key strictly",
                "You are shown the fingerprint on first connection and asked to trust \
                 it. A key that later changes is refused, not re-prompted.",
            ),
            (
                "dialog-warning-symbolic",
                "It never installs software",
                "cloudflared has to be installed on the host already. Privileged \
                 changes are previewed in full before anything runs.",
            ),
        ] {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle(subtitle)
                .build();
            row.add_prefix(&gtk4::Image::from_icon_name(icon));
            group.add(&row);
        }
        column.append(&group);

        let manual = adw::ButtonRow::builder()
            .title("Show Manual cloudflared Instructions")
            .start_icon_name("utilities-terminal-symbolic")
            .build();
        {
            let page = self.downgrade();
            manual.connect_activated(move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let tunnel = page
                    .state()
                    .tunnels()
                    .first()
                    .map(|row| row.tunnel.id.clone())
                    .unwrap_or_else(|| "<tunnel-id>".into());
                show_manual_instructions(row, &tunnel);
            });
        }
        let manual_group = w::group("", "");
        manual_group.add(&manual);
        column.append(&manual_group);

        let clamp = adw::Clamp::builder()
            .maximum_size(crate::ui::page::CONTENT_WIDTH)
            .tightening_threshold(crate::ui::page::CONTENT_WIDTH - 200)
            .child(&column)
            .build();

        gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&clamp)
            .build()
            .upcast()
    }

    fn refresh(&self) {
        let state = self.state();
        let (Some(list), Some(stack)) = (
            self.imp().list.borrow().clone(),
            self.imp().stack.borrow().clone(),
        ) else {
            return;
        };

        let hosts = state.hosts();
        if hosts.is_empty() {
            stack.set_visible_child_name("empty");
            return;
        }
        stack.set_visible_child_name("content");

        while let Some(child) = list.first_child() {
            list.remove(&child);
        }

        // The local connector is first because it needs nothing: no profile,
        // no key trust, no network. Everything below it is the optional
        // remote half.
        list.append(&self.build_local_group());

        let group = w::group(
            "Registered hosts",
            "Each host is a machine this application may control cloudflared on, using \
             a fixed allowlist of commands and nothing else.",
        );

        for host in &hosts {
            group.add(&self.build_host_row(host));
        }
        list.append(&group);

        list.append(&self.build_defaults_group());
    }

    fn build_host_row(&self, host: &RemoteHost) -> adw::ExpanderRow {
        let state = self.state();
        let status = state.host_status(host.id);

        let row = adw::ExpanderRow::builder()
            .title(&host.name)
            .subtitle(host.display_target())
            .build();

        row.add_prefix(&gtk4::Image::from_icon_name("network-server-symbolic"));
        row.add_suffix(&w::status_badge(
            status.icon_name(),
            &status.label(),
            status.css_class(),
        ));

        // Untrusted hosts cannot be operated on at all, and saying so on the
        // row is better than a disabled button with no explanation.
        if !host.is_trusted() {
            let warn = adw::ActionRow::builder()
                .title("Host key not trusted yet")
                .subtitle(
                    "Nothing will connect to this host until its key fingerprint has \
                     been shown to you and accepted.",
                )
                .build();
            warn.add_prefix(&gtk4::Image::from_icon_name("dialog-warning-symbolic"));

            let button = gtk4::Button::builder()
                .label("Check Host Key")
                .valign(gtk4::Align::Center)
                .css_classes(["suggested-action"])
                .build();
            {
                let page = self.downgrade();
                let host = host.clone();
                button.connect_clicked(move |_| {
                    if let Some(page) = page.upgrade() {
                        crate::ui::host_dialog::verify_host_key(&page, &page.state(), &host);
                    }
                });
            }
            warn.add_suffix(&button);
            row.add_row(&warn);
        }

        row.add_row(&w::property_row("Service unit", &host.service_unit));
        row.add_row(&w::property_row("Authentication", &host.auth.label()));
        row.add_row(&w::property_row(
            "Host key",
            host.host_key_fingerprint
                .as_deref()
                .unwrap_or("Not trusted"),
        ));
        row.add_row(&w::property_row(
            "Tunnel",
            &host
                .tunnel_id
                .as_ref()
                .and_then(|id| state.tunnel(id))
                .map(|row| row.tunnel.name)
                .unwrap_or_else(|| "Not associated".into()),
        ));
        row.add_row(&w::property_row(
            "Last contacted",
            &w::format_time(host.last_seen_at),
        ));

        if let RemoteStatus::Reachable {
            cloudflared_version: Some(version),
            ..
        } = &status
        {
            row.add_row(&w::property_row("cloudflared", version));
        }
        if let RemoteStatus::Unreachable { message } = &status {
            let detail = adw::ActionRow::builder()
                .title("Last error")
                .subtitle(message)
                .build();
            detail.add_prefix(&gtk4::Image::from_icon_name("dialog-error-symbolic"));
            row.add_row(&detail);
        }

        row.add_row(&self.build_host_actions(host, &status));
        row
    }

    fn build_host_actions(&self, host: &RemoteHost, status: &RemoteStatus) -> adw::ActionRow {
        let row = adw::ActionRow::builder().title("Actions").build();

        let wrap = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .valign(gtk4::Align::Center)
            .build();

        let test = gtk4::Button::builder().label("Test Connection").build();
        {
            let page = self.downgrade();
            let host = host.clone();
            test.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    check_host(&page, &page.state(), &host);
                }
            });
        }
        wrap.append(&test);

        // Start / Stop / Restart are offered only when preflight said they
        // would work. A button that is certain to fail is worse than no button.
        let can_control = status.can_control();
        for action in [
            ServiceAction::Start,
            ServiceAction::Stop,
            ServiceAction::Restart,
        ] {
            let button = gtk4::Button::builder()
                .label(action.label().trim_end_matches(" Connector"))
                .sensitive(can_control)
                .build();
            if !can_control {
                button.set_tooltip_text(Some(
                    "Not available until a connection test confirms this host runs \
                     systemd and this login may control the service.",
                ));
            }
            {
                let page = self.downgrade();
                let host = host.clone();
                button.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        run_service_action(&page, button, &page.state(), &host, action);
                    }
                });
            }
            wrap.append(&button);
        }

        let logs = gtk4::Button::builder()
            .label("Logs")
            .sensitive(can_control)
            .build();
        {
            let page = self.downgrade();
            let host = host.clone();
            logs.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    show_logs(&page, button, &page.state(), &host);
                }
            });
        }
        wrap.append(&logs);

        let edit = gtk4::Button::builder().label("Edit").build();
        {
            let page = self.downgrade();
            let host = host.clone();
            edit.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    crate::ui::host_dialog::present_edit(&page, &page.state(), &host);
                }
            });
        }
        wrap.append(&edit);

        let remove = gtk4::Button::builder()
            .label("Remove")
            .css_classes(["destructive-action"])
            .build();
        {
            let page = self.downgrade();
            let host = host.clone();
            remove.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    confirm_remove(&page, button, &page.state(), &host);
                }
            });
        }
        wrap.append(&remove);

        row.add_suffix(&wrap);
        row
    }

    /// The connector on this computer: the one start/stop/restart that needs
    /// no host profile, no key trust and no SSH.
    fn build_local_group(&self) -> adw::PreferencesGroup {
        let state = self.state();
        let settings = state.settings();
        let cached = self.imp().local.borrow().clone();
        let checking = state.is_busy("local-check");
        let acting = state.is_busy("local-action");

        let group = w::group(
            "This machine",
            "cloudflared on the computer running this application. No SSH profile \
             and no host key: privileged steps show the desktop's authentication \
             dialog instead.",
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
                settings.local_service_unit,
                cached
                    .as_ref()
                    .and_then(|found| found.cloudflared_version.clone())
                    .map(|version| format!("cloudflared {version}"))
                    .unwrap_or_else(|| "cloudflared not seen yet".into())
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

        let unit = adw::ActionRow::builder()
            .title("Service unit")
            .subtitle(&settings.local_service_unit)
            .build();
        let change = gtk4::Button::builder()
            .label("Change…")
            .valign(gtk4::Align::Center)
            .sensitive(!checking && !acting)
            .build();
        {
            let page = self.downgrade();
            change.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    edit_local_unit(&page, button);
                }
            });
        }
        unit.add_suffix(&change);
        group.add(&unit);

        let (tunnel_name, _) = local_tunnel_context(&state);
        let serves = adw::ActionRow::builder()
            .title("Serves tunnel")
            .subtitle(&tunnel_name)
            .build();
        let choose = gtk4::Button::builder()
            .label("Choose…")
            .valign(gtk4::Align::Center)
            .sensitive(!checking && !acting)
            .build();
        {
            let page = self.downgrade();
            choose.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    pick_local_tunnel(&page, button);
                }
            });
        }
        serves.add_suffix(&choose);
        group.add(&serves);

        group.add(&self.build_local_transport_row(&status, acting));

        // An empty-title `AdwActionRow` with the word in the prefix slot. The
        // title slot is measured against the suffix, so at 360px it used to
        // hyphenate "Actions" as "Actio-ns"; the prefix keeps the word on one
        // line while the wrap box takes the rest of the width and can wrap
        // the buttons onto a second line.
        let actions = adw::ActionRow::builder().build();
        let actions_label = gtk4::Label::builder().label("Actions").build();
        actions.add_prefix(&actions_label);
        actions.update_property(&[gtk4::accessible::Property::Label("Actions")]);
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
                    refresh_local(&page);
                }
            });
        }
        wrap.append(&check);

        let can_control = status.can_control();
        let hint = cached
            .as_ref()
            .map(|found| found.control_hint().to_string())
            .unwrap_or_default();
        for action in [
            ServiceAction::Start,
            ServiceAction::Stop,
            ServiceAction::Restart,
        ] {
            let button = gtk4::Button::builder()
                .label(action.label().trim_end_matches(" Connector"))
                .sensitive(can_control && !checking && !acting)
                .build();
            if !can_control && !hint.is_empty() {
                button.set_tooltip_text(Some(&hint));
            }
            {
                let page = self.downgrade();
                button.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        perform_local_action(&page, button, action);
                    }
                });
            }
            wrap.append(&button);
        }

        let logs = gtk4::Button::builder()
            .label("Logs")
            .sensitive(status_can_inspect(&cached) && !checking)
            .build();
        if !status_can_inspect(&cached) {
            logs.set_tooltip_text(Some(
                "Not available until systemd is found on this machine.",
            ));
        }
        {
            let page = self.downgrade();
            logs.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    show_local_logs(&page, button);
                }
            });
        }
        wrap.append(&logs);

        actions.add_suffix(&wrap);
        group.add(&actions);

        group
    }

    /// The QUIC / HTTP/2 switch for the local connector.
    fn build_local_transport_row(&self, status: &RemoteStatus, acting: bool) -> adw::ComboRow {
        let cached = self.imp().local.borrow().clone();
        let protocol = cached.as_ref().and_then(|found| found.protocol);

        let row = adw::ComboRow::builder()
            .title("Transport protocol")
            .subtitle(match protocol {
                Some(choice) => choice.explanation().to_string(),
                None => "cloudflared negotiates QUIC over UDP and falls back on its own. \
                         Pin HTTP/2 over TCP when UDP flows stall."
                    .to_string(),
            })
            .model(&gtk4::StringList::new(&[
                // The row's subtitle already explains what automatic does.
                // The value column beside the arrow is narrow; "Automatic
                // (cloudflared chooses)" and even "Automatic" were rendered
                // as "Auto…" even at the default window size.
                "Auto",
                TransportProtocol::Quic.label(),
                TransportProtocol::Http2.label(),
            ]))
            .sensitive(status.can_control() && !acting)
            .build();
        if !status.can_control() {
            row.set_tooltip_text(Some(
                "Not available until this machine can be controlled — see above.",
            ));
        }
        // Selected before the handler is connected, so rebuilding never
        // applies anything by itself.
        row.set_selected(match protocol {
            None => 0,
            Some(TransportProtocol::Quic) => 1,
            Some(TransportProtocol::Http2) => 2,
        });
        {
            let page = self.downgrade();
            row.connect_selected_notify(move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let choice = match row.selected() {
                    1 => Some(TransportProtocol::Quic),
                    2 => Some(TransportProtocol::Http2),
                    _ => None,
                };
                // Choosing what is already pinned is a no-op, not an
                // operation: it must not restart the service.
                let current = page
                    .imp()
                    .local
                    .borrow()
                    .clone()
                    .and_then(|found| found.protocol);
                if current == choice {
                    return;
                }
                apply_local_transport(&page, row, choice);
            });
        }
        row
    }

    fn build_defaults_group(&self) -> adw::PreferencesGroup {
        let group = w::group(
            "Least-privilege sudo",
            "Controlling a service needs passwordless sudo for exactly these commands. \
             Grant nothing wider.",
        );

        let state = self.state();
        let host = state.hosts().into_iter().next();
        let (username, unit) = match &host {
            Some(host) => (host.username.clone(), host.service_unit.clone()),
            None => ("deploy".to_string(), "cloudflared.service".to_string()),
        };

        match hosts::sudoers_snippet(&username, &unit) {
            Ok(snippet) => {
                let wrapper = gtk4::Box::builder()
                    .orientation(gtk4::Orientation::Vertical)
                    .margin_top(12)
                    .build();
                wrapper.append(&w::copyable_code_block(
                    &snippet,
                    "Install with: sudo visudo -f /etc/sudoers.d/tunnel-manager",
                ));
                group.add(&wrapper);
            }
            Err(error) => {
                group.add(&w::property_row(
                    "Could not build the rule",
                    error.message(),
                ));
            }
        }

        group
    }
}

/// Runs preflight against a host and records the result.
fn check_host(page: &HostsPage, state: &AppState, host: &RemoteHost) {
    if !host.is_trusted() {
        crate::ui::host_dialog::verify_host_key(page, state, host);
        return;
    }

    // One busy key per host so two hosts can be checked concurrently. The
    // leaked key is the same reference cleared on completion below — a
    // fixed key here would either collide or leak `true` forever and stall
    // the auto-refresh timer that consults `is_anything_busy`.
    let key: &'static str = Box::leak(format!("host-{}", host.id).into_boxed_str());
    if state.is_busy(key) {
        return;
    }
    state.set_busy(key, true);
    state.set_host_status(host.id, RemoteStatus::Checking);

    let controller = state.controller();
    let host_clone = host.clone();
    let state_clone = state.clone();
    let page_ref = page.downgrade();
    let store = state.store();
    let host_id = host.id;

    runtime::spawn(
        async move {
            let passphrase = runtime::blocking(move || {
                storage::secrets::load_host_passphrase(host_id)
                    .ok()
                    .flatten()
            })
            .await
            .flatten();

            hosts::preflight(&controller, &host_clone, passphrase).await
        },
        move |result| {
            state_clone.set_busy(key, false);
            let status = hosts::status_from(&result);
            state_clone.set_host_status(host_id, status);

            match &result {
                Ok(preflight) => {
                    runtime::spawn_blocking(
                        move || {
                            store.with(|connection| {
                                let _ = storage::hosts::touch(connection, host_id);
                            });
                        },
                        |()| {},
                    );

                    let event = match preflight.blocking_reason() {
                        None => {
                            ActivityEvent::success(ActivityCategory::Ssh, "Host checked and ready")
                        }
                        Some(reason) => ActivityEvent::warning(ActivityCategory::Ssh, reason),
                    };
                    state_clone.record(event);

                    for note in &preflight.notes {
                        state_clone
                            .record(ActivityEvent::warning(ActivityCategory::Ssh, note.clone()));
                    }

                    if let Some(page) = page_ref.upgrade() {
                        match preflight.blocking_reason() {
                            None => w::toast(&page, "Host is reachable and ready."),
                            Some(reason) => w::sticky_toast(&page, &reason, None),
                        }
                    }
                }
                Err(error) => {
                    state_clone.record(ActivityEvent::error(
                        ActivityCategory::Ssh,
                        error.user_message(),
                    ));
                    if let Some(page) = page_ref.upgrade() {
                        if error.is_security_refusal() {
                            w::inform(&page, error.title(), &error.user_message());
                        } else {
                            w::sticky_toast(&page, &error.user_message(), None);
                        }
                    }
                }
            }
        },
    );
}

/// Starts, stops or restarts the service, confirming first when it is
/// disruptive.
fn run_service_action(
    page: &HostsPage,
    anchor: &gtk4::Button,
    state: &AppState,
    host: &RemoteHost,
    action: ServiceAction,
) {
    let tunnel_name = host
        .tunnel_id
        .as_ref()
        .and_then(|id| state.tunnel(id))
        .map(|row| row.tunnel.name)
        .unwrap_or_else(|| "this tunnel".into());

    // How many *other* connectors are serving the tunnel decides whether
    // stopping this one takes the tunnel offline, and the confirmation says
    // which case the user is in.
    let others = host
        .tunnel_id
        .as_ref()
        .and_then(|id| state.tunnel(id))
        .map(|row| row.tunnel.connector_count.saturating_sub(1))
        .unwrap_or(0);

    let go = {
        let page = page.downgrade();
        let state = state.clone();
        let host = host.clone();
        move || {
            let Some(page) = page.upgrade() else {
                return;
            };
            perform_service_action(&page, &state, &host, action);
        }
    };

    match hosts::action_warning(action, &tunnel_name, others) {
        None => go(),
        Some(warning) => w::confirm(
            anchor,
            &format!("{}?", action.label()),
            &warning,
            action.label(),
            go,
        ),
    }
}

fn perform_service_action(
    page: &HostsPage,
    state: &AppState,
    host: &RemoteHost,
    action: ServiceAction,
) {
    let controller = state.controller();
    let host_clone = host.clone();
    let host_id = host.id;
    let state_clone = state.clone();
    let page_ref = page.downgrade();

    state.set_busy("host-action", true);

    runtime::spawn(
        async move {
            let passphrase = runtime::blocking(move || {
                storage::secrets::load_host_passphrase(host_id)
                    .ok()
                    .flatten()
            })
            .await
            .flatten();

            hosts::service_action(&controller, &host_clone, passphrase, action).await
        },
        move |result| {
            state_clone.set_busy("host-action", false);

            match result {
                Ok(outcome) => {
                    let message = if outcome.succeeded() {
                        format!(
                            "{} — the service is now {}.",
                            action.label(),
                            outcome.state_after.label().to_lowercase()
                        )
                    } else {
                        format!(
                            "{} did not take effect: the service is {}.",
                            action.label(),
                            outcome.state_after.label().to_lowercase()
                        )
                    };

                    state_clone.record(if outcome.succeeded() {
                        ActivityEvent::success(ActivityCategory::RemoteService, message.clone())
                    } else {
                        ActivityEvent::warning(ActivityCategory::RemoteService, message.clone())
                    });

                    state_clone.set_host_status(
                        host_id,
                        RemoteStatus::Reachable {
                            service: outcome.state_after.clone(),
                            cloudflared_version: None,
                            systemd_available: true,
                            can_control_service: true,
                        },
                    );

                    if let Some(page) = page_ref.upgrade() {
                        if outcome.succeeded() {
                            w::toast(&page, &message);
                        } else {
                            w::sticky_toast(&page, &message, None);
                        }
                    }
                }
                Err(error) => {
                    state_clone.record(ActivityEvent::error(
                        ActivityCategory::RemoteService,
                        error.user_message(),
                    ));
                    if let Some(page) = page_ref.upgrade() {
                        w::sticky_toast(&page, &error.user_message(), None);
                    }
                }
            }
        },
    );
}

fn show_logs(page: &HostsPage, anchor: &gtk4::Button, state: &AppState, host: &RemoteHost) {
    let controller = state.controller();
    let host_clone = host.clone();
    let host_id = host.id;
    let anchor_ref = anchor.downgrade();
    let unit = host.service_unit.clone();
    let state_clone = state.clone();
    let _ = page;

    runtime::spawn(
        async move {
            let passphrase = runtime::blocking(move || {
                storage::secrets::load_host_passphrase(host_id)
                    .ok()
                    .flatten()
            })
            .await
            .flatten();

            hosts::logs(
                &controller,
                &host_clone,
                passphrase,
                crate::remote::commands::DEFAULT_LOG_LINES,
            )
            .await
        },
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

                    let note = gtk4::Label::builder()
                        .label(format!(
                            "The last {} lines, redacted before display.",
                            lines.len()
                        ))
                        .halign(gtk4::Align::Start)
                        .wrap(true)
                        .build();
                    note.add_css_class("caption");
                    note.add_css_class("dimmed");
                    body.append(&note);

                    let text = if lines.is_empty() {
                        "The journal returned nothing for this unit.".to_string()
                    } else {
                        lines.join("\n")
                    };
                    body.append(&w::code_block(&text));

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

fn confirm_remove(page: &HostsPage, anchor: &gtk4::Button, state: &AppState, host: &RemoteHost) {
    let state = state.clone();
    let host = host.clone();
    let page = page.downgrade();

    w::confirm_destructive(
        anchor,
        &format!("Remove “{}”?", host.name),
        "This removes the saved profile and its trusted host key from this computer. \
         Nothing on the host itself is changed: cloudflared keeps running, and the \
         tunnel keeps serving traffic.",
        &[],
        "Remove",
        move || {
            let store = state.store();
            let host_id = host.id;
            let name = host.name.clone();
            let state = state.clone();
            let page = page.clone();

            runtime::spawn_blocking(
                move || {
                    // Both halves: the row, and the passphrase filed under the
                    // row's ID in the credential store.
                    let passphrase_result = storage::secrets::delete_host_passphrase(host_id);
                    let row_result =
                        store.with(|connection| storage::hosts::delete(connection, host_id));
                    (passphrase_result, row_result)
                },
                move |(passphrase_result, row_result)| {
                    if let Err(error) = &passphrase_result {
                        state.record(ActivityEvent::warning(
                            ActivityCategory::App,
                            format!(
                                "Removed the host profile “{name}” but its saved passphrase could not be deleted: {}",
                                error.user_message()
                            ),
                        ));
                    }
                    match &row_result {
                        Some(Err(error)) => {
                            state.record(ActivityEvent::error(
                                ActivityCategory::App,
                                format!(
                                    "Could not remove the host profile “{name}”: {}",
                                    error.user_message()
                                ),
                            ));
                            return;
                        }
                        None => {
                            state.record(ActivityEvent::error(
                                ActivityCategory::App,
                                format!(
                                    "Could not remove the host profile “{name}”: the local database is unavailable."
                                ),
                            ));
                            return;
                        }
                        Some(Ok(())) => {}
                    }
                    state.record(ActivityEvent::info(
                        ActivityCategory::App,
                        format!("Removed the host profile “{name}”"),
                    ));
                    refresh_hosts(&state);
                    if let Some(page) = page.upgrade() {
                        w::toast(&page, &format!("Removed “{name}”."));
                    }
                },
            );
        },
    );
}

/// Whether the cached inspection says anything at all can be read locally.
fn status_can_inspect(cached: &Option<LocalState>) -> bool {
    cached.as_ref().is_some_and(|found| found.can_inspect())
}

/// The tunnel the local connector is associated with, and how many *other*
/// connectors serve it — the two facts the restart warning is built from.
fn local_tunnel_context(state: &AppState) -> (String, usize) {
    match state
        .settings()
        .local_tunnel_id
        .as_deref()
        .and_then(|id| state.tunnel(id))
    {
        Some(row) => (
            row.tunnel.name.clone(),
            row.tunnel.connector_count.saturating_sub(1),
        ),
        None => ("Not associated".into(), 0),
    }
}

/// Runs the read-only local checks and caches the result on the page.
fn refresh_local(page: &HostsPage) {
    let state = page.state();
    if state.is_busy("local-check") {
        return;
    }
    state.set_busy("local-check", true);
    page.refresh();

    let unit = state.settings().local_service_unit.clone();
    let page_ref = page.downgrade();
    let state_clone = state.clone();

    runtime::spawn(async move { local::inspect(&unit).await }, move |result| {
        state_clone.set_busy("local-check", false);
        match result {
            Ok(found) => {
                if let Some(page) = page_ref.upgrade() {
                    page.imp().local.replace(Some(found));
                    page.refresh();
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
                    page.refresh();
                }
            }
        }
    });
}

/// Starts, stops or restarts the local connector, confirming first when it
/// is disruptive.
fn perform_local_action(page: &HostsPage, anchor: &gtk4::Button, action: ServiceAction) {
    let state = page.state();
    let (tunnel_name, others) = local_tunnel_context(&state);

    let go = {
        let page = page.downgrade();
        let state = state.clone();
        move || {
            let Some(page) = page.upgrade() else {
                return;
            };
            let unit = state.settings().local_service_unit.clone();
            let page_ref = page.downgrade();
            let state_clone = state.clone();

            state.set_busy("local-action", true);
            page.refresh();

            runtime::spawn(
                async move { local::service_action(&unit, action).await },
                move |result| {
                    state_clone.set_busy("local-action", false);
                    match result {
                        Ok(outcome) => {
                            let message = if outcome.succeeded() {
                                format!(
                                    "{} — the service on this machine is now {}.",
                                    action.label(),
                                    outcome.state_after.label().to_lowercase()
                                )
                            } else {
                                format!(
                                    "{} did not take effect: the service on this machine is {}.",
                                    action.label(),
                                    outcome.state_after.label().to_lowercase()
                                )
                            };
                            state_clone.record(if outcome.succeeded() {
                                ActivityEvent::success(
                                    ActivityCategory::RemoteService,
                                    message.clone(),
                                )
                            } else {
                                ActivityEvent::warning(
                                    ActivityCategory::RemoteService,
                                    message.clone(),
                                )
                            });
                            if let Some(page) = page_ref.upgrade() {
                                if outcome.succeeded() {
                                    w::toast(&page, &message);
                                } else {
                                    w::sticky_toast(&page, &message, None);
                                }
                                refresh_local(&page);
                            }
                        }
                        Err(error) => {
                            state_clone.record(ActivityEvent::error(
                                ActivityCategory::RemoteService,
                                error.user_message(),
                            ));
                            if let Some(page) = page_ref.upgrade() {
                                if error.is_security_refusal() {
                                    w::inform(&page, error.title(), &error.user_message());
                                } else {
                                    w::sticky_toast(&page, &error.user_message(), None);
                                }
                                page.refresh();
                            }
                        }
                    }
                },
            );
        }
    };

    match hosts::action_warning(action, &tunnel_name, others) {
        None => go(),
        Some(warning) => w::confirm(
            anchor,
            &format!("{}?", action.label()),
            &warning,
            action.label(),
            go,
        ),
    }
}

fn show_local_logs(page: &HostsPage, anchor: &gtk4::Button) {
    let state = page.state();
    let unit = state.settings().local_service_unit.clone();
    let anchor_ref = anchor.downgrade();
    let state_clone = state.clone();
    let unit_for_run = unit.clone();

    runtime::spawn(
        async move { local::logs(&unit_for_run, crate::remote::commands::DEFAULT_LOG_LINES).await },
        move |result| {
            let Some(anchor) = anchor_ref.upgrade() else {
                return;
            };
            match result {
                Ok(lines) => present_local_logs(&anchor, &unit, lines),
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

fn present_local_logs(anchor: &gtk4::Button, unit: &str, lines: Vec<String>) {
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

    let note = gtk4::Label::builder()
        .label(format!(
            "The last {} lines from this machine, redacted before display.",
            lines.len()
        ))
        .halign(gtk4::Align::Start)
        .wrap(true)
        .build();
    note.add_css_class("caption");
    note.add_css_class("dimmed");
    body.append(&note);

    let text = if lines.is_empty() {
        "The journal returned nothing for this unit.".to_string()
    } else {
        lines.join("\n")
    };
    body.append(&w::code_block(&text));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&body));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(anchor));
}

/// Pins or clears the local connector's transport protocol.
///
/// The change only takes effect on restart, and the restart interrupts
/// traffic — so this always confirms first, and the confirmation names the
/// drop-in file that is about to be written or removed.
fn apply_local_transport(
    page: &HostsPage,
    anchor: &impl IsA<gtk4::Widget>,
    protocol: Option<TransportProtocol>,
) {
    let state = page.state();
    let (tunnel_name, others) = local_tunnel_context(&state);
    let unit = state.settings().local_service_unit.clone();

    let change = match protocol {
        Some(choice) => format!(
            "The transport override will be written to the unit's drop-in file, \
             systemd reloaded, and the service restarted — the restart is what makes \
             the switch to {} take effect, and it interrupts traffic for a few \
             seconds while the connector reconnects.",
            choice.label()
        ),
        None => "The transport override will be removed from the unit's drop-in \
                 file, systemd reloaded, and the service restarted so cloudflared \
                 chooses its own transport again. The restart interrupts traffic \
                 for a few seconds."
            .into(),
    };
    let warning = match hosts::action_warning(ServiceAction::Restart, &tunnel_name, others) {
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
        let unit = unit.clone();
        let page_again = page.downgrade();
        let state_inner = state_clone.clone();

        state_inner.set_busy("local-action", true);
        page.refresh();

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
                            refresh_local(&page);
                        }
                    }
                    Err(error) => {
                        state_inner.record(ActivityEvent::error(
                            ActivityCategory::RemoteService,
                            error.user_message(),
                        ));
                        if let Some(page) = page_again.upgrade() {
                            if error.is_security_refusal() {
                                w::inform(&page, error.title(), &error.user_message());
                            } else {
                                w::sticky_toast(&page, &error.user_message(), None);
                            }
                            page.refresh();
                        }
                    }
                }
            },
        );
    });
}

/// Changes which unit the local card controls.
fn edit_local_unit(page: &HostsPage, anchor: &impl IsA<gtk4::Widget>) {
    let state = page.state();
    let entry = adw::EntryRow::builder()
        .title("Service unit")
        .text(&state.settings().local_service_unit)
        .build();

    let dialog = adw::AlertDialog::new(
        Some("Local service unit"),
        Some(
            "The systemd unit cloudflared runs as on this machine. Usually \
             cloudflared.service — the name cloudflared's own installer creates.",
        ),
    );
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let page_ref = page.downgrade();
    let state_clone = state.clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "save" {
            return;
        }
        let Some(page) = page_ref.upgrade() else {
            return;
        };
        match crate::domain::validation::systemd_unit(&entry.text()) {
            Ok(unit) => {
                state_clone.update_settings(|settings| settings.local_service_unit = unit);
                dialog.close();
                refresh_local(&page);
            }
            Err(error) => {
                entry.add_css_class("error");
                entry.set_tooltip_text(Some(error.message()));
            }
        }
    });

    dialog.present(Some(anchor.as_ref()));
}

/// Associates the local connector with a tunnel, so restart warnings know
/// whose traffic is at stake.
fn pick_local_tunnel(page: &HostsPage, anchor: &impl IsA<gtk4::Widget>) {
    let state = page.state();
    let tunnels = state.tunnels();
    let current = state.settings().local_tunnel_id.clone();

    let names = gtk4::StringList::new(&["None — not associated"]);
    let mut ids: Vec<Option<String>> = vec![None];
    for row in &tunnels {
        names.append(&row.tunnel.name);
        ids.push(Some(row.tunnel.id.clone()));
    }

    let combo = adw::ComboRow::builder()
        .title("Tunnel")
        .subtitle("The tunnel whose traffic the restart warnings should describe")
        .model(&names)
        .build();
    let selected = ids.iter().position(|id| *id == current).unwrap_or(0) as u32;
    combo.set_selected(selected);

    let dialog = adw::AlertDialog::new(
        Some("Local connector serves"),
        Some(
            "Which tunnel this machine's connector runs. Used for the restart \
             warning and for the tunnel detail page's local controls.",
        ),
    );
    let picker_group = w::group("", "");
    picker_group.add(&combo);
    dialog.set_extra_child(Some(&picker_group));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("save", "Save");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let page_ref = page.downgrade();
    let state_clone = state.clone();
    dialog.connect_response(None, move |dialog, response| {
        if response != "save" {
            return;
        }
        let Some(page) = page_ref.upgrade() else {
            return;
        };
        let chosen = ids.get(combo.selected() as usize).cloned().flatten();
        state_clone.update_settings(|settings| settings.local_tunnel_id = chosen);
        dialog.close();
        page.refresh();
    });

    dialog.present(Some(anchor.as_ref()));
}

fn show_manual_instructions(anchor: &impl IsA<gtk4::Widget>, tunnel_id: &str) {
    let dialog = adw::Dialog::builder()
        .title("Run cloudflared yourself")
        .content_width(680)
        .content_height(520)
        .build();

    let body = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();

    body.append(&w::copyable_code_block(
        &hosts::manual_setup_instructions(tunnel_id),
        "These steps need no SSH access from this application.",
    ));

    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .child(&body)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&scroller));
    dialog.set_child(Some(&toolbar));
    dialog.present(Some(anchor.as_ref()));
}

/// Reloads host profiles from the database into the shared state.
pub fn refresh_hosts(state: &AppState) {
    let store = state.store();
    let state = state.clone();

    runtime::spawn_blocking(
        move || {
            store
                .with(|connection| storage::hosts::list(connection).unwrap_or_default())
                .unwrap_or_default()
        },
        move |hosts| state.set_hosts(hosts),
    );
}
