//! First run, and the disconnected state.
//!
//! This is the only screen shown until a token has been accepted, so it has to
//! carry the whole of the setup story: what the application is, what it is
//! not, what permissions to grant, and how far the user has got. It is built
//! as one scrolling page with progressive disclosure rather than a wizard,
//! because every step after the token is optional and a wizard implies they
//! are not.
//!
//! The token is a [`SecretString`] from the moment it leaves the entry. It is
//! never put in a label, never formatted into a message, and only written to
//! the OS credential store — never to the application's own database.

use std::cell::RefCell;

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use secrecy::SecretString;

use crate::branding;
use crate::cloudflare::Cancel;
use crate::domain::account::{Account, REQUIRED_PERMISSIONS};
use crate::domain::activity::{ActivityCategory, ActivityEvent};
use crate::domain::validation;
use crate::domain::Zone;
use crate::services::session::{self, ConnectionState, Session, TokenCheck};
use crate::services::{runtime, tunnels};
use crate::ui::state::AppState;
use crate::ui::widgets as w;

/// Where the user has got to. Each stage reveals the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Nothing entered yet.
    Token,
    /// A check is running.
    Checking,
    /// The token is good; an account has to be chosen.
    Account,
    /// An account is chosen; a zone may optionally be chosen.
    Zone,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct OnboardingPage {
        pub state: RefCell<Option<AppState>>,
        pub stage: std::cell::Cell<Option<super::Stage>>,

        pub token_row: RefCell<Option<adw::PasswordEntryRow>>,
        pub test_button: RefCell<Option<gtk4::Button>>,
        pub test_spinner: RefCell<Option<gtk4::Stack>>,
        pub result_banner: RefCell<Option<adw::Banner>>,

        pub account_group: RefCell<Option<adw::PreferencesGroup>>,
        pub account_combo: RefCell<Option<adw::ComboRow>>,
        pub account_manual: RefCell<Option<adw::EntryRow>>,
        pub accounts: RefCell<Vec<Account>>,

        pub zone_group: RefCell<Option<adw::PreferencesGroup>>,
        pub zone_combo: RefCell<Option<adw::ComboRow>>,
        pub zone_note: RefCell<Option<gtk4::Label>>,
        pub zones: RefCell<Vec<Zone>>,

        pub save_row: RefCell<Option<adw::SwitchRow>>,
        pub finish_group: RefCell<Option<adw::PreferencesGroup>>,
        pub finish_button: RefCell<Option<gtk4::Button>>,

        /// Held so a second Test Connection cancels the first.
        pub cancel: RefCell<Option<Cancel>>,
        /// The token that passed verification, kept only until Connect.
        pub verified_token: RefCell<Option<SecretString>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for OnboardingPage {
        const NAME: &'static str = "TunnelManagerOnboardingPage";
        type Type = super::OnboardingPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for OnboardingPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.stage.set(Some(Stage::Token));
        }

        fn dispose(&self) {
            // A page that goes away while a check is in flight must not leave
            // the request running against a dead widget.
            if let Some(cancel) = self.cancel.borrow().as_ref() {
                cancel.cancel();
            }
        }
    }

    impl WidgetImpl for OnboardingPage {}
    impl BinImpl for OnboardingPage {}
}

