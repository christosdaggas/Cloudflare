//! The application shell.
//!
//! The layout is the one this project started from, evolved rather than
//! replaced: an `AdwOverlaySplitView` with a left sidebar that collapses to an
//! 80px rail, two header bars sharing the window controls between them, and a
//! breakpoint that turns the sidebar into an overlay on a narrow window.
//! Widths are hidden before they are narrowed, stacks are never homogeneous,
//! and anything that can wrap sits inside a scroller — the three habits this
//! window's layout depends on.
//!
//! Two things changed for the product.
//!
//! **The content side is an `AdwNavigationView`, not a view stack.** Each
//! product page is an `AdwNavigationPage` with its own `AdwToolbarView` and
//! `AdwHeaderBar`, so a page carries its own actions — search, filters,
//! "Create Tunnel" — instead of the shell guessing which of them to show. It
//! also means the tunnel detail page can be `push`ed and gets a back button
//! for free. A navigation view shows one page at a time, so the homogeneity
//! trap that used to force a 986px minimum does not apply to it.
//!
//! **There is a top-level stack above the split view.** Until a token has been
//! accepted the whole window is the connection page; there is nothing useful
//! for a sidebar to navigate to before then.

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::branding;
use crate::services::session::ConnectionState;
use crate::theme;
use crate::ui::state::{AppState, Event};

/// The size the window opens at.
const WINDOW_WIDTH: i32 = 1050;
const WINDOW_HEIGHT: i32 = 720;
/// Sidebar widths, in pixels.
const SIDEBAR_WIDTH: f64 = 260.0;
const RAIL_WIDTH: f64 = 80.0;

/// The product pages, in sidebar order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavItem {
    Dashboard,
    Tunnels,
    Zones,
    Hostnames,
    Hosts,
    Activity,
    Settings,
}

impl NavItem {
    pub const ALL: &'static [NavItem] = &[
        Self::Dashboard,
        Self::Tunnels,
        Self::Zones,
        Self::Hostnames,
        Self::Hosts,
        Self::Activity,
        Self::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Tunnels => "Tunnels",
            Self::Zones => "Zones",
            Self::Hostnames => "Hostnames",
            Self::Hosts => "Remote Hosts",
            Self::Activity => "Activity",
            Self::Settings => "Settings",
        }
    }

    /// Named symbolic icons only. A symbolic icon recolours itself to the
    /// current foreground, which is what keeps the sidebar legible in dark
    /// mode and correct under a selected row.
    pub fn icon_name(self) -> &'static str {
        match self {
            Self::Dashboard => "view-grid-symbolic",
            Self::Tunnels => "network-vpn-symbolic",
            Self::Zones => "web-browser-symbolic",
            Self::Hostnames => "network-transmit-receive-symbolic",
            Self::Hosts => "network-server-symbolic",
            Self::Activity => "document-open-recent-symbolic",
            Self::Settings => "preferences-system-symbolic",
        }
    }

    pub fn stack_name(self) -> &'static str {
        match self {
            Self::Dashboard => "dashboard",
            Self::Tunnels => "tunnels",
            Self::Zones => "zones",
            Self::Hostnames => "hostnames",
            Self::Hosts => "hosts",
            Self::Activity => "activity",
            Self::Settings => "settings",
        }
    }

    pub fn from_stack_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|item| item.stack_name() == name)
    }
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MainWindow {
        pub state: RefCell<Option<AppState>>,

        pub root_stack: RefCell<Option<gtk4::Stack>>,
        pub split_view: RefCell<Option<adw::OverlaySplitView>>,
        pub navigation: RefCell<Option<adw::NavigationView>>,
        pub pages: RefCell<HashMap<&'static str, adw::NavigationPage>>,

        pub nav_list: RefCell<Option<gtk4::ListBox>>,
        pub nav_labels: RefCell<Vec<gtk4::Label>>,
        pub nav_boxes: RefCell<Vec<gtk4::Box>>,
        pub sidebar_title: RefCell<Option<adw::WindowTitle>>,
        pub rail_button: RefCell<Option<gtk4::Button>>,
        pub sidebar_header: RefCell<Option<adw::HeaderBar>>,
        /// Whether the rail button currently sits in the header's title slot.
        pub rail_button_centred: Cell<bool>,
        /// One sidebar toggle per page header, kept so the breakpoint can show
        /// them all at once.
        pub sidebar_buttons: RefCell<Vec<gtk4::ToggleButton>>,
        /// The narrow breakpoint, kept so headers created after setup (the
        /// tunnel detail page) can register their own sidebar toggle.
        pub narrow_breakpoint: RefCell<Option<adw::Breakpoint>>,
        /// The labelled header buttons, kept so the breakpoint can blank their
        /// labels on a narrow window.
        pub header_labels: RefCell<Vec<adw::ButtonContent>>,
        /// Header controls that are duplicated in the primary menu and can
        /// therefore be dropped entirely when space runs out.
        pub header_extras: RefCell<Vec<gtk4::Widget>>,

        /// What the user asked for with the rail toggle.
        pub rail: Cell<bool>,
        /// Whether the breakpoint has collapsed the split view.
        pub collapsed: Cell<bool>,
        /// Guards against the sidebar selection and the navigation view
        /// driving each other in a loop.
        pub navigating: Cell<bool>,

        pub refresh_timer: RefCell<Option<glib::SourceId>>,
        /// The status-bar update notice, hidden until the version check says
        /// a newer release exists.
        pub update_banner: RefCell<Option<gtk4::Box>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainWindow {
        const NAME: &'static str = "TunnelManagerMainWindow";
        type Type = super::MainWindow;
        type ParentType = adw::ApplicationWindow;
    }

    impl ObjectImpl for MainWindow {
        fn dispose(&self) {
            if let Some(source) = self.refresh_timer.take() {
                source.remove();
            }
        }
    }

    impl WidgetImpl for MainWindow {}
    impl WindowImpl for MainWindow {}
    impl ApplicationWindowImpl for MainWindow {}
    impl AdwApplicationWindowImpl for MainWindow {}
}

