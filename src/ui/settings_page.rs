//! Settings — every preference the application has, and an honest account of
//! what it keeps, where it keeps it and what it would hand to someone helping
//! you.
//!
//! The groups sit in a plain column under the same `AdwClamp` as every other
//! page — deliberately not an `AdwPreferencesPage`, which narrows groups to
//! about two thirds of the page width (measured 567px in a 790px window)
//! while every other page spans the full clamp.
//! Three things about its shape are deliberate.
//!
//! **The four page states live in the credential group rather than in a
//! stack.** Loading, empty, populated and failed are all states of the
//! *connection*; the theme, the log level and the diagnostic report have to
//! stay reachable while Cloudflare is not, which is precisely when somebody
//! needs them.
//!
//! **A full rebuild happens only when the connection changes.** Settings
//! notifications arrive on every keystroke, because this page persists as you
//! type, so they update individual widgets instead — and never overwrite an
//! entry the user is still inside.
//!
//! **Nothing here reads or writes a secret.** The API token is only ever
//! removed, and that goes through the credential store on a background
//! thread; the token itself is never loaded into this page.

use std::cell::{Cell, RefCell};

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::branding;
use crate::domain::account::REQUIRED_PERMISSIONS;
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::validation;
use crate::services::logging::{self, LOG_LEVELS};
use crate::services::runtime;
use crate::services::session::ConnectionState;
use crate::storage::{self, settings::REFRESH_INTERVALS};
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SettingsPage {
        pub state: RefCell<Option<AppState>>,

        /// Holds the preferences page, so the whole thing can be swapped
        /// without disturbing the banners above it.
        pub body: RefCell<Option<adw::Bin>>,
        pub storage_banner: RefCell<Option<adw::Banner>>,
        pub secrets_banner: RefCell<Option<adw::Banner>>,

        pub token_row: RefCell<Option<adw::ActionRow>>,
        pub token_badge: RefCell<Option<gtk4::Box>>,
        pub remove_token_row: RefCell<Option<adw::ButtonRow>>,

        pub zone_row: RefCell<Option<adw::ComboRow>>,
        /// One entry per row of the domain list, the first of which is "no
        /// default". Kept beside the model because a `GtkStringList` holds
        /// names and the setting holds IDs.
        pub zone_ids: RefCell<Vec<Option<String>>>,

        pub theme_row: RefCell<Option<adw::ComboRow>>,
        pub refresh_row: RefCell<Option<adw::ComboRow>>,
        pub refresh_intervals: RefCell<Vec<u32>>,
        pub log_row: RefCell<Option<adw::ComboRow>>,

        pub remote_group: RefCell<Option<adw::PreferencesGroup>>,
        pub username_row: RefCell<Option<adw::EntryRow>>,
        pub port_row: RefCell<Option<adw::SpinRow>>,
        pub unit_row: RefCell<Option<adw::EntryRow>>,
        pub unit_error: RefCell<Option<gtk4::Box>>,

        /// Set while widgets are being brought back into step with the
        /// settings they display, so a programmatic change is not mistaken
        /// for one the user made.
        pub syncing: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SettingsPage {
        const NAME: &'static str = "TunnelManagerSettingsPage";
        type Type = super::SettingsPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for SettingsPage {}
    impl WidgetImpl for SettingsPage {}
    impl BinImpl for SettingsPage {}
}

glib::wrapper! {
    pub struct SettingsPage(ObjectSubclass<imp::SettingsPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl SettingsPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                match event {
                    // The account, the permissions and the domain list all
                    // change together, and none of it can arrive while this
                    // page has the focus — the window is showing the
                    // connection page at that point.
                    Event::Connection => page.rebuild(),
                    Event::Zones => {
                        if page.zones_changed() {
                            page.rebuild();
                        }
                    }
                    Event::Settings => page.sync(),
                    _ => {}
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
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();

        // Both banners are built once and revealed as needed: an
        // `AdwBanner` animates its own appearance, which a widget added and
        // removed from the tree cannot.
        let storage_banner = adw::Banner::builder().use_markup(false).build();
        let secrets_banner = adw::Banner::builder()
            .title(
                "No desktop credential store answered, so the API token cannot be \
                 remembered after you quit. On GNOME this usually means gnome-keyring \
                 is not running.",
            )
            .use_markup(false)
            .build();
        column.append(&w::wrappable(&storage_banner));
        column.append(&w::wrappable(&secrets_banner));

        let body = adw::Bin::builder().vexpand(true).build();
        column.append(&body);

        let imp = self.imp();
        imp.storage_banner.replace(Some(storage_banner));
        imp.secrets_banner.replace(Some(secrets_banner));
        imp.body.replace(Some(body));

        self.set_child(Some(&column));
        self.rebuild();
    }

    fn rebuild(&self) {
        let Some(body) = self.imp().body.borrow().clone() else {
            return;
        };

        // Plain column, not AdwPreferencesPage: the page widget narrows its
        // groups to ~567px in a 790px window while every other page spans
        // the full clamp, so the cards never lined up. Same shape as the
        // zones page.
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(24)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(12)
            .margin_end(12)
            .build();

        let intro = gtk4::Label::builder()
            .label(
                "These preferences are stored on this machine. Nothing on this page is sent \
                 to Cloudflare.",
            )
            .halign(gtk4::Align::Center)
            .justify(gtk4::Justification::Center)
            .wrap(true)
            .build();
        intro.add_css_class("dimmed");
        column.append(&intro);

        column.append(&self.build_credential_group());
        column.append(&self.build_account_group());
        column.append(&self.build_appearance_group());
        column.append(&self.build_refresh_group());
        column.append(&self.build_remote_group());
        column.append(&self.build_data_group());
        column.append(&self.build_about_group());

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
        body.set_child(Some(&scroller));
        self.sync();
    }

    // ── The credential ───────────────────────────────────────────────────

    fn build_credential_group(&self) -> adw::PreferencesGroup {
        let state = self.state();
        let group = w::group(
            "Cloudflare credential",
            "One API token, held in your desktop's credential store. It is never written \
             to this application's database and never appears in the activity log.",
        );

        match state.connection() {
            ConnectionState::Connecting => {
                let row = adw::ActionRow::builder()
                    .title("Checking the API token…")
                    .subtitle("Verifying it with Cloudflare and working out what it can reach.")
                    .build();
                row.add_suffix(
                    &adw::Spinner::builder()
                        .width_request(16)
                        .height_request(16)
                        .valign(gtk4::Align::Center)
                        .build(),
                );
                group.add(&row);
            }
            ConnectionState::Connected { account, .. } => {
                group.add(&w::status_row(
                    "Connection",
                    "object-select-symbolic",
                    "Connected",
                    "success",
                    Some(&format!("{} · {}", account.name, account.short_id())),
                ));
            }
            ConnectionState::Failed { title, message } => {
                group.add(&w::status_row(
                    "Connection",
                    "dialog-error-symbolic",
                    "Failed",
                    "error",
                    Some(&title),
                ));
                let detail = w::inline_message("dialog-error-symbolic", &message, "error");
                detail.set_margin_top(12);
                group.add(&detail);
            }
            ConnectionState::Disconnected => {
                group.add(&w::status_row(
                    "Connection",
                    "network-offline-symbolic",
                    "Not connected",
                    "status-neutral",
                    Some("No API token has been accepted yet. The connection page asks for one."),
                ));
            }
        }

        let token_row = adw::ActionRow::builder().title("Saved API token").build();
        group.add(&token_row);
        self.imp().token_badge.replace(None);
        self.imp().token_row.replace(Some(token_row));
        self.apply_token_state();

        // "Not checked" is not "missing": the API reports only that a token
        // is live, so each permission is inferred from a call that has
        // actually been made.
        let capabilities = state.session().map(|session| session.capabilities);
        let capabilities = capabilities.unwrap_or_default();
        for (index, permission) in REQUIRED_PERMISSIONS.iter().enumerate() {
            let permission_state = capabilities.get(index);
            group.add(&w::status_row(
                &format!(
                    "{} → {} → {}",
                    permission.scope, permission.dashboard_name, permission.level
                ),
                permission_state.icon_name(),
                permission_state.label(),
                permission_state.css_class(),
                Some(permission.needed_for),
            ));
        }

        if capabilities.has_known_gap() {
            let note = w::inline_message(
                "dialog-warning-symbolic",
                "A missing permission cannot be granted from here. Edit the token in the \
                 Cloudflare dashboard, then use Change API Token above.",
                "warning",
            );
            note.set_margin_top(12);
            group.add(&note);
        }

        let change = adw::ButtonRow::builder()
            .title("Change API Token")
            .start_icon_name("dialog-password-symbolic")
            .build();
        change.connect_activated({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let state = page.state();
                w::confirm(
                    row,
                    "Change the API token?",
                    "Tunnel Manager returns to the connection page and forgets the saved \
                     token. Registered hosts, and every preference on this page, are kept.",
                    "Disconnect",
                    move || state.disconnect(),
                );
            }
        });
        group.add(&change);

        let remove = adw::ButtonRow::builder()
            .title("Remove Saved Token")
            .start_icon_name("user-trash-symbolic")
            .build();
        remove.add_css_class("destructive-action");
        remove.connect_activated({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.confirm_remove_token(row);
            }
        });
        group.add(&remove);
        self.imp().remove_token_row.replace(Some(remove));

        group
    }

    /// The saved token's state as an icon, a word, a tint and a sentence.
    fn token_state(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        let state = self.state();
        if !state.secrets_available() {
            return (
                "dialog-warning-symbolic",
                "Unavailable",
                "warning",
                "No credential store answered, so the token cannot be remembered.",
            );
        }
        if state.settings().token_saved {
            (
                "object-select-symbolic",
                "Saved",
                "success",
                "In your desktop's credential store, never in this application's database.",
            )
        } else {
            (
                "dialog-information-symbolic",
                "Not saved",
                "status-neutral",
                "This session only. You will be asked for the token again next time.",
            )
        }
    }

    fn apply_token_state(&self) {
        let imp = self.imp();
        let Some(row) = imp.token_row.borrow().clone() else {
            return;
        };

        let (icon, label, class, detail) = self.token_state();
        row.set_subtitle(detail);

        if let Some(previous) = imp.token_badge.take() {
            row.remove(&previous);
        }
        let badge = w::status_badge(icon, label, class);
        row.add_suffix(&badge);
        imp.token_badge.replace(Some(badge));
    }

    fn confirm_remove_token(&self, anchor: &impl IsA<gtk4::Widget>) {
        let page = self.downgrade();
        w::confirm_destructive(
            anchor,
            "Remove the saved API token?",
            "The token is deleted from your desktop's credential store. This session stays \
             connected; the next time Tunnel Manager starts you will have to paste the \
             token again.",
            &[],
            "Remove Token",
            move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.remove_token();
            },
        );
    }

    /// The credential store speaks D-Bus and can block for as long as the
    /// keyring takes to unlock, so this never runs on the main thread.
    fn remove_token(&self) {
        let state = self.state();
        let page = self.downgrade();

        runtime::spawn_blocking(storage::secrets::delete_api_token, move |result| {
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    state.update_settings(|settings| settings.token_saved = false);
                    state.record(ActivityEvent::info(
                        ActivityCategory::App,
                        "Removed the saved Cloudflare API token from the credential store.",
                    ));
                    w::toast(&page, "Saved token removed");
                }
                Err(error) => {
                    let message = error.user_message();
                    state.record(ActivityEvent::warning(
                        ActivityCategory::App,
                        message.clone(),
                    ));
                    w::sticky_toast(&page, &message, None);
                }
            }
        });
    }

    // ── Account and domain ───────────────────────────────────────────────

    fn build_account_group(&self) -> adw::PreferencesGroup {
        let state = self.state();
        let settings = state.settings();
        let group = w::group(
            "Account and domain",
            "Which account this application works in, and which domain a new public \
             hostname is offered under.",
        );

        let account_name = state
            .session()
            .map(|session| session.account.name)
            .or(settings.account_name)
            .unwrap_or_else(|| "Not selected".into());
        group.add(&w::property_row("Account", &account_name));

        if let Some(id) = state.account_id() {
            group.add(&w::copyable_row("Account ID", &id));
        }

        let zones = state.zones();
        let names = gtk4::StringList::new(&["No default domain"]);
        let mut ids: Vec<Option<String>> = vec![None];
        for zone in &zones {
            names.append(&zone.name);
            ids.push(Some(zone.id.clone()));
        }

        let row = adw::ComboRow::builder()
            .title("Default domain")
            .subtitle(
                "Offered first when a hostname is published. A hostname can still be \
                 created on any other domain the token can see.",
            )
            .model(&names)
            .build();
        row.set_selected(index_of(&ids, &settings.default_zone_id));

        row.connect_selected_notify({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                let selected = page
                    .imp()
                    .zone_ids
                    .borrow()
                    .get(row.selected() as usize)
                    .cloned()
                    .flatten();
                let name = selected.as_ref().and_then(|id| {
                    page.state()
                        .zones()
                        .into_iter()
                        .find(|zone| zone.id == *id)
                        .map(|zone| zone.name)
                });
                page.state().update_settings(|settings| {
                    settings.default_zone_id = selected;
                    settings.default_zone_name = name;
                });
            }
        });

        group.add(&row);
        self.imp().zone_row.replace(Some(row));
        self.imp().zone_ids.replace(ids);

        if zones.is_empty() {
            let note = w::inline_message(
                "dialog-information-symbolic",
                "No domains are visible with this token. Tunnels work without one; \
                 publishing a public hostname needs the Zone → Read permission.",
                "dimmed",
            );
            note.set_margin_top(12);
            group.add(&note);
        }

        group
    }

    /// Whether the domain list on screen still matches the session's.
    fn zones_changed(&self) -> bool {
        let current: Vec<Option<String>> = std::iter::once(None)
            .chain(self.state().zones().into_iter().map(|zone| Some(zone.id)))
            .collect();
        *self.imp().zone_ids.borrow() != current
    }

    // ── Appearance ───────────────────────────────────────────────────────

    fn build_appearance_group(&self) -> adw::PreferencesGroup {
        let group = w::group(
            "Appearance",
            "Light and dark are libadwaita's own, so the whole window follows — including \
             the accent colour chosen in GNOME Settings.",
        );

        let row = adw::ComboRow::builder()
            .title("Theme")
            .model(&gtk4::StringList::new(&[
                "Follow the system",
                "Light",
                "Dark",
            ]))
            .build();

        // Read the live scheme rather than the setting: the switcher in the
        // primary menu writes both, and the manager is the one that is
        // actually on screen.
        row.set_selected(theme_index(adw::StyleManager::default().color_scheme()));

        row.connect_selected_notify({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                let (scheme, name) = theme_for_index(row.selected());
                adw::StyleManager::default().set_color_scheme(scheme);
                page.state()
                    .update_settings(|settings| settings.theme = name.to_string());
            }
        });

        group.add(&row);
        self.imp().theme_row.replace(Some(row));
        group
    }

    // ── Refresh ──────────────────────────────────────────────────────────

    fn build_refresh_group(&self) -> adw::PreferencesGroup {
        let settings = self.state().settings();
        let group = w::group(
            "Refresh",
            "How often the tunnel list is fetched again on its own. Cloudflare's rate \
             limit is shared with your dashboard session, so a short interval is not \
             free: what this window spends, the browser tab beside it cannot.",
        );

        let mut intervals: Vec<u32> = REFRESH_INTERVALS
            .iter()
            .map(|(seconds, _)| *seconds)
            .collect();
        let names = gtk4::StringList::new(&[]);
        for (_, label) in REFRESH_INTERVALS {
            names.append(label);
        }

        // A value that is not on the list can only have come from an older
        // version or a hand-edited database. Showing it is better than
        // silently claiming one of the offered intervals is in force.
        if !intervals.contains(&settings.refresh_interval_secs) {
            intervals.push(settings.refresh_interval_secs);
            names.append(&format!("Every {} seconds", settings.refresh_interval_secs));
        }

        let row = adw::ComboRow::builder()
            .title("Refresh tunnels")
            .subtitle("A manual refresh is always available from the header bar.")
            .model(&names)
            .build();
        row.set_selected(position_of(&intervals, &settings.refresh_interval_secs));

        row.connect_selected_notify({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                let Some(seconds) = page
                    .imp()
                    .refresh_intervals
                    .borrow()
                    .get(row.selected() as usize)
                    .copied()
                else {
                    return;
                };
                page.state()
                    .update_settings(|settings| settings.refresh_interval_secs = seconds);

                // The timer was armed with the old interval and will not
                // notice on its own.
                if let Some(window) = page.root().and_downcast::<crate::ui::MainWindow>() {
                    window.restart_refresh_timer();
                }
            }
        });

        group.add(&row);
        self.imp().refresh_intervals.replace(intervals);
        self.imp().refresh_row.replace(Some(row));
        group
    }

    // ── Remote control defaults ──────────────────────────────────────────

    fn build_remote_group(&self) -> adw::PreferencesGroup {
        let settings = self.state().settings();
        let group = w::group(
            "Remote control defaults",
            "Only the values the Add Host form starts with. Remote control is entirely \
             optional — every Cloudflare feature works without it — and nothing on this \
             page connects to anything.",
        );

        let username = adw::EntryRow::builder()
            .title("Default SSH username")
            .build();
        username.set_text(&settings.default_ssh_username);
        username.connect_changed({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                let value = row.text().to_string();
                page.state()
                    .update_settings(|settings| settings.default_ssh_username = value);
            }
        });
        group.add(&username);

        let port = adw::SpinRow::with_range(1.0, 65535.0, 1.0);
        port.set_title("Default SSH port");
        port.set_value(f64::from(settings.default_ssh_port));
        port.connect_value_notify({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                let value = row.value().round().clamp(1.0, f64::from(u16::MAX)) as u16;
                page.state()
                    .update_settings(|settings| settings.default_ssh_port = value);
            }
        });
        group.add(&port);

        let unit = adw::EntryRow::builder()
            .title("Default systemd unit")
            .build();
        unit.set_text(&settings.default_service_unit);
        unit.connect_changed({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                page.validate_unit(row);
            }
        });
        group.add(&unit);

        let imp = self.imp();
        imp.username_row.replace(Some(username));
        imp.port_row.replace(Some(port));
        imp.unit_row.replace(Some(unit));
        imp.unit_error.replace(None);
        imp.remote_group.replace(Some(group.clone()));

        group
    }

    fn validate_unit(&self, row: &adw::EntryRow) {
        match validation::systemd_unit(&row.text()) {
            Ok(unit) => {
                row.remove_css_class("error");
                self.set_unit_error(None);
                // The normalised name is persisted — `cloudflared` becomes
                // `cloudflared.service` — but the entry is left exactly as
                // typed, because rewriting it would move the cursor.
                self.state()
                    .update_settings(|settings| settings.default_service_unit = unit);
            }
            Err(error) => {
                row.add_css_class("error");
                self.set_unit_error(Some(error.message()));
            }
        }
    }

    /// Shows or clears the message under the remote group. An invalid unit
    /// name is never persisted, so the previous default stays in force until
    /// this clears.
    fn set_unit_error(&self, message: Option<&str>) {
        let imp = self.imp();
        let Some(group) = imp.remote_group.borrow().clone() else {
            return;
        };

        if let Some(previous) = imp.unit_error.take() {
            group.remove(&previous);
        }

        if let Some(message) = message {
            let line = w::inline_message("dialog-warning-symbolic", message, "error");
            line.set_margin_top(12);
            group.add(&line);
            imp.unit_error.replace(Some(line));
        }
    }

    // ── Data and diagnostics ─────────────────────────────────────────────

    fn build_data_group(&self) -> adw::PreferencesGroup {
        let settings = self.state().settings();
        let group = w::group(
            "Data and diagnostics",
            "What is kept on this machine, and what to send to somebody who is helping \
             you.",
        );

        group.add(&w::copyable_row(
            "Database",
            &storage::db::database_path().display().to_string(),
        ));
        group.add(&w::property_row(
            "Schema version",
            &storage::SCHEMA_VERSION.to_string(),
        ));

        let names = gtk4::StringList::new(&[]);
        for (_, label) in LOG_LEVELS {
            names.append(label);
        }
        let row = adw::ComboRow::builder()
            .title("Log level")
            .subtitle(
                "Written to standard error, redacted, and kept in memory for the report below.",
            )
            .model(&names)
            .build();
        row.set_selected(log_level_index(&current_log_level(&settings.log_level)));

        row.connect_selected_notify({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                if page.imp().syncing.get() {
                    return;
                }
                let Some((name, _)) = LOG_LEVELS.get(row.selected() as usize) else {
                    return;
                };
                if logging::set_level(name) {
                    page.state()
                        .update_settings(|settings| settings.log_level = (*name).to_string());
                } else {
                    // Nothing installed a subscriber, so the level would be a
                    // claim rather than a fact.
                    w::toast(
                        row,
                        "Logging was not initialised in this session, so the level was not changed.",
                    );
                    page.sync();
                }
            }
        });
        group.add(&row);
        self.imp().log_row.replace(Some(row));

        let report = adw::ButtonRow::builder()
            .title("Copy Redacted Diagnostic Report")
            .start_icon_name("edit-copy-symbolic")
            .build();
        report.connect_activated({
            let page = self.downgrade();
            move |row| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.copy_report(row);
            }
        });
        group.add(&report);

        group
    }

    fn copy_report(&self, anchor: &impl IsA<gtk4::Widget>) {
        let anchor = anchor.as_ref();
        anchor.clipboard().set_text(&self.diagnostic_report());
        w::toast(
            anchor,
            "Diagnostic report copied — read it before you share it",
        );
        self.state().record(ActivityEvent::info(
            ActivityCategory::App,
            "Copied a redacted diagnostic report to the clipboard.",
        ));
    }

    /// Everything a bug report needs and nothing a credential could hide in.
    ///
    /// The whole thing goes through [`crate::domain::activity::redact`] at the
    /// end rather than field by field, so a value added here later is covered
    /// without anyone remembering to cover it.
    fn diagnostic_report(&self) -> String {
        let state = self.state();
        let settings = state.settings();
        let mut report = String::new();

        report.push_str(&format!(
            "{} {} diagnostic report\n",
            branding::APP_NAME,
            branding::VERSION
        ));
        report.push_str(&format!(
            "Generated: {}\n\n",
            w::format_exact_time(Some(chrono::Utc::now()))
        ));

        report.push_str("Storage\n");
        report.push_str(&format!(
            "  Database: {}\n",
            storage::db::database_path().display()
        ));
        report.push_str(&format!(
            "  Data directory: {}\n",
            storage::db::data_dir().display()
        ));
        report.push_str(&format!("  Schema version: {}\n", storage::SCHEMA_VERSION));
        report.push_str(&format!(
            "  Database opened: {}\n",
            yes_no(state.store().is_available())
        ));
        if let Some(problem) = state.storage_problem() {
            report.push_str(&format!("  Problem: {problem}\n"));
        }
        report.push_str(&format!(
            "  Credential store: {}\n",
            if state.secrets_available() {
                "available"
            } else {
                "unavailable"
            }
        ));
        report.push_str(&format!(
            "  API token saved: {}\n\n",
            yes_no(settings.token_saved)
        ));

        report.push_str("Session\n");
        report.push_str(&format!("  Connected: {}\n", yes_no(state.is_connected())));
        if let Some(session) = state.session() {
            // The short ID only: enough to tell two accounts apart in a bug
            // report, not enough to be worth pasting into a public issue.
            report.push_str(&format!("  Account: {}\n", session.account.short_id()));
            report.push_str(&format!("  Domains visible: {}\n", session.zones.len()));
            for (index, permission) in REQUIRED_PERMISSIONS.iter().enumerate() {
                report.push_str(&format!(
                    "  {} {} {}: {}\n",
                    permission.scope,
                    permission.dashboard_name,
                    permission.level,
                    session.capabilities.get(index).label()
                ));
            }
        }
        report.push_str(&format!("  Tunnels loaded: {}\n", state.tunnels().len()));
        report.push_str(&format!("  Registered hosts: {}\n\n", state.hosts().len()));

        report.push_str("Preferences\n");
        report.push_str(&format!("  Theme: {}\n", settings.theme));
        report.push_str(&format!(
            "  Refresh: {}\n",
            settings.refresh_interval_label()
        ));
        report.push_str(&format!(
            "  Log level: {}\n",
            current_log_level(&settings.log_level)
        ));
        report.push_str(&format!(
            "  Default SSH port: {}\n\n",
            settings.default_ssh_port
        ));

        report.push_str("Recent log\n");
        let lines = logging::recent_lines();
        if lines.is_empty() {
            report.push_str("  nothing buffered\n");
        }
        for line in lines {
            report.push_str(&format!("  {line}\n"));
        }

        crate::domain::activity::redact(&report)
    }

    // ── About ────────────────────────────────────────────────────────────

    fn build_about_group(&self) -> adw::PreferencesGroup {
        let group = w::group("About", "");

        // First in the group, because a third-party client that looks like a
        // first-party one is a trademark problem and a trust problem at once.
        let notice = adw::ActionRow::builder()
            .title("Not a Cloudflare product")
            .subtitle(branding::AFFILIATION_NOTICE)
            .build();
        notice.add_prefix(&gtk4::Image::from_icon_name("dialog-information-symbolic"));
        group.add(&notice);

        group.add(&w::property_row("Application", branding::APP_NAME));
        group.add(&w::property_row("Version", branding::VERSION));

        let about = adw::ButtonRow::builder()
            .title(format!("About {}", branding::APP_NAME))
            .start_icon_name("help-about-symbolic")
            .build();
        about.set_action_name(Some("app.about"));
        group.add(&about);

        group
    }

    // ── Keeping the widgets in step ──────────────────────────────────────

    /// Brings every widget back into step with the settings without
    /// rebuilding anything, so a notification raised by one keystroke cannot
    /// disturb the entry that raised it.
    fn sync(&self) {
        let imp = self.imp();
        if imp.syncing.get() {
            return;
        }
        imp.syncing.set(true);

        let state = self.state();
        let settings = state.settings();

        if let Some(banner) = imp.storage_banner.borrow().as_ref() {
            match state.storage_problem() {
                Some(problem) => {
                    banner.set_title(&problem);
                    banner.set_revealed(true);
                }
                None => banner.set_revealed(false),
            }
        }
        if let Some(banner) = imp.secrets_banner.borrow().as_ref() {
            banner.set_revealed(!state.secrets_available());
        }

        self.apply_token_state();
        if let Some(row) = imp.remove_token_row.borrow().as_ref() {
            row.set_sensitive(settings.token_saved && state.secrets_available());
        }

        if let Some(row) = imp.zone_row.borrow().as_ref() {
            let index = index_of(&imp.zone_ids.borrow(), &settings.default_zone_id);
            if row.selected() != index {
                row.set_selected(index);
            }
        }

        if let Some(row) = imp.theme_row.borrow().as_ref() {
            let index = theme_index_for_name(&settings.theme);
            if row.selected() != index {
                row.set_selected(index);
            }
        }

        if let Some(row) = imp.refresh_row.borrow().as_ref() {
            let index = position_of(
                &imp.refresh_intervals.borrow(),
                &settings.refresh_interval_secs,
            );
            if row.selected() != index {
                row.set_selected(index);
            }
        }

        if let Some(row) = imp.log_row.borrow().as_ref() {
            let index = log_level_index(&current_log_level(&settings.log_level));
            if row.selected() != index {
                row.set_selected(index);
            }
        }

        if let Some(row) = imp.username_row.borrow().as_ref() {
            if !is_being_edited(row) && row.text() != settings.default_ssh_username {
                row.set_text(&settings.default_ssh_username);
            }
        }
        if let Some(row) = imp.port_row.borrow().as_ref() {
            if !is_being_edited(row) && row.value().round() as u16 != settings.default_ssh_port {
                row.set_value(f64::from(settings.default_ssh_port));
            }
        }
        if let Some(row) = imp.unit_row.borrow().as_ref() {
            if !is_being_edited(row) && row.text() != settings.default_service_unit {
                row.set_text(&settings.default_service_unit);
            }
        }

        imp.syncing.set(false);
    }
}

