//! Public hostnames — every address the account's tunnels serve.
//!
//! A published hostname is two facts in two places: an ingress rule on a
//! tunnel, and a proxied CNAME in a zone. This page looks across all the
//! account's remotely-managed tunnels at once, which is the only vantage
//! point from which "three hostnames have no DNS record" is visible at all.
//! The zones those records live in are on their own page.
//!
//! The list is assembled by one sweep: a configuration read per tunnel, each
//! joined with the zone's DNS by the hostnames service, accumulated as the
//! requests land and re-sorted so a refresh never reshuffles rows under the
//! pointer.

use std::cell::{Cell, RefCell};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::cloudflare::{Cancel, CloudflareError};
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::hostname::{DnsState, PublicHostname};
use crate::domain::tunnel::Tunnel;
use crate::services::{hostnames, runtime};
use crate::ui::state::{AppState, Event};
use crate::ui::tunnel_detail_page::record_outcome;
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct HostnamesPage {
        pub state: RefCell<Option<AppState>>,
        pub stack: RefCell<Option<gtk4::Stack>>,
        pub content: RefCell<Option<gtk4::Box>>,
        /// Every public hostname found across every tunnel, accumulated as the
        /// per-tunnel requests land.
        pub hostnames: RefCell<Vec<PublicHostname>>,
        /// Tunnels whose configuration could not be read, and why. Shown
        /// rather than swallowed: a half-complete list that looks complete is
        /// worse than an error.
        pub failures: RefCell<Vec<(String, String)>>,
        pub pending: Cell<usize>,
        /// Bumped on every reload so a result from a superseded sweep is
        /// discarded instead of being merged into the new one.
        pub generation: Cell<u64>,
        pub cancel: RefCell<Option<Cancel>>,
        /// The inputs the last sweep was started from. See
        /// [`super::HostnamesPage::inputs_signature`].
        pub signature: RefCell<Vec<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for HostnamesPage {
        const NAME: &'static str = "TunnelManagerHostnamesPage";
        type Type = super::HostnamesPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for HostnamesPage {}
    impl WidgetImpl for HostnamesPage {}
    impl BinImpl for HostnamesPage {}
}

glib::wrapper! {
    pub struct HostnamesPage(ObjectSubclass<imp::HostnamesPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl HostnamesPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                if !matches!(event, Event::Tunnels | Event::Zones | Event::Busy) {
                    return;
                }
                let Some(page) = page.upgrade() else {
                    return;
                };
                // Only a change to the tunnels or the zones can change what a
                // sweep would find; anything else just redraws.
                if page.inputs_signature() == *page.imp().signature.borrow() {
                    page.refresh();
                } else {
                    page.reload();
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
        let content = gtk4::Box::builder()
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
            .child(&content)
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
        stack.add_named(
            &w::loading_state("Reading each tunnel's configuration…"),
            Some("loading"),
        );
        stack.add_named(&scroller, Some("content"));
        stack.add_named(&self.build_disconnected_state(), Some("empty"));

        self.imp().content.replace(Some(content));
        self.imp().stack.replace(Some(stack.clone()));
        self.set_child(Some(&stack));

        self.reload();
    }

    fn build_disconnected_state(&self) -> adw::StatusPage {
        w::empty_state(
            "network-offline-symbolic",
            "Not connected",
            "Connect a Cloudflare API token to see the hostnames the tunnels serve.",
            None,
        )
    }

    // ── Loading ──────────────────────────────────────────────────────────

    /// The identity of everything a sweep reads.
    ///
    /// `Event::Tunnels` fires once per tunnel while the hostname counts are
    /// filled in one request at a time, so reloading on the event itself would
    /// start a whole sweep per tunnel. Comparing this instead means a sweep
    /// starts only when a tunnel or a zone actually appeared, vanished or was
    /// renamed.
    fn inputs_signature(&self) -> Vec<String> {
        let state = self.state();
        let mut signature: Vec<String> = state
            .tunnels()
            .iter()
            .filter(|row| row.tunnel.config_source.is_remotely_managed())
            .map(|row| format!("t:{}:{}", row.tunnel.id, row.tunnel.name))
            .collect();
        signature.extend(state.zones().iter().map(|zone| format!("z:{}", zone.id)));
        signature
    }

    /// Starts a fresh sweep: one configuration read per remotely-managed
    /// tunnel, each joined with the zone's DNS by the hostnames service.
    fn reload(&self) {
        let imp = self.imp();
        let state = self.state();

        imp.signature.replace(self.inputs_signature());

        // Anything still in flight belongs to the previous sweep. Cancelling
        // it stops the requests as well as discarding their results.
        let cancel = Cancel::new();
        if let Some(previous) = imp.cancel.replace(Some(cancel.clone())) {
            previous.cancel();
        }
        let generation = imp.generation.get().wrapping_add(1);
        imp.generation.set(generation);

        imp.hostnames.replace(Vec::new());
        imp.failures.replace(Vec::new());
        imp.pending.set(0);

        let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
            self.refresh();
            return;
        };

        let managed: Vec<Tunnel> = state
            .tunnels()
            .into_iter()
            .map(|row| row.tunnel)
            .filter(|tunnel| tunnel.config_source.is_remotely_managed())
            .collect();

        imp.pending.set(managed.len());
        self.refresh();

        for tunnel in managed {
            let client = session.client.clone();
            let zones = session.zones.clone();
            let account_id = account_id.clone();
            let cancel = cancel.clone();
            let id = tunnel.id.clone();
            let name = tunnel.name.clone();
            let reported = tunnel.name.clone();
            let page = self.downgrade();

            runtime::spawn(
                async move {
                    hostnames::load_hostnames(&client, &account_id, &id, &name, &zones, &cancel)
                        .await
                },
                move |result| {
                    if let Some(page) = page.upgrade() {
                        page.absorb(generation, &reported, result);
                    }
                },
            );
        }
    }

    /// Merges one tunnel's answer into the accumulated list.
    fn absorb(
        &self,
        generation: u64,
        tunnel_name: &str,
        result: Result<Vec<PublicHostname>, CloudflareError>,
    ) {
        let imp = self.imp();
        if imp.generation.get() != generation {
            return;
        }
        imp.pending.set(imp.pending.get().saturating_sub(1));

        match result {
            Ok(found) => {
                imp.hostnames.borrow_mut().extend(found);
                // The requests finish in whatever order the network decides,
                // so the list is sorted rather than left in arrival order —
                // otherwise a refresh reshuffles rows under the pointer.
                imp.hostnames.borrow_mut().sort_by(|left, right| {
                    left.display_name()
                        .cmp(&right.display_name())
                        .then_with(|| left.tunnel_name.cmp(&right.tunnel_name))
                });
            }
            Err(error) if error.is_cancelled() => {}
            Err(error) => {
                let message = error.user_message();
                imp.failures
                    .borrow_mut()
                    .push((tunnel_name.to_string(), message.clone()));
                self.state().record(
                    ActivityEvent::error(
                        ActivityCategory::Api,
                        format!("Could not read the configuration of “{tunnel_name}”"),
                    )
                    .with_detail(message),
                );
            }
        }

        self.refresh();
    }

    // ── Rendering ────────────────────────────────────────────────────────

    fn refresh(&self) {
        let state = self.state();
        let imp = self.imp();
        let (Some(content), Some(stack)) =
            (imp.content.borrow().clone(), imp.stack.borrow().clone())
        else {
            return;
        };

        if state.session().is_none() {
            stack.set_visible_child_name("empty");
            return;
        }

        let nothing_yet = state.zones().is_empty()
            && imp.hostnames.borrow().is_empty()
            && imp.failures.borrow().is_empty();
        let working = imp.pending.get() > 0 || state.is_busy("zones") || state.is_busy("tunnels");

        if nothing_yet && working {
            stack.set_visible_child_name("loading");
            return;
        }
        stack.set_visible_child_name("content");

        while let Some(child) = content.first_child() {
            content.remove(&child);
        }

        content.append(&self.build_hostnames());
    }

    fn build_hostnames(&self) -> gtk4::Box {
        let state = self.state();
        let imp = self.imp();
        let hostnames = imp.hostnames.borrow().clone();
        let failures = imp.failures.borrow().clone();

        let section = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .build();

        if let Some(warning) = describe_mismatches(&hostnames) {
            section.append(&w::notice(
                "dialog-warning-symbolic",
                &warning,
                "warning",
                None,
            ));
        }

        let local = state
            .tunnels()
            .iter()
            .filter(|row| !row.tunnel.config_source.is_remotely_managed())
            .count();
        if local > 0 {
            section.append(&w::notice(
                "dialog-information-symbolic",
                &format!(
                    "{} configured by a config.yml on its own machine, so its ingress \
                         rules are not listed here.",
                    w::plural(local, "tunnel is", "tunnels are")
                ),
                "status-neutral",
                None,
            ));
        }

        if !failures.is_empty() {
            let group = w::group(
                "Not read",
                "These tunnels are missing from the list below rather than shown as \
                 empty, because an empty list would be a claim this application cannot \
                 make.",
            );
            for (tunnel_name, message) in &failures {
                let row = adw::ActionRow::builder()
                    .title(tunnel_name)
                    .subtitle(message)
                    .build();
                row.add_prefix(&w::tinted_icon("dialog-error-symbolic", "error", "Failed"));
                group.add(&row);
            }
            section.append(&group);
        }

        if hostnames.is_empty() {
            section.append(&self.build_no_hostnames_state());
            return section;
        }

        let group = w::group(
            "Public hostnames",
            "Every hostname served by every remotely-managed tunnel on this account. \
             Each is an ingress rule plus a DNS record, and both halves have to be \
             right.",
        );
        group.add(&self.build_add_row());
        for hostname in &hostnames {
            group.add(&self.build_hostname_row(hostname));
        }
        section.append(&group);

        section
    }

    fn build_no_hostnames_state(&self) -> gtk4::Widget {
        let state = self.state();

        // Still counting: showing "no hostnames" while the reads are in
        // flight would be a statement the page cannot yet support.
        if self.imp().pending.get() > 0 {
            return w::loading_state("Reading each tunnel's configuration…").upcast();
        }

        if state.tunnels().is_empty() {
            let button = gtk4::Button::builder()
                .label("Create Tunnel")
                .action_name("win.create-tunnel")
                .build();
            return w::empty_state(
                "network-vpn-symbolic",
                "No tunnels yet",
                "A public hostname is served by a tunnel, so there is nothing to publish \
                 until one exists.",
                Some(button),
            )
            .upcast();
        }

        let button = gtk4::Button::builder().label("Add Public Hostname").build();
        {
            let page = self.downgrade();
            button.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    page.add_hostname(button);
                }
            });
        }

        w::empty_state(
            "web-browser-symbolic",
            "Nothing published",
            "No tunnel on this account serves a public hostname yet. Publishing one adds \
             an ingress rule to a tunnel and a proxied CNAME to the matching zone.",
            Some(button),
        )
        .upcast()
    }

    fn build_add_row(&self) -> adw::ButtonRow {
        let row = adw::ButtonRow::builder()
            .title("Add Public Hostname")
            .start_icon_name("list-add-symbolic")
            .build();
        let page = self.downgrade();
        row.connect_activated(move |row| {
            if let Some(page) = page.upgrade() {
                page.add_hostname(row);
            }
        });
        row
    }

    fn build_hostname_row(&self, hostname: &PublicHostname) -> adw::ExpanderRow {
        let row = adw::ExpanderRow::builder()
            .title(hostname.display_name())
            .subtitle(&hostname.raw_service)
            .build();

        if hostname.disabled {
            // A disabled hostname's rule is still there — its service is
            // 404 — so the row stays and says why, instead of vanishing.
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

        row.add_row(&w::property_row("Tunnel", &hostname.tunnel_name));
        if hostname.disabled {
            row.add_row(&w::property_row(
                "Status",
                "Disabled — traffic answers 404 until you enable it again",
            ));
        }
        row.add_row(&w::property_row("Origin service", &hostname.raw_service));
        row.add_row(&w::property_row(
            "Zone",
            hostname
                .zone_name
                .as_deref()
                .unwrap_or("Not on this account"),
        ));
        row.add_row(&w::status_row(
            "DNS record",
            hostname.dns.icon_name(),
            hostname.dns.label(),
            hostname.dns.css_class(),
            Some(&hostname.dns.detail(&hostname.tunnel_name)),
        ));
        row.add_row(&w::property_row(
            "Proxy state",
            match &hostname.dns {
                DnsState::Correct { .. } => "Proxied through Cloudflare",
                DnsState::NotProxied { .. } => "Not proxied — traffic bypasses the tunnel",
                _ => "—",
            },
        ));
        row.add_row(&self.build_actions_row(hostname));

        row
    }

    fn build_actions_row(&self, hostname: &PublicHostname) -> adw::ActionRow {
        let row = adw::ActionRow::builder().title("Actions").build();

        // AdwWrapBox rather than a GtkBox: at 360px four buttons have to
        // reflow onto separate lines instead of forcing a horizontal
        // scrollbar.
        let wrap = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .valign(gtk4::Align::Center)
            .build();

        let edit = gtk4::Button::builder().label("Edit Origin").build();
        {
            let page = self.downgrade();
            let hostname = hostname.clone();
            edit.connect_clicked(move |button| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let Some(tunnel) = page.tunnel_for(&hostname, button) else {
                    return;
                };
                crate::ui::hostname_dialog::present_edit(
                    &page,
                    &page.state(),
                    &tunnel,
                    &hostname,
                    |page| page.reload(),
                );
            });
        }
        wrap.append(&edit);

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

        let move_it = gtk4::Button::builder().label("Move").build();
        {
            let page = self.downgrade();
            let hostname = hostname.clone();
            move_it.connect_clicked(move |button| {
                if page.upgrade().is_some() {
                    explain_move(button, &hostname);
                }
            });
        }
        wrap.append(&move_it);

        if hostname.disabled {
            let enable = gtk4::Button::builder()
                .label("Enable")
                .css_classes(["suggested-action"])
                .build();
            {
                let page = self.downgrade();
                let hostname = hostname.clone();
                enable.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        page.enable(button, &hostname);
                    }
                });
            }
            wrap.append(&enable);
        } else {
            let disable = gtk4::Button::builder().label("Disable").build();
            disable.set_tooltip_text(Some(
                "Traffic will answer 404 until you enable it again; the origin is \
                 remembered.",
            ));
            {
                let page = self.downgrade();
                let hostname = hostname.clone();
                disable.connect_clicked(move |button| {
                    if let Some(page) = page.upgrade() {
                        page.confirm_disable(button, &hostname);
                    }
                });
            }
            wrap.append(&disable);
        }

        let unpublish = gtk4::Button::builder()
            .label("Unpublish")
            .css_classes(["destructive-action"])
            .build();
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

        row.add_suffix(&wrap);
        row
    }

    // ── Actions ──────────────────────────────────────────────────────────

    /// The tunnel a hostname belongs to, or an explanation of why it is gone.
    fn tunnel_for(
        &self,
        hostname: &PublicHostname,
        anchor: &impl IsA<gtk4::Widget>,
    ) -> Option<Tunnel> {
        match self.state().tunnel(&hostname.tunnel_id) {
            Some(row) => Some(row.tunnel),
            None => {
                w::inform(
                    anchor,
                    "Tunnel not found",
                    "The tunnel that serves this hostname is no longer in the list. \
                     Refresh and try again.",
                );
                None
            }
        }
    }

    /// Publishes on a tunnel the user picks.
    ///
    /// The picker is skipped when there is only one candidate: a dialog whose
    /// only choice is already made asks the user to confirm nothing.
    fn add_hostname(&self, anchor: &impl IsA<gtk4::Widget>) {
        let state = self.state();
        let tunnels: Vec<Tunnel> = state
            .tunnels()
            .into_iter()
            .map(|row| row.tunnel)
            .filter(Tunnel::is_editable)
            .collect();

        match tunnels.as_slice() {
            [] => w::inform(
                anchor,
                "No tunnel to publish on",
                "A public hostname is an ingress rule on a tunnel whose configuration \
                 Cloudflare holds. This account has no such tunnel — a locally \
                 configured one is edited in its own config.yml instead.",
            ),
            [only] => {
                crate::ui::hostname_dialog::present_new(self, &state, only, |page| page.reload())
            }
            _ => self.present_tunnel_picker(anchor, tunnels),
        }
    }

    fn present_tunnel_picker(&self, anchor: &impl IsA<gtk4::Widget>, tunnels: Vec<Tunnel>) {
        let names = gtk4::StringList::new(&[]);
        for tunnel in &tunnels {
            names.append(&tunnel.name);
        }

        let combo = adw::ComboRow::builder()
            .title("Tunnel")
            .subtitle("The tunnel whose ingress rules the hostname will be added to")
            .model(&names)
            .build();

        let group = w::group("", "");
        group.add(&combo);

        let dialog = adw::AlertDialog::new(
            Some("Publish a hostname"),
            Some(
                "The hostname's DNS record will point at whichever tunnel is chosen here, \
                 so pick the one that runs the service it should reach.",
            ),
        );
        dialog.set_extra_child(Some(&group));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("continue", "Continue");
        dialog.set_response_appearance("continue", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("continue"));
        dialog.set_close_response("cancel");

        let page = self.downgrade();
        dialog.connect_response(None, move |_, response| {
            if response != "continue" {
                return;
            }
            let Some(page) = page.upgrade() else {
                return;
            };
            let Some(tunnel) = tunnels.get(combo.selected() as usize) else {
                return;
            };
            crate::ui::hostname_dialog::present_new(&page, &page.state(), tunnel, |page| {
                page.reload()
            });
        });

        dialog.present(Some(anchor.as_ref()));
    }

    fn repair_dns(&self, anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
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
        let tunnel_id = hostname.tunnel_id.clone();
        let state_clone = state.clone();
        let page = self.downgrade();
        let cancel = Cancel::new();

        runtime::spawn(
            async move { hostnames::repair_dns(&client, &zone_id, &tunnel_id, &host, &cancel).await },
            move |outcome| {
                record_outcome(&state_clone, &outcome, ActivityCategory::Dns);
                if let Some(page) = page.upgrade() {
                    crate::ui::hostname_dialog::show_outcome(&page, &outcome);
                    page.reload();
                }
            },
        );
    }

    /// Disables a hostname: the rule stays, its service becomes 404, and the
    /// origin it served is remembered so Enable can restore it exactly.
    fn confirm_disable(&self, anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
        let hostname = hostname.clone();
        let page = self.downgrade();

        w::confirm(
            anchor,
            &format!("Disable {}?", hostname.display_name()),
            &format!(
                "“{}” will answer 404 until you enable it again. The ingress rule \
                 stays in place, and the origin it serves is remembered, so Enable \
                 restores it exactly. No DNS record is touched.",
                hostname.tunnel_name
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
                let tunnel_id = hostname_for_call.tunnel_id.clone();
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
                                    &hostname.tunnel_id,
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
                                    page.reload();
                                }
                            },
                        );
                    },
                );
            },
        );
    }

    /// Re-enables a disabled hostname by restoring the origin it served.
    fn enable(&self, _anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
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
        let tunnel_id = hostname_for_call.tunnel_id.clone();
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
                            page.reload();
                        }
                    },
                );
            },
        );
    }

    fn confirm_unpublish(&self, anchor: &gtk4::Button, hostname: &PublicHostname) {
        let state = self.state();
        let hostname = hostname.clone();
        let page = self.downgrade();

        let mut warnings = vec![format!(
            "The ingress rule for {} will be removed from “{}”. Every other rule on that \
             tunnel is left exactly as it is.",
            hostname.display_name(),
            hostname.tunnel_name
        )];
        match &hostname.dns {
            DnsState::Correct { .. } | DnsState::NotProxied { .. } => warnings.push(format!(
                "The CNAME for {} pointing at that tunnel will be removed from the {} \
                 zone. No other DNS record for the hostname will be touched.",
                hostname.hostname,
                hostname.zone_name.as_deref().unwrap_or("matching")
            )),
            _ => warnings.push(
                "No DNS record pointing at that tunnel was found, so none will be \
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
                let tunnel_id = hostname.tunnel_id.clone();
                let host = hostname.hostname.clone();
                let path = hostname.path.clone();
                let zone_id = hostname.zone_id.clone();
                let state_clone = state.clone();
                let page_ref = page.downgrade();
                let cancel = Cancel::new();
                // A second copy for the outcome callback: unpublishing a
                // disabled hostname also forgets its remembered origin.
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
                            page.reload();
                        }
                    },
                );
            },
        );
    }
}