glib::wrapper! {
    pub struct OnboardingPage(ObjectSubclass<imp::OnboardingPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl OnboardingPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        // A saved token that Cloudflare has since rejected is reported here
        // rather than by a toast that would appear over an empty dashboard.
        state.subscribe(&page, {
            let page = page.downgrade();
            move |state, event| {
                if event != crate::ui::state::Event::Connection {
                    return;
                }
                let Some(page) = page.upgrade() else {
                    return;
                };
                if let ConnectionState::Failed { message, .. } = state.connection() {
                    page.banner(&message, Some("error"));
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
            .expect("the page is always built with a state")
    }

    fn build(&self) {
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(24)
            .margin_top(24)
            .margin_bottom(36)
            .margin_start(12)
            .margin_end(12)
            .build();

        column.append(&self.build_intro());
        column.append(&self.build_token_group());
        column.append(&self.build_permissions_group());
        column.append(&self.build_account_group());
        column.append(&self.build_zone_group());
        column.append(&self.build_finish_group());

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

        // The banner belongs above the scrolling content so it stays visible
        // while the user scrolls down to fix whatever it is complaining about.
        let banner = adw::Banner::builder().revealed(false).build();
        self.imp().result_banner.replace(Some(banner.clone()));

        let root = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        root.append(&w::wrappable(&banner));
        root.append(&scroller);

        self.set_child(Some(&root));
        self.apply_stage();
    }

    fn build_intro(&self) -> gtk4::Box {
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .build();

        let icon = gtk4::Image::from_icon_name(branding::APP_ID);
        icon.set_pixel_size(96);
        icon.set_margin_top(12);
        column.append(&icon);

        let title = gtk4::Label::builder()
            .label(format!("Connect {} to Cloudflare", branding::APP_NAME))
            .justify(gtk4::Justification::Center)
            .wrap(true)
            .build();
        title.add_css_class("title-1");
        column.append(&title);

        let body = gtk4::Label::builder()
            .label(
                "Paste a scoped API token to manage the Cloudflare Tunnels on your \
                 account. The token is stored in your desktop's credential store, not \
                 in this application's files.",
            )
            .justify(gtk4::Justification::Center)
            .wrap(true)
            .max_width_chars(56)
            .build();
        body.add_css_class("body");
        column.append(&body);

        let notice = gtk4::Label::builder()
            .label(branding::AFFILIATION_NOTICE)
            .justify(gtk4::Justification::Center)
            .wrap(true)
            .max_width_chars(56)
            .build();
        notice.add_css_class("caption");
        notice.add_css_class("dimmed");
        column.append(&notice);

        column
    }

    fn build_token_group(&self) -> adw::PreferencesGroup {
        let group = w::group(
            "API token",
            "Create one at Cloudflare → My Profile → API Tokens → Create Token, using \
             the permissions below.",
        );

        // AdwPasswordEntryRow hides the value and provides the show/hide peek
        // control itself; there is no separate "show token" button to get
        // wrong, and the entry refuses input methods that could retain text.
        let token_row = adw::PasswordEntryRow::builder()
            .title("Cloudflare API token")
            .build();
        group.add(&token_row);

        let test = adw::ButtonRow::builder()
            .title("Test Connection")
            .start_icon_name("network-transmit-receive-symbolic")
            .build();
        group.add(&test);

        // AdwButtonRow has no busy state of its own, so the plain button below
        // is what actually carries the spinner; the row activates it.
        let stack = gtk4::Stack::new();
        stack.add_named(&gtk4::Label::new(Some("Test Connection")), Some("idle"));
        let busy = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk4::Align::Center)
            .build();
        busy.append(
            &adw::Spinner::builder()
                .width_request(16)
                .height_request(16)
                .build(),
        );
        busy.append(&gtk4::Label::new(Some("Checking…")));
        stack.add_named(&busy, Some("busy"));

        let button = gtk4::Button::builder()
            .child(&stack)
            .halign(gtk4::Align::Center)
            .build();
        button.add_css_class("pill");
        button.add_css_class("suggested-action");

        {
            let page = self.downgrade();
            let run = move || {
                if let Some(page) = page.upgrade() {
                    page.test_connection();
                }
            };
            let run_clone = run.clone();
            button.connect_clicked(move |_| run());
            test.connect_activated(move |_| run_clone());
        }

        // Enter in the token field is the same as pressing the button.
        {
            let button = button.clone();
            token_row.connect_entry_activated(move |_| button.emit_clicked());
        }

        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_top(12)
            .build();
        wrapper.append(&button);
        group.add(&wrapper);

        // The row is redundant with the button; keep only the button.
        group.remove(&test);

        self.imp().token_row.replace(Some(token_row));
        self.imp().test_button.replace(Some(button));
        self.imp().test_spinner.replace(Some(stack));

        group
    }

    fn build_permissions_group(&self) -> adw::PreferencesGroup {
        let group = w::group(
            "Minimum recommended permissions",
            "Grant only these three. A token with more access than it needs is a \
             larger problem when it leaks.",
        );

        for permission in REQUIRED_PERMISSIONS {
            let row = adw::ActionRow::builder()
                .title(format!(
                    "{} → {} → {}",
                    permission.scope, permission.dashboard_name, permission.level
                ))
                .subtitle(permission.needed_for)
                .build();
            row.add_prefix(&gtk4::Image::from_icon_name("emblem-default-symbolic"));
            group.add(&row);
        }

        let note = adw::ActionRow::builder()
            .title("Account resources")
            .subtitle(
                "Under “Account Resources”, include the account whose tunnels you want \
                 to manage. Under “Zone Resources”, include the domains you will publish \
                 hostnames on.",
            )
            .build();
        note.add_prefix(&gtk4::Image::from_icon_name("dialog-information-symbolic"));
        group.add(&note);

        group
    }

    fn build_account_group(&self) -> adw::PreferencesGroup {
        let group = w::group(
            "Account",
            "Which Cloudflare account this application will manage.",
        );

        let combo = adw::ComboRow::builder().title("Cloudflare account").build();
        combo.set_use_subtitle(true);
        group.add(&combo);

        // Shown only when account discovery could not list any — an
        // account-scoped token authenticates fine and is refused by
        // `/accounts`, which is a perfectly normal way to scope a token.
        let manual = adw::EntryRow::builder()
            .title("Account ID")
            .visible(false)
            .build();
        manual.set_show_apply_button(false);
        group.add(&manual);

        {
            let page = self.downgrade();
            combo.connect_selected_notify(move |_| {
                if let Some(page) = page.upgrade() {
                    page.on_account_changed();
                }
            });
        }
        {
            let page = self.downgrade();
            manual.connect_changed(move |_| {
                if let Some(page) = page.upgrade() {
                    page.update_finish_sensitivity();
                }
            });
        }

        group.set_visible(false);
        self.imp().account_group.replace(Some(group.clone()));
        self.imp().account_combo.replace(Some(combo));
        self.imp().account_manual.replace(Some(manual));

        group
    }

    fn build_zone_group(&self) -> adw::PreferencesGroup {
        let group = w::group(
            "Default domain",
            "Optional. Sets which domain the hostname picker offers first. Every other \
             feature works without one.",
        );

        let combo = adw::ComboRow::builder().title("Domain").build();
        combo.set_use_subtitle(true);
        group.add(&combo);

        let note = gtk4::Label::builder()
            .halign(gtk4::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .visible(false)
            .build();
        note.add_css_class("caption");
        note.add_css_class("dimmed");

        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_top(6)
            .build();
        wrapper.append(&note);
        group.add(&wrapper);

        group.set_visible(false);
        self.imp().zone_group.replace(Some(group.clone()));
        self.imp().zone_combo.replace(Some(combo));
        self.imp().zone_note.replace(Some(note));

        group
    }

    fn build_finish_group(&self) -> adw::PreferencesGroup {
        let group = w::group("Finish", "");

        let save = adw::SwitchRow::builder()
            .title("Remember this token")
            .subtitle(
                "Saves it in your desktop's credential store so you do not have to \
                       paste it again.",
            )
            .active(true)
            .build();
        group.add(&save);

        let ssh = adw::ActionRow::builder()
            .title("Remote host setup is optional")
            .subtitle(
                "Registering a Linux host over SSH lets this application start and stop \
                 cloudflared for you. Everything else works without it, and you can add \
                 a host later from Remote Hosts.",
            )
            .build();
        ssh.add_prefix(&gtk4::Image::from_icon_name("network-server-symbolic"));
        group.add(&ssh);

        let button = gtk4::Button::builder()
            .label("Connect")
            .halign(gtk4::Align::Center)
            .sensitive(false)
            .build();
        button.add_css_class("pill");
        button.add_css_class("suggested-action");

        {
            let page = self.downgrade();
            button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.finish();
                }
            });
        }

        let wrapper = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .build();
        wrapper.append(&button);

        let skip = gtk4::Label::builder()
            .label("Skip SSH setup — you can register a host at any time.")
            .halign(gtk4::Align::Center)
            .wrap(true)
            .build();
        skip.add_css_class("caption");
        skip.add_css_class("dimmed");
        wrapper.append(&skip);

        group.add(&wrapper);
        group.set_visible(false);

        self.imp().save_row.replace(Some(save));
        self.imp().finish_group.replace(Some(group.clone()));
        self.imp().finish_button.replace(Some(button));

        group
    }

    // ── Behaviour ────────────────────────────────────────────────────────

    fn set_stage(&self, stage: Stage) {
        self.imp().stage.set(Some(stage));
        self.apply_stage();
    }

    fn apply_stage(&self) {
        let imp = self.imp();
        let stage = imp.stage.get().unwrap_or(Stage::Token);

        if let Some(stack) = imp.test_spinner.borrow().as_ref() {
            stack.set_visible_child_name(if stage == Stage::Checking {
                "busy"
            } else {
                "idle"
            });
        }
        if let Some(button) = imp.test_button.borrow().as_ref() {
            button.set_sensitive(stage != Stage::Checking);
        }
        if let Some(row) = imp.token_row.borrow().as_ref() {
            row.set_sensitive(stage != Stage::Checking);
        }

        let past_token = matches!(stage, Stage::Account | Stage::Zone);
        if let Some(group) = imp.account_group.borrow().as_ref() {
            group.set_visible(past_token);
        }
        if let Some(group) = imp.finish_group.borrow().as_ref() {
            group.set_visible(past_token);
        }
        if let Some(group) = imp.zone_group.borrow().as_ref() {
            group.set_visible(stage == Stage::Zone);
        }

        // The credential store may be absent — over SSH, in a bare container,
        // or with no keyring daemon. Saying so before the switch is used is
        // better than failing after Connect.
        if let Some(row) = imp.save_row.borrow().as_ref() {
            let available = self.state().secrets_available();
            row.set_sensitive(available);
            if !available {
                row.set_active(false);
                row.set_subtitle(
                    "No credential store is available on this session, so the token \
                     cannot be remembered. It will work for this session only.",
                );
            }
        }

        self.update_finish_sensitivity();
    }

    fn banner(&self, message: &str, css_class: Option<&str>) {
        let Some(banner) = self.imp().result_banner.borrow().clone() else {
            return;
        };

        banner.set_title(message);
        for class in ["error", "warning", "success"] {
            banner.remove_css_class(class);
        }
        if let Some(class) = css_class {
            banner.add_css_class(class);
        }
        banner.set_revealed(true);
    }

    fn test_connection(&self) {
        let imp = self.imp();

        let Some(row) = imp.token_row.borrow().clone() else {
            return;
        };

        let raw = row.text().to_string();
        let token = match validation::api_token(&raw) {
            Ok(token) => token,
            Err(error) => {
                self.banner(error.message(), Some("error"));
                return;
            }
        };

        // A second attempt cancels the first, so a slow request cannot land
        // after a newer one and overwrite its result.
        if let Some(previous) = imp.cancel.borrow().as_ref() {
            previous.cancel();
        }
        let cancel = Cancel::new();
        imp.cancel.replace(Some(cancel.clone()));

        let secret = SecretString::from(token);
        let client = match session::client_for(secret.clone(), &session::api_base_url()) {
            Ok(client) => client,
            Err(error) => {
                self.banner(&error.user_message(), Some("error"));
                return;
            }
        };

        self.set_stage(Stage::Checking);
        self.banner("Checking the token with Cloudflare…", None);

        let page = self.downgrade();
        runtime::spawn(
            async move { session::check_token(&client, &cancel).await },
            move |result| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.imp().verified_token.replace(Some(secret));
                page.on_token_checked(result);
            },
        );
    }

    fn on_token_checked(&self, result: crate::cloudflare::Result<TokenCheck>) {
        let state = self.state();

        match result {
            Err(error) if error.is_cancelled() => {
                self.set_stage(Stage::Token);
            }

            Err(error) => {
                self.imp().verified_token.replace(None);
                self.set_stage(Stage::Token);
                self.banner(&error.user_message(), Some("error"));
                state.record(
                    ActivityEvent::error(ActivityCategory::App, "Token check failed")
                        .with_detail(error.user_message()),
                );
            }

            Ok(check) => {
                let caveat = check.status.caveat();
                self.imp().accounts.replace(check.accounts.clone());
                self.populate_accounts(&check);

                match caveat {
                    Some(caveat) => {
                        self.banner(&format!("The token is valid. {caveat}"), Some("warning"))
                    }
                    None => self.banner("The token is valid.", Some("success")),
                }

                self.set_stage(Stage::Account);
                state.record(ActivityEvent::success(
                    ActivityCategory::App,
                    format!(
                        "API token verified; {} account(s) visible",
                        check.accounts.len()
                    ),
                ));

                // With exactly one account there is nothing to choose, so go
                // straight on to loading its zones.
                if check.accounts.len() == 1 {
                    self.on_account_changed();
                }
            }
        }
    }

    fn populate_accounts(&self, check: &TokenCheck) {
        let imp = self.imp();
        let Some(combo) = imp.account_combo.borrow().clone() else {
            return;
        };
        let Some(manual) = imp.account_manual.borrow().clone() else {
            return;
        };

        if check.needs_manual_account() {
            combo.set_visible(false);
            manual.set_visible(true);
            let reason = check
                .accounts_unavailable
                .clone()
                .unwrap_or_else(|| "No accounts were returned.".into());
            self.banner(
                &format!(
                    "{reason} Enter the account ID from the right-hand sidebar of the \
                     Cloudflare dashboard."
                ),
                Some("warning"),
            );
            return;
        }

        combo.set_visible(true);
        manual.set_visible(false);

        let names = gtk4::StringList::new(&[]);
        for account in &check.accounts {
            names.append(&account.name);
        }
        combo.set_model(Some(&names));

        // Reselect whatever was chosen last time, if it is still there.
        let saved = self.state().settings().account_id;
        if let Some(saved) = saved {
            if let Some(index) = check
                .accounts
                .iter()
                .position(|account| account.id == saved)
            {
                combo.set_selected(index as u32);
            }
        }
    }

    /// The account the user has settled on, from the combo or the manual field.
    fn selected_account(&self) -> Option<Account> {
        let imp = self.imp();
        let accounts = imp.accounts.borrow();

        if !accounts.is_empty() {
            let index = imp
                .account_combo
                .borrow()
                .as_ref()
                .map(|combo| combo.selected() as usize)?;
            return accounts.get(index).cloned();
        }

        let manual = imp.account_manual.borrow().clone()?;
        let id = validation::account_id(&manual.text()).ok()?;
        Some(Account {
            name: format!("Account {}", &id[..8.min(id.len())]),
            id,
        })
    }

    fn on_account_changed(&self) {
        let Some(account) = self.selected_account() else {
            self.update_finish_sensitivity();
            return;
        };

        if let Some(combo) = self.imp().account_combo.borrow().as_ref() {
            combo.set_subtitle(&account.short_id());
        }

        self.load_zones(account);
        self.update_finish_sensitivity();
    }

    fn load_zones(&self, account: Account) {
        let Some(token) = self.imp().verified_token.borrow().clone() else {
            return;
        };
        let Ok(client) = session::client_for(token, &session::api_base_url()) else {
            return;
        };

        let cancel = Cancel::new();
        self.imp().cancel.replace(Some(cancel.clone()));

        let page = self.downgrade();
        runtime::spawn(
            async move {
                let mut session = Session::new(client, account);
                let problem = session.load_zones(&cancel).await;
                (session, problem)
            },
            move |(session, problem)| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.on_zones_loaded(session, problem);
            },
        );
    }

    fn on_zones_loaded(&self, session: Session, problem: Option<String>) {
        let imp = self.imp();
        imp.zones.replace(session.zones.clone());

        let Some(combo) = imp.zone_combo.borrow().clone() else {
            return;
        };
        let note = imp.zone_note.borrow().clone();

        let names = gtk4::StringList::new(&["No default domain"]);
        for zone in &session.zones {
            names.append(&zone.name);
        }
        combo.set_model(Some(&names));

        if let Some(saved) = self.state().settings().default_zone_id {
            if let Some(index) = session.zones.iter().position(|zone| zone.id == saved) {
                combo.set_selected(index as u32 + 1);
            }
        }

        if let Some(note) = note {
            match &problem {
                Some(problem) => {
                    note.set_label(&format!(
                        "{problem} Tunnels can still be created and managed; publishing \
                         a hostname needs Zone: Read and DNS: Edit."
                    ));
                    note.set_visible(true);
                    combo.set_sensitive(false);
                }
                None if session.zones.is_empty() => {
                    note.set_label(
                        "This token can read zones, but the account has none in its \
                         scope. Hostname publishing will be unavailable until a domain \
                         is added to the token's zone resources.",
                    );
                    note.set_visible(true);
                    combo.set_sensitive(false);
                }
                None => {
                    note.set_label(&format!(
                        "{} available. This only sets which one is offered first.",
                        w::plural(session.zones.len(), "domain", "domains")
                    ));
                    note.set_visible(true);
                    combo.set_sensitive(true);
                }
            }
        }

        self.set_stage(Stage::Zone);
    }

    fn selected_zone(&self) -> Option<Zone> {
        let imp = self.imp();
        let combo = imp.zone_combo.borrow().clone()?;
        let index = combo.selected() as usize;
        if index == 0 {
            return None;
        }
        imp.zones.borrow().get(index - 1).cloned()
    }

    fn update_finish_sensitivity(&self) {
        let Some(button) = self.imp().finish_button.borrow().clone() else {
            return;
        };
        button.set_sensitive(self.selected_account().is_some());
    }

    fn finish(&self) {
        let imp = self.imp();

        let (Some(account), Some(token)) =
            (self.selected_account(), imp.verified_token.borrow().clone())
        else {
            return;
        };

        let zone = self.selected_zone();
        let remember = imp
            .save_row
            .borrow()
            .as_ref()
            .map(|row| row.is_active())
            .unwrap_or(false);

        let state = self.state();

        let Ok(client) = session::client_for(token.clone(), &session::api_base_url()) else {
            self.banner("Could not build an HTTP client.", Some("error"));
            return;
        };

        let mut live = Session::new(client, account.clone());
        live.zones = imp.zones.borrow().clone();
        live.default_zone_id = zone.as_ref().map(|zone| zone.id.clone());

        state.update_settings(|settings| {
            settings.account_id = Some(account.id.clone());
            settings.account_name = Some(account.name.clone());
            settings.default_zone_id = zone.as_ref().map(|zone| zone.id.clone());
            settings.default_zone_name = zone.as_ref().map(|zone| zone.name.clone());
        });

        state.set_session(Some(live));
        state.set_connection(ConnectionState::Connected {
            account: account.clone(),
            token: crate::cloudflare::models::TokenStatus {
                id: None,
                status: "active".into(),
                expires_on: None,
            },
            capabilities: Default::default(),
        });

        state.record(
            ActivityEvent::success(
                ActivityCategory::App,
                format!("Connected to Cloudflare account “{}”", account.name),
            )
            .with_target(account.name.clone()),
        );

        if remember {
            state.save_token(token, |_| {});
        }

        // Kick off the first tunnel load so the dashboard is not empty when it
        // appears.
        refresh_tunnels(&state);
    }
}