glib::wrapper! {
    pub struct MainWindow(ObjectSubclass<imp::MainWindow>)
        @extends gtk4::Widget, gtk4::Window, gtk4::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk4::Accessible, gtk4::Buildable,
                    gtk4::ConstraintTarget, gtk4::Native, gtk4::Root, gtk4::ShortcutManager;
}

impl MainWindow {
    pub fn new(app: &crate::application::TunnelManagerApplication, state: &AppState) -> Self {
        let window: Self = glib::Object::builder()
            .property("application", app)
            .property("title", branding::APP_NAME)
            .property("default-width", WINDOW_WIDTH)
            .property("default-height", WINDOW_HEIGHT)
            .build();

        window.imp().state.replace(Some(state.clone()));
        window.build_ui();
        window.setup_actions();
        window.apply_saved_theme();
        window.start_refresh_timer();

        state.subscribe(&window, {
            let window = window.downgrade();
            move |_, event| {
                if !matches!(event, Event::Connection) {
                    return;
                }
                if let Some(window) = window.upgrade() {
                    window.apply_connection_state();
                }
            }
        });

        window
    }

    pub fn state(&self) -> AppState {
        self.imp()
            .state
            .borrow()
            .clone()
            .expect("every page is constructed with a state before it can be shown")
    }

    fn build_ui(&self) {
        let imp = self.imp();

        let split_view = adw::OverlaySplitView::builder()
            .min_sidebar_width(SIDEBAR_WIDTH)
            .max_sidebar_width(SIDEBAR_WIDTH)
            .sidebar_width_fraction(0.25)
            .build();

        split_view.set_sidebar(Some(&self.build_sidebar()));
        split_view.set_content(Some(&self.build_navigation(&split_view)));

        // Both halves of the application live in one stack: the connection
        // page while disconnected, the shell once a token has been accepted.
        // Homogeneity off for the same reason a view stack needs it — the
        // larger child must not set the window's minimum size.
        let root_stack = gtk4::Stack::new();
        root_stack.set_hhomogeneous(false);
        root_stack.set_vhomogeneous(false);
        root_stack.set_transition_type(gtk4::StackTransitionType::Crossfade);
        root_stack.set_transition_duration(theme::transition_duration(self, 200));
        root_stack.add_named(&self.build_onboarding(), Some("onboarding"));
        root_stack.add_named(&split_view, Some("main"));

        // The toast overlay sits above everything so a toast floats over the
        // whole window rather than over one pane.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&root_stack));
        self.set_content(Some(&toast_overlay));

        imp.split_view.replace(Some(split_view));
        imp.root_stack.replace(Some(root_stack));

        self.add_narrow_breakpoint();
        self.sync_scheme_class();
        self.apply_connection_state();
        self.select(NavItem::Dashboard);
        self.check_for_updates();
    }

    /// Keeps `.scheme-light` / `.scheme-dark` on the window in step with the
    /// current colour scheme.
    ///
    /// This exists so that scheme-dependent colours can live in `style.css`
    /// rather than in code. GTK only evaluates `@media (prefers-color-scheme)`
    /// for the theme's own stylesheet, so an application stylesheet needs a
    /// class to hang its light and dark values off instead.
    fn sync_scheme_class(&self) {
        theme::track_appearance(self, |window| {
            let manager = adw::StyleManager::default();
            let dark = manager.is_dark();
            let high_contrast = manager.is_high_contrast();

            // add/remove, never `set_css_classes`: that replaces the whole
            // list, including the `csd` and `background` classes GTK puts on
            // a client-side-decorated window to draw its rounded corners and
            // shadow. Clobbering them squares off the window.
            let (add, remove) = if dark {
                ("scheme-dark", "scheme-light")
            } else {
                ("scheme-light", "scheme-dark")
            };
            window.remove_css_class(remove);
            window.add_css_class(add);
            // High contrast gets its own class so scheme-dependent tokens
            // can fall back to stock surfaces there (see style.css).
            if high_contrast {
                window.add_css_class("high-contrast");
            } else {
                window.remove_css_class("high-contrast");
            }
        });
    }

    // ── Onboarding ───────────────────────────────────────────────────────

