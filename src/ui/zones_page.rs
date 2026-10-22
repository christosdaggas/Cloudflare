//! Zones — the domains this token can see.
//!
//! A published hostname is two facts in two places: an ingress rule on a
//! tunnel, and a proxied CNAME in a zone. The hostnames themselves live on
//! their own page; this one answers the question that has to be answered
//! *before* publishing — which domains exist on the account, and are they
//! ready to serve records yet. A zone that is still pending will accept a
//! record and then serve nothing, and that has to be known before publishing
//! into it, not discovered afterwards.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::cloudflare::Cancel;
use crate::domain::account::PermissionState;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::services::runtime;
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ZonesPage {
        pub state: RefCell<Option<AppState>>,
        pub stack: RefCell<Option<gtk4::Stack>>,
        pub content: RefCell<Option<gtk4::Box>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ZonesPage {
        const NAME: &'static str = "TunnelManagerZonesPage";
        type Type = super::ZonesPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ZonesPage {}
    impl WidgetImpl for ZonesPage {}
    impl BinImpl for ZonesPage {}
}

glib::wrapper! {
    pub struct ZonesPage(ObjectSubclass<imp::ZonesPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl ZonesPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                if !matches!(event, Event::Zones | Event::Busy) {
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

        let stack = gtk4::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.add_named(&w::loading_state("Reading zones…"), Some("loading"));
        stack.add_named(&scroller, Some("content"));
        stack.add_named(&self.build_disconnected_state(), Some("empty"));

        self.imp().content.replace(Some(content));
        self.imp().stack.replace(Some(stack.clone()));
        self.set_child(Some(&stack));

        self.refresh();
    }

    fn build_disconnected_state(&self) -> adw::StatusPage {
        w::empty_state(
            "network-offline-symbolic",
            "Not connected",
            "Connect a Cloudflare API token to see the domains on the account.",
            None,
        )
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

        let zones = state.zones();
        if zones.is_empty() && state.is_busy("zones") {
            stack.set_visible_child_name("loading");
            return;
        }
        stack.set_visible_child_name("content");

        while let Some(child) = content.first_child() {
            content.remove(&child);
        }

        content.append(&self.build_zones());
    }

    fn build_zones(&self) -> gtk4::Widget {
        let state = self.state();
        let zones = state.zones();

        if zones.is_empty() {
            return self.build_no_zones_state().upcast();
        }

        let default_zone_id = state.settings().default_zone_id;
        let group = w::group(
            "Zones",
            "The domains this token can see on the connected account. A hostname can \
             only be published into one of these.",
        );

        for zone in &zones {
            let row = adw::ActionRow::builder()
                .title(&zone.name)
                .subtitle(&zone.account_name)
                .build();
            row.set_title_selectable(true);

            if default_zone_id.as_deref() == Some(zone.id.as_str()) {
                // A word and an icon, not a tint: the default zone has to be
                // identifiable in greyscale and to a screen reader.
                row.add_suffix(&w::status_badge("starred-symbolic", "Default", "accent"));
            }
            row.add_suffix(&w::status_badge(
                zone.status.icon_name(),
                zone.status.label(),
                zone.status.css_class(),
            ));
            group.add(&row);

            // Shown inline rather than folded away: a record created in a zone
            // that is not yet using Cloudflare's nameservers is accepted and
            // then serves nothing, and that has to be read before publishing,
            // not discovered afterwards.
            if let Some(caveat) = zone.status.caveat() {
                let note = adw::ActionRow::builder()
                    .title("Before you publish here")
                    .subtitle(caveat)
                    .build();
                note.add_prefix(&w::tinted_icon(
                    zone.status.icon_name(),
                    zone.status.css_class(),
                    zone.status.label(),
                ));
                group.add(&note);
            }
        }

        group.upcast()
    }

    /// No zones is a legitimate state, not a failure — a token scoped to
    /// tunnels alone is a supported way to run this application.
    fn build_no_zones_state(&self) -> adw::StatusPage {
        let denied = self
            .state()
            .session()
            .is_some_and(|session| session.capabilities.zones.0 == PermissionState::Denied);

        let description = if denied {
            "The API token was refused when it asked for the account's domains, so it \
             is missing the Zone:Read permission. Tunnels still work without it; \
             publishing a public hostname does not."
        } else {
            "Either the API token has no Zone:Read permission, or the account has no \
             domains on it. Tunnels, connectors and remote hosts all work without a \
             zone — only publishing a public hostname needs one."
        };

        let retry = gtk4::Button::builder().label("Check Again").build();
        {
            let state = self.state();
            retry.connect_clicked(move |_| refresh_zones(&state));
        }

        w::empty_state("web-browser-symbolic", "No zones", description, Some(retry))
    }
}

/// Reloads the account's zones into the shared state.
///
/// Module-level rather than a method on the page for the same reason
/// `refresh_tunnels` is: the refresh action, the automatic timer and the retry
/// button above all want identical behaviour, and none of them should have to
/// hold a page to get it.
pub fn refresh_zones(state: &AppState) {
    let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
        return;
    };
    if state.is_busy("zones") {
        return;
    }

    state.set_busy("zones", true);
    let client = session.client.clone();
    let cancel = Cancel::new();
    let state = state.clone();

    runtime::spawn(
        async move { client.list_zones(Some(&account_id), &cancel).await },
        move |result| {
            state.set_busy("zones", false);
            match result {
                Ok(zones) => {
                    tracing::debug!(count = zones.len(), "zone list refreshed");
                    note_zone_access(&state, PermissionState::Granted);
                    state.set_zones(zones);
                }
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    // A refusal is the one reliable evidence the token lacks
                    // Zone:Read, and it is what lets the empty state say so
                    // rather than guessing between "no permission" and "no
                    // domains".
                    if error.is_permission_problem() {
                        note_zone_access(&state, PermissionState::Denied);
                    }
                    state.record(
                        ActivityEvent::error(ActivityCategory::Api, "Could not list zones")
                            .with_detail(error.user_message()),
                    );
                }
            }
        },
    );
}

fn note_zone_access(state: &AppState, permission: PermissionState) {
    let Some(mut session) = state.session() else {
        return;
    };
    if session.capabilities.zones.0 == permission {
        return;
    }
    session.capabilities.zones.0 = permission;
    state.set_session(Some(session));
}