/// Reconnects with a saved token, at startup.
///
/// Without this the user would be asked to paste a token they already told the
/// application to remember, which would make "Remember this token" a lie. It
/// runs only when both halves are present — a saved token *and* a chosen
/// account — because either alone leaves the connection page as the right
/// screen to be on.
pub fn restore_session(state: &AppState) {
    if !state.pending_restore() {
        return;
    }
    let settings = state.settings();
    let (Some(account_id), Some(account_name)) =
        (settings.account_id.clone(), settings.account_name.clone())
    else {
        return;
    };

    state.set_busy("connect", true);
    let state_clone = state.clone();

    runtime::spawn_blocking(crate::storage::secrets::load_api_token, move |result| {
        if !matches!(state_clone.connection(), ConnectionState::Connecting) {
            return;
        }

        let token = match result {
            Ok(Some(token)) => token,
            Ok(None) => {
                // The token was removed from the credential store by
                // something else. Say so rather than silently showing the
                // connection page with no explanation.
                state_clone.set_busy("connect", false);
                state_clone.update_settings(|settings| settings.token_saved = false);
                state_clone.set_connection(ConnectionState::Disconnected);
                state_clone.record(ActivityEvent::warning(
                    ActivityCategory::App,
                    "The saved API token is no longer in the credential store.",
                ));
                return;
            }
            Err(error) => {
                state_clone.set_busy("connect", false);
                state_clone.set_secrets_available(false);
                state_clone.set_connection(ConnectionState::Disconnected);
                state_clone.record(ActivityEvent::warning(
                    ActivityCategory::App,
                    error.user_message(),
                ));
                return;
            }
        };

        let Ok(client) = session::client_for(token, &session::api_base_url()) else {
            state_clone.set_busy("connect", false);
            state_clone.set_connection(ConnectionState::Disconnected);
            return;
        };

        let account = Account {
            id: account_id.clone(),
            name: account_name.clone(),
        };
        let cancel = Cancel::new();
        let state_inner = state_clone.clone();
        let default_zone = state_clone.settings().default_zone_id.clone();

        runtime::spawn(
            async move {
                let check = session::check_token(&client, &cancel).await;
                let mut live = Session::new(client, account);
                live.default_zone_id = default_zone;
                let zone_problem = live.load_zones(&cancel).await;
                (check, live, zone_problem)
            },
            move |(check, live, zone_problem)| {
                if !matches!(state_inner.connection(), ConnectionState::Connecting) {
                    return;
                }
                state_inner.set_busy("connect", false);

                match check {
                    Ok(check) => {
                        let account = live.account.clone();
                        state_inner.set_session(Some(live));
                        state_inner.set_connection(ConnectionState::Connected {
                            account: account.clone(),
                            token: check.status,
                            capabilities: Default::default(),
                        });
                        state_inner.record(ActivityEvent::info(
                            ActivityCategory::App,
                            format!("Reconnected to “{}”", account.name),
                        ));
                        if let Some(problem) = zone_problem {
                            state_inner
                                .record(ActivityEvent::warning(ActivityCategory::Api, problem));
                        }
                        refresh_tunnels(&state_inner);
                    }
                    Err(error) => {
                        // A saved token that no longer works leaves the
                        // application on the connection page with the
                        // reason, rather than on an empty dashboard.
                        state_inner.record(ActivityEvent::error(
                            ActivityCategory::App,
                            format!("Saved token rejected: {}", error.user_message()),
                        ));
                        state_inner.set_connection(ConnectionState::Failed {
                            title: error.title().to_string(),
                            message: error.user_message(),
                        });
                    }
                }
            },
        );
    });
}