    fn build_onboarding(&self) -> adw::ToolbarView {
        let header = adw::HeaderBar::builder()
            .title_widget(&adw::WindowTitle::new(branding::APP_NAME, "Not connected"))
            .build();
        header.pack_end(
            &gtk4::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Main Menu")
                .primary(true)
                .popover(&self.build_primary_menu())
                .build(),
        );

        let view = adw::ToolbarView::builder()
            .content(&crate::ui::OnboardingPage::new(&self.state()))
            .css_classes(["app-content"])
            .build();
        view.add_top_bar(&header);
        view
    }

    /// Switches between the connection page and the shell.
    ///
    /// A silent reconnect in progress (`Connecting`) shows the shell, not the
    /// connection page: there is a saved token to try, so asking for one
    /// would be wrong even though the connection is not live yet.
    fn apply_connection_state(&self) {
        let Some(stack) = self.imp().root_stack.borrow().clone() else {
            return;
        };

        let state = self.state();
        let show_main =
            state.is_connected() || matches!(state.connection(), ConnectionState::Connecting);
        stack.set_visible_child_name(if show_main { "main" } else { "onboarding" });
    }

    // ── Sidebar ──────────────────────────────────────────────────────────

    fn build_sidebar(&self) -> adw::ToolbarView {
        let imp = self.imp();

        let sidebar_title = adw::WindowTitle::new(branding::APP_NAME, "");

        // Collapses the sidebar to the 80px rail. Deliberately separate from
        // the breakpoint-driven overlay collapse: the rail is a manual
        // preference, the overlay is what a narrow window does on its own.
        let rail_button = gtk4::Button::builder()
            .icon_name("sidebar-show-symbolic")
            .tooltip_text("Collapse sidebar")
            .valign(gtk4::Align::Center)
            .action_name("win.toggle-rail")
            .css_classes(["flat"])
            .build();
        rail_button.update_property(&[gtk4::accessible::Property::Label("Collapse sidebar")]);

        let header = adw::HeaderBar::builder()
            .title_widget(&sidebar_title)
            .build();
        header.pack_end(&rail_button);

        let nav_list = gtk4::ListBox::builder()
            .selection_mode(gtk4::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();

        let mut labels = Vec::new();
        let mut boxes = Vec::new();
        for item in NavItem::ALL {
            let (row, label, row_box) = Self::build_nav_row(*item);
            nav_list.append(&row);
            labels.push(label);
            boxes.push(row_box);
        }

        let window = self.downgrade();
        nav_list.connect_row_selected(move |_, row| {
            let (Some(window), Some(row)) = (window.upgrade(), row) else {
                return;
            };
            if let Some(item) = NavItem::ALL.get(row.index() as usize) {
                window.show_page(*item);
            }
        });

        let scrolled = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .child(&nav_list)
            .build();

        imp.nav_list.replace(Some(nav_list));
        imp.nav_labels.replace(labels);
        imp.nav_boxes.replace(boxes);
        imp.sidebar_title.replace(Some(sidebar_title));
        imp.rail_button.replace(Some(rail_button));
        imp.sidebar_header.replace(Some(header.clone()));

        let view = adw::ToolbarView::builder().content(&scrolled).build();
        view.add_top_bar(&header);
        view
    }

    fn build_nav_row(item: NavItem) -> (gtk4::ListBoxRow, gtk4::Label, gtk4::Box) {
        let box_ = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .css_classes(["nav-row"])
            .build();

        let icon = gtk4::Image::from_icon_name(item.icon_name());
        icon.set_pixel_size(16);
        box_.append(&icon);

        // Ellipsizing matters: without it the label's minimum width is the
        // full text, which would stop the pane from ever reaching the rail
        // width even after the label is hidden and shown again.
        let label = gtk4::Label::builder()
            .label(item.title())
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        box_.append(&label);

        let row = gtk4::ListBoxRow::builder()
            .child(&box_)
            .tooltip_text(item.title())
            .build();
        row.update_property(&[gtk4::accessible::Property::Label(item.title())]);

        (row, label, box_)
    }

    // ── Content ──────────────────────────────────────────────────────────

    /// Takes the split view as an argument rather than reading it back out of
    /// `imp`: this runs *while* `build_ui` is still assembling the split view,
    /// so the field is not populated yet.
    ///
    /// Returns the whole content pane, not just the navigation view: the pane
    /// is the navigation view plus the status bar beneath it, so the bar shares
    /// the content's background instead of reading as a separate strip.
    fn build_navigation(&self, split_view: &adw::OverlaySplitView) -> gtk4::Box {
        let imp = self.imp();
        let state = self.state();
        let navigation = adw::NavigationView::new();

        let mut pages = HashMap::new();
        for item in NavItem::ALL {
            let content: gtk4::Widget = match item {
                NavItem::Dashboard => crate::ui::DashboardPage::new(&state).upcast(),
                NavItem::Tunnels => crate::ui::TunnelsPage::new(&state).upcast(),
                NavItem::Zones => crate::ui::ZonesPage::new(&state).upcast(),
                NavItem::Hostnames => crate::ui::HostnamesPage::new(&state).upcast(),
                NavItem::Hosts => crate::ui::HostsPage::new(&state).upcast(),
                NavItem::Activity => crate::ui::ActivityPage::new(&state).upcast(),
                NavItem::Settings => crate::ui::SettingsPage::new(&state).upcast(),
            };

            let header = adw::HeaderBar::new();

            // Hidden until the breakpoint collapses the split view; at that
            // point the sidebar is an overlay and needs a way back. One per
            // page header, because only the visible page's header is on
            // screen at any moment.
            let sidebar_button = gtk4::ToggleButton::builder()
                .icon_name("sidebar-show-symbolic")
                .tooltip_text("Show sidebar")
                .visible(false)
                .build();
            sidebar_button.update_property(&[gtk4::accessible::Property::Label("Show sidebar")]);
            split_view
                .bind_property("show-sidebar", &sidebar_button, "active")
                .sync_create()
                .bidirectional()
                .build();
            header.pack_start(&sidebar_button);
            imp.sidebar_buttons.borrow_mut().push(sidebar_button);

            for slot in page_header_widgets(*item) {
                match slot {
                    HeaderSlot::Start(widget) => {
                        if let Some(content) = widget
                            .downcast_ref::<gtk4::Button>()
                            .and_then(|button| button.child().and_downcast::<adw::ButtonContent>())
                        {
                            imp.header_labels.borrow_mut().push(content);
                        }
                        header.pack_start(&widget);
                    }
                    HeaderSlot::End(widget) => {
                        imp.header_extras.borrow_mut().push(widget.clone());
                        header.pack_end(&widget);
                    }
                }
            }

            header.pack_end(&{
                let menu = gtk4::MenuButton::builder()
                    .icon_name("open-menu-symbolic")
                    .tooltip_text("Main Menu")
                    .primary(true)
                    .popover(&self.build_primary_menu())
                    .build();
                menu.update_property(&[gtk4::accessible::Property::Label("Main Menu")]);
                menu
            });

            // `.app-content` rather than relying on Adwaita's `.content-pane`:
            // that class is applied by AdwOverlaySplitView only while the
            // split view is expanded, so a rule hung off it silently stops
            // matching once the breakpoint collapses.
            let toolbar = adw::ToolbarView::builder()
                .content(&content)
                .css_classes(["app-content"])
                .build();
            toolbar.add_top_bar(&header);

            let page = adw::NavigationPage::builder()
                .title(item.title())
                .tag(item.stack_name())
                .child(&toolbar)
                .build();

            navigation.add(&page);
            pages.insert(item.stack_name(), page);
        }

        imp.pages.replace(pages);
        imp.navigation.replace(Some(navigation.clone()));

        // The status bar is part of the content pane, not a window-wide strip:
        // the sidebar column keeps its own background to the bottom edge, and
        // the bar inherits exactly the surface the pages above it use.
        let pane = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        pane.add_css_class("app-content");
        pane.append(&navigation);
        pane.append(&self.build_status_bar());

        pane
    }

    // ── Primary menu ─────────────────────────────────────────────────────

    /// The primary menu: a theme switcher, then the standard GNOME menu items.
    fn build_primary_menu(&self) -> gtk4::Popover {
        let box_ = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(0)
            .width_request(240)
            .build();
        box_.append(&self.build_theme_switcher());
        box_.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        box_.append(&Self::build_menu_list());

        gtk4::Popover::builder()
            .child(&box_)
            .css_classes(["menu"])
            .build()
    }

    fn build_menu_list() -> gtk4::Box {
        let list = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(3)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();

        let about = format!("About {}", branding::APP_NAME);
        let mut items: Vec<(&str, &str, &str)> =
            vec![("Refresh", "view-refresh-symbolic", "win.refresh")];
        #[cfg(feature = "adw-shortcuts")]
        items.push((
            "Keyboard Shortcuts",
            "preferences-desktop-keyboard-shortcuts-symbolic",
            "app.shortcuts",
        ));
        items.push(("What's New", "starred-symbolic", "app.whats-new"));
        items.push((&about, "help-about-symbolic", "app.about"));

        for (label, icon, action) in &items {
            list.append(&Self::menu_item(label, icon, action));
        }

        list.append(&gtk4::Separator::new(gtk4::Orientation::Horizontal));
        list.append(&Self::menu_item(
            "Quit",
            "application-exit-symbolic",
            "app.quit",
        ));
        list
    }

    fn menu_item(label: &str, icon: &str, action: &str) -> gtk4::Button {
        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        content.append(&gtk4::Image::from_icon_name(icon));

        let text = gtk4::Label::builder()
            .label(label)
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .build();
        content.append(&text);

        gtk4::Button::builder()
            .child(&content)
            .action_name(action)
            .css_classes(["flat", "menu-item"])
            .build()
    }

    /// Three circles: follow the system, force light, force dark.
    ///
    /// Setting `AdwStyleManager:color-scheme` is all an application has to do;
    /// every stock widget, and every colour variable this project uses,
    /// updates itself. The choice is persisted so it survives a restart.
    fn build_theme_switcher(&self) -> gtk4::Box {
        let row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(18)
            .halign(gtk4::Align::Center)
            .margin_top(18)
            .margin_bottom(18)
            .build();

        let state = self.state();
        let system = Self::theme_button("system", "System", adw::ColorScheme::Default, &state);
        let light = Self::theme_button("light", "Light", adw::ColorScheme::ForceLight, &state);
        let dark = Self::theme_button("dark", "Dark", adw::ColorScheme::ForceDark, &state);

        light.set_group(Some(&system));
        dark.set_group(Some(&system));

        let sync = |system: &gtk4::ToggleButton,
                    light: &gtk4::ToggleButton,
                    dark: &gtk4::ToggleButton| {
            match adw::StyleManager::default().color_scheme() {
                adw::ColorScheme::ForceLight | adw::ColorScheme::PreferLight => {
                    light.set_active(true)
                }
                adw::ColorScheme::ForceDark | adw::ColorScheme::PreferDark => dark.set_active(true),
                _ => system.set_active(true),
            }
        };
        sync(&system, &light, &dark);

        // AdwStyleManager is a process-wide singleton, so a handler left on it
        // outlives any window. Capture the buttons weakly and drop out when
        // they are gone, or this leaks a widget per window for the lifetime of
        // the process.
        let (system_ref, light_ref, dark_ref) =
            (system.downgrade(), light.downgrade(), dark.downgrade());
        adw::StyleManager::default().connect_color_scheme_notify(move |_| {
            if let (Some(system), Some(light), Some(dark)) = (
                system_ref.upgrade(),
                light_ref.upgrade(),
                dark_ref.upgrade(),
            ) {
                sync(&system, &light, &dark);
            }
        });

        row.append(&system);
        row.append(&light);
        row.append(&dark);
        row
    }

    fn theme_button(
        css_class: &'static str,
        tooltip: &str,
        scheme: adw::ColorScheme,
        state: &AppState,
    ) -> gtk4::ToggleButton {
        let swatch = gtk4::Box::builder()
            .width_request(44)
            .height_request(44)
            .css_classes(["theme-swatch", css_class])
            .build();

        let button = gtk4::ToggleButton::builder()
            .child(&swatch)
            .tooltip_text(tooltip)
            .css_classes(["flat", "circular", "theme-button"])
            .build();
        button.update_property(&[gtk4::accessible::Property::Label(tooltip)]);

        let state = state.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                adw::StyleManager::default().set_color_scheme(scheme);
                state.update_settings(|settings| settings.theme = css_class.to_string());
            }
        });

        button
    }

    fn apply_saved_theme(&self) {
        adw::StyleManager::default().set_color_scheme(self.state().settings().color_scheme());
    }

    // ── Behaviour ────────────────────────────────────────────────────────

    /// Below 700sp the split view collapses: the sidebar becomes an overlay
    /// and each page header grows a button to reveal it.
    ///
    /// `sp` rather than `px` so the breakpoint tracks the user's text-scale
    /// factor — at 125% text, the same content needs more room.
    fn add_narrow_breakpoint(&self) {
        let imp = self.imp();
        let Some(split_view) = imp.split_view.borrow().clone() else {
            return;
        };

        let Ok(condition) = adw::BreakpointCondition::parse("max-width: 700sp") else {
            return;
        };

        let breakpoint = adw::Breakpoint::new(condition);
        breakpoint.add_setter(&split_view, "collapsed", Some(&true.to_value()));

        for button in imp.sidebar_buttons.borrow().iter() {
            breakpoint.add_setter(button, "visible", Some(&true.to_value()));
        }
        imp.narrow_breakpoint.replace(Some(breakpoint.clone()));

        // A header bar does not shed children on its own, so at 360px the
        // window controls would be pushed off the edge. Two steps: the
        // labelled action loses its label (`AdwButtonContent` keeps the icon),
        // and the refresh button — which is also in the primary menu — goes
        // entirely.
        for content in imp.header_labels.borrow().iter() {
            breakpoint.add_setter(content, "label", Some(&"".to_value()));
        }
        for widget in imp.header_extras.borrow().iter() {
            breakpoint.add_setter(widget, "visible", Some(&false.to_value()));
        }

        // The rail is suspended while the split view is collapsed: an
        // icon-only overlay sliding over the content is no use to anyone. This
        // cannot be an `add_setter`, because the resulting width depends on
        // the user's own rail preference rather than on a fixed value.
        let window = self.downgrade();
        breakpoint.connect_apply(move |_| {
            if let Some(window) = window.upgrade() {
                window.imp().collapsed.set(true);
                window.apply_sidebar_state();
            }
        });

        let window = self.downgrade();
        breakpoint.connect_unapply(move |_| {
            if let Some(window) = window.upgrade() {
                window.imp().collapsed.set(false);
                window.apply_sidebar_state();
            }
        });

        self.add_breakpoint(breakpoint);
    }

    fn setup_actions(&self) {
        let toggle_rail = gio::ActionEntry::builder("toggle-rail")
            .activate(|window: &Self, _, _| {
                let imp = window.imp();
                imp.rail.set(!imp.rail.get());
                window.apply_sidebar_state();
            })
            .build();

        let page = gio::ActionEntry::builder("page")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Self, _, parameter| {
                if let Some(name) = parameter.and_then(glib::Variant::str) {
                    window.show_named_page(name);
                }
            })
            .build();

        let refresh = gio::ActionEntry::builder("refresh")
            .activate(|window: &Self, _, _| window.refresh_everything())
            .build();

        let create_tunnel = gio::ActionEntry::builder("create-tunnel")
            .activate(|window: &Self, _, _| {
                crate::ui::create_tunnel_dialog::present(window, &window.state());
            })
            .build();

        let add_host = gio::ActionEntry::builder("add-host")
            .activate(|window: &Self, _, _| {
                crate::ui::host_dialog::present_new(window, &window.state());
            })
            .build();

        let tunnels_filter = gio::ActionEntry::builder("tunnels-filter")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Self, _, parameter| {
                let Some(name) = parameter.and_then(glib::Variant::str) else {
                    return;
                };
                window.show_named_page("tunnels");
                if let Some(page) = window.tunnels_page() {
                    page.set_filter_by_name(name);
                }
            })
            .build();

        let open_tunnel = gio::ActionEntry::builder("open-tunnel")
            .parameter_type(Some(glib::VariantTy::STRING))
            .activate(|window: &Self, _, parameter| {
                if let Some(id) = parameter.and_then(glib::Variant::str) {
                    window.open_tunnel_detail(id);
                }
            })
            .build();

        let sidebar = gio::ActionEntry::builder("sidebar")
            .activate(|window: &Self, _, _| window.toggle_sidebar())
            .build();

        self.add_action_entries([
            toggle_rail,
            page,
            refresh,
            create_tunnel,
            add_host,
            tunnels_filter,
            open_tunnel,
            sidebar,
        ]);
    }

    /// Applies the sidebar's current shape: full pane, or the icon rail.
    ///
    /// Order matters. The labels are hidden *before* the widths are narrowed,
    /// because the pane cannot be allocated less than the sidebar's own
    /// minimum width, and a visible label contributes its full text width to
    /// that minimum. Hiding first drops the minimum to the icon width, and
    /// only then does asking for 80px have any effect.
    fn apply_sidebar_state(&self) {
        let imp = self.imp();
        let collapsed = imp.collapsed.get();
        let rail = imp.rail.get() && !collapsed;

        for label in imp.nav_labels.borrow().iter() {
            label.set_visible(!rail);
        }

        if let Some(title) = imp.sidebar_title.borrow().as_ref() {
            title.set_visible(!rail);
        }

        // With the label gone the row box would still be left-aligned, leaving
        // the icon against the leading edge of an 80px rail. Centre the box
        // instead. `halign` is not expressible in CSS, so it has to be set
        // here rather than in the `.rail` stylesheet rules.
        for row_box in imp.nav_boxes.borrow().iter() {
            row_box.set_halign(if rail {
                gtk4::Align::Center
            } else {
                gtk4::Align::Fill
            });
        }

        if let Some(split_view) = imp.split_view.borrow().as_ref() {
            let width = if rail { RAIL_WIDTH } else { SIDEBAR_WIDTH };
            split_view.set_min_sidebar_width(width);
            split_view.set_max_sidebar_width(width);

            if let Some(sidebar) = split_view.sidebar() {
                if rail {
                    sidebar.add_css_class("rail");
                } else {
                    sidebar.remove_css_class("rail");
                }
            }
        }

        // In the rail the header has no title, so a trailing-packed button
        // reads as hanging off the right edge of an 80px strip. Move it into
        // the header's title slot instead, which is centred. A header bar
        // child can only be in one slot at a time, so this is a real reparent
        // — guarded by `rail_button_centred` so it happens only on transition.
        if let (Some(header), Some(button), Some(title)) = (
            imp.sidebar_header.borrow().as_ref(),
            imp.rail_button.borrow().as_ref(),
            imp.sidebar_title.borrow().as_ref(),
        ) {
            if rail != imp.rail_button_centred.get() {
                if rail {
                    header.remove(button);
                    header.set_title_widget(Some(button));
                } else {
                    header.set_title_widget(Some(title));
                    header.pack_end(button);
                }
                imp.rail_button_centred.set(rail);
            }
        }

        if let Some(button) = imp.rail_button.borrow().as_ref() {
            button.set_visible(!collapsed);
            button.set_icon_name(if rail {
                "sidebar-show-right-symbolic"
            } else {
                "sidebar-show-symbolic"
            });
            button.set_tooltip_text(Some(if rail {
                "Expand sidebar"
            } else {
                "Collapse sidebar"
            }));
        }
    }

    /// Shows or hides the sidebar. Only meaningful while the split view is
    /// collapsed, where the sidebar is an overlay.
    pub fn toggle_sidebar(&self) {
        if let Some(split_view) = self.imp().split_view.borrow().as_ref() {
            split_view.set_show_sidebar(!split_view.shows_sidebar());
        }
    }

    pub fn show_named_page(&self, name: &str) -> bool {
        match NavItem::from_stack_name(name) {
            Some(item) => {
                self.select(item);
                true
            }
            None => false,
        }
    }

    fn select(&self, item: NavItem) {
        let index = NavItem::ALL.iter().position(|i| *i == item).unwrap_or(0) as i32;
        if let Some(list) = self.imp().nav_list.borrow().as_ref() {
            if let Some(row) = list.row_at_index(index) {
                list.select_row(Some(&row));
            }
        }
        // Selecting an already-selected row emits nothing, so drive the
        // navigation directly too — otherwise `win.page` on the current page
        // would not pop a pushed detail view.
        self.show_page(item);
    }

    fn show_page(&self, item: NavItem) {
        let imp = self.imp();
        if imp.navigating.get() {
            return;
        }
        imp.navigating.set(true);

        if let (Some(navigation), Some(page)) = (
            imp.navigation.borrow().as_ref(),
            imp.pages.borrow().get(item.stack_name()),
        ) {
            // `replace` rather than `push`: the sidebar is top-level
            // navigation, so moving between its entries should not build a
            // back stack. A detail page pushed on top of one of these still
            // gets its back button.
            navigation.replace(std::slice::from_ref(page));
        }

        // On a narrow window the sidebar is an overlay covering the content;
        // picking a page should get it out of the way.
        if let Some(split_view) = imp.split_view.borrow().as_ref() {
            if split_view.is_collapsed() {
                split_view.set_show_sidebar(false);
            }
        }

        imp.navigating.set(false);
    }

    fn tunnels_page(&self) -> Option<crate::ui::TunnelsPage> {
        self.imp()
            .pages
            .borrow()
            .get("tunnels")
            .and_then(|page| page.child())
            .and_downcast::<adw::ToolbarView>()
            .and_then(|toolbar| toolbar.content())
            .and_downcast::<crate::ui::TunnelsPage>()
    }

    /// Pushes the detail page for a tunnel onto the navigation stack.
    pub fn open_tunnel_detail(&self, tunnel_id: &str) {
        let state = self.state();
        let Some(row) = state.tunnel(tunnel_id) else {
            state.record(crate::domain::activity::ActivityEvent::warning(
                crate::domain::activity::ActivityCategory::App,
                format!("Unknown tunnel requested: {tunnel_id}"),
            ));
            crate::ui::widgets::sticky_toast(
                self,
                "That tunnel is not in the list. Refresh and try again.",
                None,
            );
            return;
        };

        self.show_named_page("tunnels");

        let Some(navigation) = self.imp().navigation.borrow().clone() else {
            return;
        };

        let detail = crate::ui::TunnelDetailPage::new(&state, &row.tunnel);
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(
            &gtk4::Label::builder().label(&row.tunnel.name).build(),
        ));
        // Same sidebar toggle and menu as every section header: when the
        // breakpoint collapses the split view, the detail page is the
        // visible one and would otherwise strand the user with no way back
        // to the sidebar or the menu.
        if let Some(split_view) = self.imp().split_view.borrow().clone() {
            let sidebar_button = gtk4::ToggleButton::builder()
                .icon_name("sidebar-show-symbolic")
                .tooltip_text("Show sidebar")
                .visible(false)
                .build();
            sidebar_button.update_property(&[gtk4::accessible::Property::Label("Show sidebar")]);
            split_view
                .bind_property("show-sidebar", &sidebar_button, "active")
                .sync_create()
                .bidirectional()
                .build();
            header.pack_start(&sidebar_button);
            if let Some(breakpoint) = self.imp().narrow_breakpoint.borrow().clone() {
                breakpoint.add_setter(&sidebar_button, "visible", Some(&true.to_value()));
            }
            self.imp().sidebar_buttons.borrow_mut().push(sidebar_button);
        }
        header.pack_end(&{
            let menu = gtk4::MenuButton::builder()
                .icon_name("open-menu-symbolic")
                .tooltip_text("Main Menu")
                .primary(true)
                .popover(&self.build_primary_menu())
                .build();
            menu.update_property(&[gtk4::accessible::Property::Label("Main Menu")]);
            menu
        });
        let toolbar = adw::ToolbarView::builder()
            .content(&detail)
            .css_classes(["app-content"])
            .build();
        toolbar.add_top_bar(&header);

        let page = adw::NavigationPage::builder()
            .title(&row.tunnel.name)
            .tag(format!("tunnel:{tunnel_id}"))
            .child(&toolbar)
            .build();

        navigation.push(&page);
    }

    /// Refreshes everything the connected account can see.
    pub fn refresh_everything(&self) {
        let state = self.state();
        if !state.is_connected() {
            return;
        }

        crate::ui::refresh_tunnels(&state);
        crate::ui::hosts_page::refresh_hosts(&state);
        crate::ui::zones_page::refresh_zones(&state);
    }

    /// Bottom status bar: update indicator and version on the trailing edge.
    ///
    /// Shared with Network Manager's mechanism: the version is always visible
    /// in the bottom-right corner, and the update notice appears beside it
    /// only when the GitHub release check finds a newer version. The bar has
    /// no background of its own — it sits on the content pane, so it reads as
    /// the bottom of the page rather than as a separate strip.
    fn build_status_bar(&self) -> gtk4::Box {
        let bar = gtk4::Box::builder().spacing(6).build();
        bar.add_css_class("status-bar");

        let spacer = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .hexpand(true)
            .build();
        bar.append(&spacer);

        // Hidden until the version check completes.
        let update_banner = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk4::Align::Center)
            .build();
        update_banner.add_css_class("update-banner");
        update_banner.set_visible(false);

        let update_icon = gtk4::Image::from_icon_name("software-update-available-symbolic");
        update_icon.set_pixel_size(12);
        update_banner.append(&update_icon);

        let update_label = gtk4::Label::new(Some("New version available"));
        update_label.add_css_class("update-banner-label");
        update_banner.append(&update_label);

        bar.append(&update_banner);
        self.imp().update_banner.replace(Some(update_banner));

        let version_label = gtk4::Label::new(Some(&format!("Version {}", branding::VERSION)));
        version_label.add_css_class("app-version-label");
        version_label.set_tooltip_text(Some(&format!(
            "Tunnel Manager {} — released under the MIT licence",
            branding::VERSION
        )));
        bar.append(&version_label);

        bar
    }

    /// Run the one-time GitHub release check in the background.
    ///
    /// Mirrors Network Manager: silent on failure, and the result crosses
    /// back to the main thread through the shared runtime before it touches
    /// any widget.
    fn check_for_updates(&self) {
        let window = self.downgrade();
        crate::services::runtime::spawn(
            crate::version_check::check_for_update(branding::VERSION),
            move |update| {
                if let (Some(window), Some(update)) = (window.upgrade(), update) {
                    window.show_update_available(&update);
                }
            },
        );
    }

    /// Show the update notice in the status bar with a link to the release.
    fn show_update_available(&self, info: &crate::version_check::UpdateInfo) {
        let imp = self.imp();
        let Some(banner) = imp.update_banner.borrow().clone() else {
            return;
        };

        while let Some(child) = banner.first_child() {
            banner.remove(&child);
        }

        let icon = gtk4::Image::from_icon_name("software-update-available-symbolic");
        icon.set_pixel_size(12);
        icon.add_css_class("update-icon");
        banner.append(&icon);

        let link = gtk4::LinkButton::with_label(
            &info.download_url,
            &format!("v{} available", info.latest_version),
        );
        link.add_css_class("update-link");
        banner.append(&link);

        banner.set_visible(true);
    }

    /// Starts the automatic refresh, if the user has asked for one.
    fn start_refresh_timer(&self) {
        let imp = self.imp();
        if let Some(source) = imp.refresh_timer.take() {
            source.remove();
        }

        let Some(interval) = self.state().settings().refresh_interval() else {
            return;
        };

        let window = self.downgrade();
        let source = crate::services::runtime::every(interval, move || {
            let Some(window) = window.upgrade() else {
                // The window is gone; stop the timer rather than leaking it.
                return false;
            };
            // Never start a refresh on top of one that is still running: a
            // slow account and a short interval would otherwise queue requests
            // faster than they complete.
            if window.state().is_connected() && !window.state().is_anything_busy() {
                crate::ui::refresh_tunnels(&window.state());
            }
            true
        });

        imp.refresh_timer.replace(Some(source));
    }

    /// Re-reads the refresh interval after it has been changed in Settings.
    pub fn restart_refresh_timer(&self) {
        self.start_refresh_timer();
    }
}

