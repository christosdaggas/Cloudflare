//! The dashboard.
//!
//! One screen that answers "is anything wrong, and what do I do next". It owns
//! no data of its own: everything is read from [`AppState`] and rebuilt when
//! the state says something changed, which means it can never drift out of
//! step with the Tunnels page about what "degraded" means.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::domain::tunnel::{TunnelFilter, TunnelStatus};
use crate::services::tunnels;
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct DashboardPage {
        pub state: RefCell<Option<AppState>>,
        pub content: RefCell<Option<gtk4::Box>>,
        pub stack: RefCell<Option<gtk4::Stack>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for DashboardPage {
        const NAME: &'static str = "TunnelManagerDashboardPage";
        type Type = super::DashboardPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for DashboardPage {}
    impl WidgetImpl for DashboardPage {}
    impl BinImpl for DashboardPage {}
}

glib::wrapper! {
    pub struct DashboardPage(ObjectSubclass<imp::DashboardPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl DashboardPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                if !matches!(
                    event,
                    Event::Tunnels | Event::Hosts | Event::Connection | Event::Busy
                ) {
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

        // Three states share the page: loading, empty and populated. A stack
        // rather than rebuilding one container, so the transition does not
        // flash and the scroll position is kept per state.
        let stack = gtk4::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.add_named(&w::loading_state("Loading tunnels…"), Some("loading"));
        stack.add_named(&scroller, Some("content"));
        stack.add_named(&self.build_empty_state(), Some("empty"));
        stack.add_named(&self.build_error_state(), Some("error"));

        self.imp().content.replace(Some(content));
        self.imp().stack.replace(Some(stack.clone()));
        self.set_child(Some(&stack));

        self.refresh();
    }

    fn build_empty_state(&self) -> adw::StatusPage {
        let button = gtk4::Button::builder()
            .label("Create Tunnel")
            .action_name("win.create-tunnel")
            .build();

        w::empty_state(
            "network-vpn-symbolic",
            "No tunnels yet",
            "A Cloudflare Tunnel connects a service on a machine you control to the \
             internet, without opening a port. Create one to get started.",
            Some(button),
        )
    }

    fn build_error_state(&self) -> adw::StatusPage {
        let button = gtk4::Button::builder().label("Try Again").build();
        {
            let page = self.downgrade();
            button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    let state = page.state();
                    crate::ui::onboarding_page::refresh_tunnels(&state);
                }
            });
        }

        w::error_state(
            "network-error-symbolic",
            "Could not list tunnels",
            "The dashboard could not be loaded. The detail is in the Activity page.",
            Some(button),
        )
    }

    fn refresh(&self) {
        let state = self.state();
        let Some(content) = self.imp().content.borrow().clone() else {
            return;
        };
        let Some(stack) = self.imp().stack.borrow().clone() else {
            return;
        };

        let rows = state.tunnels();

        if rows.is_empty() {
            if state.is_busy("tunnels") {
                stack.set_visible_child_name("loading");
            } else if state.tunnels_error().is_some() {
                stack.set_visible_child_name("error");
            } else {
                stack.set_visible_child_name("empty");
            }
            return;
        }
        stack.set_visible_child_name("content");

        while let Some(child) = content.first_child() {
            content.remove(&child);
        }

        if let Some(banner) = self.build_attention_banner() {
            content.append(&banner);
        }
        content.append(&self.build_tunnel_stats());
        content.append(&self.build_reach_stats());
        content.append(&self.build_quick_actions());
    }

    /// The one line at the top that says whether anything needs looking at.
    fn build_attention_banner(&self) -> Option<gtk4::Widget> {
        let state = self.state();
        let summary = tunnels::summarise(&state.tunnels());
        let unreachable = state.unreachable_host_count();

        let mut problems = Vec::new();
        if summary.down > 0 {
            problems.push(w::plural(
                summary.down,
                "tunnel is down",
                "tunnels are down",
            ));
        }
        if summary.degraded > 0 {
            problems.push(format!(
                "{} degraded",
                w::plural(summary.degraded, "tunnel is", "tunnels are")
            ));
        }
        if unreachable > 0 {
            problems.push(format!(
                "{} unreachable",
                w::plural(unreachable, "registered host is", "registered hosts are")
            ));
        }

        if problems.is_empty() {
            return None;
        }

        let button = gtk4::Button::builder()
            .label("Show Tunnels")
            .action_name("win.page")
            .css_classes(["pill", "suggested-action"])
            .build();
        button.set_action_target_value(Some(&"tunnels".to_variant()));

        let notice = w::notice(
            "dialog-warning-symbolic",
            &format!("Needs attention: {}.", problems.join(", ")),
            "warning",
            Some(button),
        );

        // The label wraps and the page's own scroller gives it the content
        // width. No extra `wrappable` here: that would allocate the card at
        // its minimum width and wrap the sentence long before the edge.
        Some(notice.upcast())
    }

    fn build_tunnel_stats(&self) -> gtk4::Box {
        let state = self.state();
        let rows = state.tunnels();
        let summary = tunnels::summarise(&rows);

        let section = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .build();

        let heading = gtk4::Label::builder()
            .label("Tunnels")
            .halign(gtk4::Align::Start)
            .build();
        heading.add_css_class("title-4");
        section.append(&heading);

        // AdwWrapBox rather than a GtkBox: at 360px the four cards have to
        // reflow onto separate lines instead of forcing a horizontal
        // scrollbar.
        let wrap = adw::WrapBox::builder()
            .child_spacing(12)
            .line_spacing(12)
            .build();

        let cards: &[(usize, &str, TunnelStatus, TunnelFilter)] = &[
            (
                summary.healthy,
                "Healthy",
                TunnelStatus::Healthy,
                TunnelFilter::Healthy,
            ),
            (
                summary.degraded,
                "Degraded",
                TunnelStatus::Degraded,
                TunnelFilter::Degraded,
            ),
            (summary.down, "Down", TunnelStatus::Down, TunnelFilter::Down),
            (
                summary.inactive + summary.other,
                "Inactive",
                TunnelStatus::Inactive,
                TunnelFilter::Inactive,
            ),
        ];

        let total = rows.len();
        let share = format!("of {}", w::plural(total, "tunnel", "tunnels"));

        for (count, label, status, filter) in cards {
            let mut card = w::StatCard::new(&count.to_string(), label, status.icon_name())
                .detail(&share)
                .proportion(if total > 0 {
                    *count as f64 / total as f64
                } else {
                    0.0
                });
            // A zero is not a problem, however bad the state it counts,
            // so it stays neutral rather than shouting in red.
            if *count > 0 {
                card = card.tint(status.css_class());
            }
            let card = card.activates().build();

            if let Some(button) = card.downcast_ref::<gtk4::Button>() {
                button.set_tooltip_text(Some(status.explanation()));
                button.set_action_name(Some("win.tunnels-filter"));
                button.set_action_target_value(Some(&filter.name().to_variant()));
            }

            wrap.append(&card);
        }

        section.append(&wrap);
        section
    }

    fn build_reach_stats(&self) -> gtk4::Box {
        let state = self.state();
        let rows = state.tunnels();

        let section = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .build();

        let heading = gtk4::Label::builder()
            .label("Reach")
            .halign(gtk4::Align::Start)
            .build();
        heading.add_css_class("title-4");
        section.append(&heading);

        let wrap = adw::WrapBox::builder()
            .child_spacing(12)
            .line_spacing(12)
            .build();

        // "Still counting" rather than 0: the hostname count needs one request
        // per tunnel, and showing zero while they are in flight would be a
        // lie the user might act on.
        let tunnel_count = rows.len();
        let hostnames = match tunnels::total_hostnames(&rows) {
            Some(total) => total.to_string(),
            None => "…".into(),
        };
        wrap.append(
            &w::StatCard::new(&hostnames, "Public hostnames", "web-browser-symbolic")
                .detail(format!(
                    "across {}",
                    w::plural(tunnel_count, "tunnel", "tunnels")
                ))
                .build(),
        );

        let host_count = state.hosts().len();
        let unreachable = state.unreachable_host_count();

        let hosts_card = w::StatCard::new(
            &host_count.to_string(),
            if host_count == 0 {
                "Registered hosts — optional"
            } else {
                "Registered hosts"
            },
            "network-server-symbolic",
        )
        .activates()
        .build();
        if let Some(button) = hosts_card.downcast_ref::<gtk4::Button>() {
            button.set_action_name(Some("win.page"));
            button.set_action_target_value(Some(&"hosts".to_variant()));
        }
        wrap.append(&hosts_card);

        if unreachable > 0 {
            let card = w::StatCard::new(
                &unreachable.to_string(),
                "Unreachable hosts",
                "network-offline-symbolic",
            )
            .tint("warning")
            .detail(format!(
                "of {}",
                w::plural(host_count, "registered host", "registered hosts")
            ))
            .proportion(if host_count > 0 {
                unreachable as f64 / host_count as f64
            } else {
                0.0
            })
            .activates()
            .build();
            if let Some(button) = card.downcast_ref::<gtk4::Button>() {
                button.set_action_name(Some("win.page"));
                button.set_action_target_value(Some(&"hosts".to_variant()));
            }
            wrap.append(&card);
        }

        section.append(&wrap);

        if host_count == 0 {
            let note = w::inline_message(
                "dialog-information-symbolic",
                "No remote hosts are registered. Every Cloudflare feature works without \
                 one; registering a host adds the ability to start and stop cloudflared \
                 over SSH.",
                "dimmed",
            );
            note.set_margin_top(6);
            section.append(&note);
        }

        section
    }

    fn build_quick_actions(&self) -> gtk4::Box {
        let section = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .build();

        let heading = gtk4::Label::builder()
            .label("Quick actions")
            .halign(gtk4::Align::Start)
            .build();
        heading.add_css_class("title-4");
        section.append(&heading);

        // One row at every width: the four buttons share the row equally and
        // their content fills the button, so nothing can widen the window.
        // At 360px the labels are allowed to ellipsize; the tooltip keeps the
        // full wording available.
        let row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .homogeneous(true)
            .build();

        for (icon, label, action, target) in [
            (
                "list-add-symbolic",
                "Create Tunnel",
                "win.create-tunnel",
                None,
            ),
            ("view-refresh-symbolic", "Refresh", "win.refresh", None),
            (
                "network-server-symbolic",
                "Add Remote Host",
                "win.add-host",
                None,
            ),
            (
                "document-open-recent-symbolic",
                "Show Activity",
                "win.page",
                Some("activity"),
            ),
        ] {
            let content = adw::ButtonContent::builder()
                .icon_name(icon)
                .label(label)
                .can_shrink(true)
                .build();
            // Fill, not the default: a shrinking `ButtonContent` that is not
            // told to fill is allocated its minimum width, which ellipsizes
            // every label even when the button has room for it.
            content.set_halign(gtk4::Align::Fill);
            content.set_hexpand(true);

            let button = gtk4::Button::builder()
                .child(&content)
                .action_name(action)
                .tooltip_text(label)
                .css_classes(["pill", "quick-action"])
                .build();
            if let Some(target) = target {
                button.set_action_target_value(Some(&target.to_variant()));
            }
            row.append(&button);
        }

        section.append(&row);
        section
    }
}
