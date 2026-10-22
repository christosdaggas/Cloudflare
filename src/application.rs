//! The `GtkApplication` subclass: global actions, the stylesheet, and the
//! dialogs that belong to the application rather than to a window.

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use std::cell::RefCell;

use crate::release_notes::RELEASES;
use crate::ui::MainWindow;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TunnelManagerApplication {
        pub window: RefCell<Option<MainWindow>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TunnelManagerApplication {
        const NAME: &'static str = "TunnelManagerApplication";
        type Type = super::TunnelManagerApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for TunnelManagerApplication {}

    impl ApplicationImpl for TunnelManagerApplication {
        fn startup(&self) {
            self.parent_startup();
            let app = self.obj();
            app.load_stylesheet();
            app.setup_actions();
        }

        fn activate(&self) {
            self.parent_activate();
            let app = self.obj();

            // Presenting an existing window rather than building a second one
            // is what makes the app single-instance in practice.
            if let Some(window) = self.window.borrow().as_ref() {
                window.present();
                return;
            }

            let state = crate::ui::AppState::load();
            let window = MainWindow::new(&app, &state);
            window.present();
            self.window.replace(Some(window));

            // After the window exists, so the reconnection's progress and its
            // failure both have somewhere to show.
            crate::ui::restore_session(&state);
        }
    }

    impl GtkApplicationImpl for TunnelManagerApplication {}
    impl AdwApplicationImpl for TunnelManagerApplication {}
}

glib::wrapper! {
    pub struct TunnelManagerApplication(ObjectSubclass<imp::TunnelManagerApplication>)
        @extends gio::Application, gtk4::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl Default for TunnelManagerApplication {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelManagerApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", crate::branding::APP_ID)
            .property("flags", gio::ApplicationFlags::empty())
            .build()
    }

    /// Loads `data/style.css` at the application priority, which sits above
    /// the Adwaita theme but below any user stylesheet.
    fn load_stylesheet(&self) {
        let provider = gtk4::CssProvider::new();
        provider.load_from_resource(&crate::branding::resource("style.css"));

        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }

    fn setup_actions(&self) {
        let about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| app.show_about_dialog())
            .build();

        let whats_new = gio::ActionEntry::builder("whats-new")
            .activate(|app: &Self, _, _| app.show_whats_new_dialog())
            .build();

        let shortcuts = gio::ActionEntry::builder("shortcuts")
            .activate(|app: &Self, _, _| app.show_shortcuts_dialog())
            .build();

        let quit = gio::ActionEntry::builder("quit")
            .activate(|app: &Self, _, _| app.quit())
            .build();

        // Lets the running application be driven from outside over its
        // existing D-Bus action group, which is how the screenshots in
        // README.md are produced and how a smoke test would open each page.
        let page = gio::ActionEntry::builder("page")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|app: &Self, _, parameter| {
                let Some(name) = parameter.and_then(glib::Variant::str) else {
                    return;
                };
                if let Some(window) = app.imp().window.borrow().as_ref() {
                    if !window.show_named_page(name) {
                        window
                            .state()
                            .record(crate::domain::activity::ActivityEvent::warning(
                                crate::domain::activity::ActivityCategory::App,
                                format!("Unknown page requested over D-Bus: {name}"),
                            ));
                        crate::ui::widgets::sticky_toast(
                            window,
                            &format!("There is no page named {name}."),
                            None,
                        );
                    }
                    window.present();
                }
            })
            .build();

        let sidebar = gio::ActionEntry::builder("sidebar")
            .activate(|app: &Self, _, _| {
                if let Some(window) = app.imp().window.borrow().as_ref() {
                    window.toggle_sidebar();
                }
            })
            .build();

        self.add_action_entries([about, whats_new, shortcuts, quit, page, sidebar]);

        // Accelerators are what make an action discoverable from the keyboard;
        // AdwShortcutsDialog below documents the same list for the mouse.
        self.set_accels_for_action("app.quit", &["<Control>q"]);
        self.set_accels_for_action("app.shortcuts", &["<Control>question"]);
        self.set_accels_for_action("win.toggle-rail", &["F9"]);
        self.set_accels_for_action("win.refresh", &["<Control>r"]);
        self.set_accels_for_action("win.create-tunnel", &["<Control>n"]);
        self.set_accels_for_action("window.close", &["<Control>w"]);
    }

    fn parent_widget(&self) -> Option<gtk4::Widget> {
        self.active_window().map(|window| window.upcast())
    }

    /// `AdwAboutDialog`, not `GtkAboutDialog`: it is adaptive, it presents
    /// inside the window instead of as a second toplevel, and it is what every
    /// current GNOME application uses.
    fn show_about_dialog(&self) {
        let dialog = adw::AboutDialog::builder()
            .application_name(crate::branding::APP_NAME)
            .application_icon(crate::branding::APP_ID)
            .version(crate::branding::VERSION)
            .developer_name("Christos A. Daggas")
            .copyright("© 2026 Christos A. Daggas")
            .license_type(gtk4::License::MitX11)
            .website("https://chrisdaggas.com")
            .issue_url("https://github.com/christosdaggas/Cloudflare/issues")
            .support_url("https://chrisdaggas.com")
            .comments(format!(
                "A native GNOME client for managing remotely configured Cloudflare \
                 Tunnels: their hostnames, their DNS records and — optionally, over \
                 SSH — the cloudflared service on the machines that serve them.\n\n{}",
                crate::branding::AFFILIATION_NOTICE
            ))
            .developers(vec!["Christos A. Daggas"])
            .build();

        dialog.add_link(
            "Cloudflare Tunnel documentation",
            "https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/",
        );
        dialog.add_link(
            "How to create a scoped API token",
            "https://developers.cloudflare.com/fundamentals/api/get-started/create-token/",
        );
        dialog.add_link("Contact", "mailto:info@chrisdaggas.com");
        // Named in the About dialog because the disclaimer has to be reachable
        // from inside the application, not only from its README.
        dialog.set_debug_info(crate::branding::AFFILIATION_NOTICE);
        dialog.present(self.parent_widget().as_ref());
    }

    fn show_whats_new_dialog(&self) {
        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(18)
            .build();

        for (index, release) in RELEASES.iter().enumerate() {
            if index > 0 {
                content.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
            }

            let heading = gtk4::Label::builder()
                .label(format!("Version {}", release.version))
                .halign(gtk4::Align::Start)
                .css_classes(["title-4"])
                .build();
            content.append(&heading);

            let date = gtk4::Label::builder()
                .label(release.date)
                .halign(gtk4::Align::Start)
                .css_classes(["caption", "dimmed"])
                .build();
            content.append(&date);

            for change in release.changes {
                let row = gtk4::Box::new(gtk4::Orientation::Horizontal, 12);
                row.set_valign(gtk4::Align::Start);

                let bullet = gtk4::Label::builder()
                    .label("•")
                    .valign(gtk4::Align::Start)
                    .css_classes(["accent"])
                    .build();
                row.append(&bullet);

                let text = gtk4::Label::builder()
                    .label(*change)
                    .halign(gtk4::Align::Start)
                    .xalign(0.0)
                    .wrap(true)
                    .hexpand(true)
                    .build();
                row.append(&text);

                content.append(&row);
            }
        }

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(420)
            .child(&content)
            .build();

        // AdwAlertDialog replaces the deprecated AdwMessageDialog.
        let dialog = adw::AlertDialog::new(Some("What's New"), None);
        dialog.set_extra_child(Some(&scrolled));
        dialog.add_response("close", "Close");
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.present(self.parent_widget().as_ref());
    }

    /// `AdwShortcutsDialog` (libadwaita 1.8) replaces `GtkShortcutsWindow`,
    /// which GTK deprecated in 4.18.
    fn show_shortcuts_dialog(&self) {
        let general = adw::ShortcutsSection::new(Some("General"));
        general.add(adw::ShortcutsItem::new(
            "Show keyboard shortcuts",
            "<Control>question",
        ));
        general.add(adw::ShortcutsItem::new("Quit", "<Control>q"));

        let navigation = adw::ShortcutsSection::new(Some("Navigation"));
        navigation.add(adw::ShortcutsItem::new(
            "Collapse or expand the sidebar",
            "F9",
        ));
        navigation.add(adw::ShortcutsItem::new("Close the window", "<Control>w"));

        let actions = adw::ShortcutsSection::new(Some("Actions"));
        actions.add(adw::ShortcutsItem::new("Refresh everything", "<Control>r"));
        actions.add(adw::ShortcutsItem::new("Create a tunnel", "<Control>n"));

        let dialog = adw::ShortcutsDialog::new();
        dialog.add(general);
        dialog.add(navigation);
        dialog.add(actions);
        dialog.present(self.parent_widget().as_ref());
    }
}