/// Why there is no in-place "move to another tunnel".
///
/// Moving a hostname means repointing a live DNS record: the old tunnel stops
/// serving it at the moment the record changes, and any mistake in the new
/// tunnel's ingress rule is a public outage rather than a failed edit. Making
/// it two deliberate steps — unpublish here, publish there — means the DNS
/// record is only ever changed by an operation the user named, and each half
/// reports its own outcome.
fn explain_move(anchor: &impl IsA<gtk4::Widget>, hostname: &PublicHostname) {
    w::inform(
        anchor,
        &format!("Moving {}", hostname.display_name()),
        &format!(
            "There is no one-click move, on purpose. Moving this hostname to another \
             tunnel means repointing its DNS record, and a repointed record is a change \
             the public sees immediately.\n\n\
             Unpublish it from “{}” here, then publish it on the other tunnel. Each step \
             names exactly what it changes and reports whether the ingress rule and the \
             DNS record both succeeded, which a single combined action cannot do.",
            hostname.tunnel_name
        ),
    );
}

/// One sentence counting the hostnames that are not being served as
/// configured, or `None` when every one of them is healthy.
fn describe_mismatches(hostnames: &[PublicHostname]) -> Option<String> {
    let mut missing = 0usize;
    let mut not_proxied = 0usize;
    let mut conflicting = 0usize;
    let mut unchecked = 0usize;

    for hostname in hostnames {
        match &hostname.dns {
            DnsState::Correct { .. } => {}
            DnsState::NotProxied { .. } => not_proxied += 1,
            DnsState::Missing => missing += 1,
            DnsState::Conflicting { .. } => conflicting += 1,
            DnsState::Unknown { .. } => unchecked += 1,
        }
    }

    let total = missing + not_proxied + conflicting + unchecked;
    if total == 0 {
        return None;
    }

    let mut parts = Vec::new();
    if missing > 0 {
        parts.push(format!("{missing} with no DNS record"));
    }
    if not_proxied > 0 {
        parts.push(format!("{not_proxied} not proxied"));
    }
    if conflicting > 0 {
        parts.push(format!("{conflicting} pointing somewhere else"));
    }
    if unchecked > 0 {
        parts.push(format!("{unchecked} whose DNS could not be checked"));
    }

    Some(format!(
        "{} not served as configured: {}.",
        w::plural(total, "hostname is", "hostnames are"),
        parts.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::hostname::OriginService;

    fn hostname(name: &str, dns: DnsState) -> PublicHostname {
        PublicHostname {
            hostname: name.into(),
            path: None,
            service: OriginService::http("localhost", 8080),
            raw_service: "http://localhost:8080".into(),
            tunnel_id: "t".repeat(32),
            tunnel_name: "edge".into(),
            zone_id: None,
            zone_name: None,
            dns,
            origin_request: None,
            rule_index: 0,
            disabled: false,
        }
    }

    #[test]
    fn a_healthy_list_raises_no_banner() {
        let rows = vec![hostname(
            "app.example.com",
            DnsState::Correct {
                record_id: "1".into(),
            },
        )];
        assert_eq!(describe_mismatches(&rows), None);
        assert_eq!(describe_mismatches(&[]), None);
    }

    #[test]
    fn the_banner_counts_each_kind_of_mismatch() {
        let rows = vec![
            hostname("a.example.com", DnsState::Missing),
            hostname("b.example.com", DnsState::Missing),
            hostname(
                "c.example.com",
                DnsState::NotProxied {
                    record_id: "1".into(),
                },
            ),
            hostname(
                "d.example.com",
                DnsState::Correct {
                    record_id: "2".into(),
                },
            ),
        ];

        let warning = describe_mismatches(&rows).expect("three are wrong");
        assert!(
            warning.starts_with("3 hostnames are not served"),
            "{warning}"
        );
        assert!(warning.contains("2 with no DNS record"), "{warning}");
        assert!(warning.contains("1 not proxied"), "{warning}");
    }

    #[test]
    fn one_mismatch_reads_as_singular() {
        let rows = vec![hostname("a.example.com", DnsState::Missing)];
        let warning = describe_mismatches(&rows).expect("one is wrong");
        assert!(warning.starts_with("1 hostname is not served"), "{warning}");
    }
}