/// Loads the account's tunnels into the shared state.
///
/// Lives here rather than on a page because three different places start it —
/// finishing onboarding, the refresh button, and the automatic timer — and all
/// three want exactly the same behaviour.
pub fn refresh_tunnels(state: &AppState) {
    let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
        return;
    };
    if state.is_busy("tunnels") {
        return;
    }

    state.set_busy("tunnels", true);
    let cancel = Cancel::new();
    let client = session.client.clone();

    let state_clone = state.clone();
    runtime::spawn(
        async move { tunnels::list(&client, &account_id, &cancel).await },
        move |result| {
            state_clone.set_busy("tunnels", false);
            match result {
                Ok(rows) => {
                    let count = rows.len();
                    state_clone.set_tunnels(rows);
                    if let Some(mut session) = state_clone.session() {
                        session.note_tunnel_access(true);
                        state_clone.set_session(Some(session));
                    }
                    tracing::debug!(count, "tunnel list refreshed");
                    refresh_hostname_counts(&state_clone);
                }
                Err(error) if error.is_cancelled() => {}
                Err(error) => {
                    if error.is_permission_problem() {
                        if let Some(mut session) = state_clone.session() {
                            session.note_tunnel_access(false);
                            state_clone.set_session(Some(session));
                        }
                    }
                    state_clone.set_tunnels_error(error.user_message());
                    state_clone.record(
                        ActivityEvent::error(ActivityCategory::Api, "Could not list tunnels")
                            .with_detail(error.user_message()),
                    );
                }
            }
        },
    );
}

/// Fills in each tunnel's hostname count, one request per tunnel.
///
/// Deliberately after the list has already been shown: the count needs a
/// separate call per tunnel and waiting for all of them before showing
/// anything would make a ten-tunnel account feel broken.
fn refresh_hostname_counts(state: &AppState) {
    let (Some(session), Some(account_id)) = (state.session(), state.account_id()) else {
        return;
    };

    for row in state.tunnels() {
        if row.hostname_count.is_some() || !row.tunnel.config_source.is_remotely_managed() {
            continue;
        }

        let client = session.client.clone();
        let account_id = account_id.clone();
        let tunnel = row.tunnel.clone();
        let state = state.clone();
        let cancel = Cancel::new();

        runtime::spawn(
            async move {
                let count =
                    tunnels::load_hostname_count(&client, &account_id, &tunnel, &cancel).await;
                (tunnel.id.clone(), count)
            },
            move |(id, count)| {
                if let (Some(mut row), Some(count)) = (state.tunnel(&id), count) {
                    row.hostname_count = Some(count);
                    state.update_tunnel(row);
                }
            },
        );
    }
}