/// Whether the user is inside this widget. GTK maintains the focus child all
/// the way up the tree, which is what makes the check work for the text
/// widget inside a row rather than the row itself.
fn is_being_edited(widget: &impl IsA<gtk4::Widget>) -> bool {
    let widget = widget.as_ref();
    widget.has_focus() || widget.focus_child().is_some()
}

fn index_of(ids: &[Option<String>], wanted: &Option<String>) -> u32 {
    ids.iter().position(|id| id == wanted).unwrap_or(0) as u32
}

fn position_of(values: &[u32], wanted: &u32) -> u32 {
    values.iter().position(|value| value == wanted).unwrap_or(0) as u32
}

fn theme_index(scheme: adw::ColorScheme) -> u32 {
    match scheme {
        adw::ColorScheme::ForceLight | adw::ColorScheme::PreferLight => 1,
        adw::ColorScheme::ForceDark | adw::ColorScheme::PreferDark => 2,
        _ => 0,
    }
}

fn theme_index_for_name(name: &str) -> u32 {
    match name {
        "light" => 1,
        "dark" => 2,
        _ => 0,
    }
}

fn theme_for_index(index: u32) -> (adw::ColorScheme, &'static str) {
    match index {
        1 => (adw::ColorScheme::ForceLight, "light"),
        2 => (adw::ColorScheme::ForceDark, "dark"),
        _ => (adw::ColorScheme::Default, "system"),
    }
}

/// The level actually in force, which is not the saved one when `RUST_LOG`
/// set something else at startup.
fn current_log_level(saved: &str) -> String {
    let live = logging::current_level();
    if live.is_empty() {
        saved.to_string()
    } else {
        live
    }
}

fn log_level_index(level: &str) -> u32 {
    LOG_LEVELS
        .iter()
        .position(|(name, _)| *name == level)
        .or_else(|| {
            LOG_LEVELS
                .iter()
                .position(|(name, _)| *name == logging::DEFAULT_LOG_LEVEL)
        })
        .unwrap_or(0) as u32
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
    }
}