/// Where a page's own header control goes.
enum HeaderSlot {
    Start(gtk4::Widget),
    End(gtk4::Widget),
}

/// The controls each page contributes to its own header bar.
///
/// Kept as a function rather than a method on each page so that every header
/// ends up the same shape and the ordering is decided in one place.
fn page_header_widgets(item: NavItem) -> Vec<HeaderSlot> {
    let refresh = || {
        let button = gtk4::Button::builder()
            .icon_name("view-refresh-symbolic")
            .tooltip_text("Refresh")
            .action_name("win.refresh")
            .build();
        button.update_property(&[gtk4::accessible::Property::Label("Refresh")]);
        HeaderSlot::End(button.upcast())
    };

    match item {
        NavItem::Dashboard => vec![refresh()],
        NavItem::Tunnels => vec![
            HeaderSlot::Start(
                gtk4::Button::builder()
                    .child(
                        // `can_shrink` lets the label drop when the header
                        // runs out of room, which is what keeps a 360px window
                        // from pushing the window controls off the edge.
                        &adw::ButtonContent::builder()
                            .icon_name("list-add-symbolic")
                            .label("Create Tunnel")
                            .can_shrink(true)
                            .build(),
                    )
                    .action_name("win.create-tunnel")
                    .css_classes(["suggested-action"])
                    .build()
                    .upcast(),
            ),
            refresh(),
        ],
        NavItem::Zones | NavItem::Hostnames => vec![refresh()],
        NavItem::Hosts => vec![
            HeaderSlot::Start(
                gtk4::Button::builder()
                    .child(
                        &adw::ButtonContent::builder()
                            .icon_name("list-add-symbolic")
                            .label("Add Host")
                            .can_shrink(true)
                            .build(),
                    )
                    .action_name("win.add-host")
                    .build()
                    .upcast(),
            ),
            refresh(),
        ],
        NavItem::Activity | NavItem::Settings => vec![],
    }
}
