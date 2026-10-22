//! The tunnel list.
//!
//! A `GtkColumnView` over a `gio::ListStore` of [`TunnelObject`], with the
//! standard model chain — store → filter → sort → selection → view — so that
//! searching, filtering and sorting are all model operations and the view
//! never keeps a copy of anything.
//!
//! The table has eight columns and the window's minimum width is 360px, so
//! the two are reconciled by an `AdwBreakpointBin` *inside the page*: as the
//! window narrows, columns drop out from the least important inwards until
//! only the name and the status are left. A page-local breakpoint rather than
//! the window's, because which columns matter is the page's business.

use std::cell::RefCell;

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::domain::tunnel::TunnelFilter;
use crate::services::tunnels;
use crate::ui::models::{self, TunnelObject};
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct TunnelsPage {
        pub state: RefCell<Option<AppState>>,
        pub store: RefCell<Option<gio::ListStore>>,
        pub filter: RefCell<Option<gtk4::CustomFilter>>,
        pub selection: RefCell<Option<gtk4::SingleSelection>>,
        pub stack: RefCell<Option<gtk4::Stack>>,
        pub toggles: RefCell<Option<adw::ToggleGroup>>,
        pub search: RefCell<Option<gtk4::SearchEntry>>,
        pub summary_label: RefCell<Option<gtk4::Label>>,
        pub notice_area: RefCell<Option<gtk4::Box>>,
        /// The toolbar and the summary bar, kept so `refresh` can hide them
        /// when there is nothing to filter or count. They live outside the
        /// state stack on purpose: a filter that matches nothing must never
        /// take the filter chips away with it.
        pub toolbar: RefCell<Option<gtk4::Widget>>,
        pub summary_bar: RefCell<Option<gtk4::Widget>>,

        /// The live search text and filter, shared with the filter closure.
        pub query: RefCell<String>,
        pub active_filter: std::cell::Cell<Option<TunnelFilter>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TunnelsPage {
        const NAME: &'static str = "TunnelManagerTunnelsPage";
        type Type = super::TunnelsPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for TunnelsPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.active_filter.set(Some(TunnelFilter::All));
        }
    }

    impl WidgetImpl for TunnelsPage {}
    impl BinImpl for TunnelsPage {}
}

glib::wrapper! {
    pub struct TunnelsPage(ObjectSubclass<imp::TunnelsPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl TunnelsPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                if !matches!(event, Event::Tunnels | Event::Busy | Event::Hosts) {
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
        let imp = self.imp();

        let store = gio::ListStore::new::<TunnelObject>();

        // The filter re-reads `query` and `active_filter` every time it is
        // asked, and `filter.changed(...)` is what asks it to re-run.
        let filter = {
            let page = self.downgrade();
            gtk4::CustomFilter::new(move |object| {
                let Some(page) = page.upgrade() else {
                    return true;
                };
                let Some(tunnel) = object.downcast_ref::<TunnelObject>() else {
                    return true;
                };
                page.matches(tunnel)
            })
        };

        let filtered = gtk4::FilterListModel::new(Some(store.clone()), Some(filter.clone()));

        let column_view = gtk4::ColumnView::builder()
            .show_row_separators(true)
            .single_click_activate(false)
            .build();
        column_view.add_css_class("tunnel-table");

        // Sorting is driven by the column headers, so the sort model has to be
        // fed the view's own sorter — and it has to sit between the filter and
        // the selection, or a row's position would not survive a re-sort.
        let sorted = gtk4::SortListModel::new(Some(filtered), column_view.sorter());
        let selection = gtk4::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        column_view.set_model(Some(&selection));

        self.add_columns(&column_view);

        {
            let page = self.downgrade();
            column_view.connect_activate(move |view, position| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let Some(object) = view
                    .model()
                    .and_then(|model| model.item(position))
                    .and_downcast::<TunnelObject>()
                else {
                    return;
                };
                page.open(&object.id());
            });
        }

        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .child(&column_view)
            .build();

        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        content.append(&self.build_notice_area());
        let toolbar = self.build_toolbar();
        content.append(&toolbar);

        // The states share the *table region* only. The search box and the
        // filter chips sit above the stack and stay put whatever it shows:
        // a filter that matches nothing used to replace the toolbar with the
        // "Nothing matches" page, removing the chips at exactly the moment
        // the user needs them to pick a different one.
        let stack = gtk4::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.set_vexpand(true);
        stack.add_named(&w::loading_state("Loading tunnels…"), Some("loading"));
        stack.add_named(&w::table_card(&scroller), Some("content"));
        stack.add_named(&self.build_empty_state(), Some("empty"));
        stack.add_named(&self.build_no_matches_state(), Some("no-matches"));
        stack.add_named(&self.build_error_state(), Some("error"));
        content.append(&stack);

        let summary = self.build_summary();
        content.append(&summary);

        // Same clamp as every other page: without it the table and its
        // groups stretch to the full window while every other page stops
        // at CONTENT_WIDTH, so the cards never line up.
        let clamped = adw::Clamp::builder()
            .maximum_size(crate::ui::page::CONTENT_WIDTH)
            .tightening_threshold(crate::ui::page::CONTENT_WIDTH - 200)
            .vexpand(true)
            .hexpand(true)
            .child(&content)
            .build();

        // The page's own breakpoints. `AdwBreakpointBin` needs a width and
        // height request below every breakpoint it declares, or it warns and
        // refuses to apply them.
        let bin = adw::BreakpointBin::builder()
            .width_request(200)
            .height_request(200)
            .child(&clamped)
            .build();
        self.add_column_breakpoints(&bin, &column_view);

        imp.store.replace(Some(store));
        imp.filter.replace(Some(filter));
        imp.selection.replace(Some(selection));
        imp.stack.replace(Some(stack));
        imp.toolbar.replace(Some(toolbar.upcast()));
        imp.summary_bar.replace(Some(summary.upcast()));

        self.set_child(Some(&bin));
        self.refresh();
    }

    fn build_notice_area(&self) -> gtk4::Box {
        // The same horizontal margins as the search field and the table
        // beneath it, so the notice lines up with the rest of the page
        // instead of touching the content edges.
        let area = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .margin_start(12)
            .margin_end(12)
            .build();
        self.imp().notice_area.replace(Some(area.clone()));
        area
    }

    fn build_toolbar(&self) -> gtk4::Box {
        let bar = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();

        let search = gtk4::SearchEntry::builder()
            .placeholder_text("Search tunnels by name, ID or host")
            .hexpand(true)
            .build();
        search.update_property(&[gtk4::accessible::Property::Label(
            "Search tunnels by name, ID or host",
        )]);
        search.set_search_delay(150);
        {
            let page = self.downgrade();
            search.connect_search_changed(move |entry| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                *page.imp().query.borrow_mut() = entry.text().to_lowercase();
                page.reapply_filter();
            });
        }
        bar.append(&search);

        // AdwToggleGroup rather than a linked strip of buttons: it behaves as
        // one widget for the keyboard and for a screen reader, and it can
        // shrink rather than overflow on a narrow window.
        let toggles = adw::ToggleGroup::builder()
            // Filled across the row on a normal window; `can_shrink` lets the
            // five labels share the width at 360px instead of pushing a
            // horizontal scrollbar.
            .can_shrink(true)
            .homogeneous(true)
            .halign(gtk4::Align::Fill)
            .hexpand(true)
            .build();
        toggles.add_css_class("filter-group");
        for filter in TunnelFilter::ALL {
            toggles.add(
                adw::Toggle::builder()
                    .name(filter.name())
                    .label(filter.label())
                    .build(),
            );
        }
        toggles.set_active_name(Some(TunnelFilter::All.name()));
        toggles.update_property(&[gtk4::accessible::Property::Label("Filter by status")]);

        {
            let page = self.downgrade();
            toggles.connect_active_notify(move |group| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let name = group.active_name().unwrap_or_default();
                page.imp()
                    .active_filter
                    .set(TunnelFilter::from_name(&name).or(Some(TunnelFilter::All)));
                page.reapply_filter();
            });
        }

        // A horizontal scroller rather than `can_shrink`: shrinking
        // ellipsizes the toggle labels, and a filter called "Degra…" is not a
        // filter. `propagate_natural_width` keeps the group at its full size
        // whenever there is room, so the scrollbar only appears when it is
        // genuinely needed.
        let chips = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Automatic)
            .vscrollbar_policy(gtk4::PolicyType::Never)
            // Height only. Propagating the natural *width* would hand the
            // group's full width back to the scroller as a minimum, which is
            // exactly what the scroller exists to avoid.
            .propagate_natural_height(true)
            .child(&toggles)
            .build();
        bar.append(&chips);

        self.imp().search.replace(Some(search));
        self.imp().toggles.replace(Some(toggles));
        bar
    }

    fn build_summary(&self) -> gtk4::Box {
        let bar = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .css_classes(["toolbar", "status-bar"])
            .build();

        let label = gtk4::Label::builder()
            .halign(gtk4::Align::Start)
            .hexpand(true)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        label.add_css_class("caption");
        label.add_css_class("dimmed");
        bar.append(&label);

        self.imp().summary_label.replace(Some(label));
        bar
    }

    fn build_empty_state(&self) -> adw::StatusPage {
        let button = gtk4::Button::builder()
            .label("Create Tunnel")
            .action_name("win.create-tunnel")
            .build();

        w::empty_state(
            "network-vpn-symbolic",
            "No tunnels on this account",
            "A tunnel connects a service on a machine you control to Cloudflare's \
             network, without opening an inbound port.",
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
            "The tunnel list could not be refreshed. What was last shown, if \
             anything, is out of date — the detail is in the Activity page.",
            Some(button),
        )
    }

    fn build_no_matches_state(&self) -> adw::StatusPage {
        let button = gtk4::Button::builder().label("Clear Filters").build();
        {
            let page = self.downgrade();
            button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.clear_filters();
                }
            });
        }

        w::empty_state(
            "edit-find-symbolic",
            "Nothing matches",
            "No tunnel matches the current search and filter.",
            Some(button),
        )
    }

    // ── Columns ──────────────────────────────────────────────────────────

    fn add_columns(&self, view: &gtk4::ColumnView) {
        // Name, with the "Locally configured" badge attached: a tunnel whose
        // ingress lives in a config.yml on its own machine cannot be edited
        // from here, and that has to be visible in the list rather than only
        // after opening it.
        let name = gtk4::ColumnViewColumn::builder()
            .title("Name")
            .expand(true)
            .resizable(true)
            .factory(&two_line_factory())
            .sorter(&string_sorter("name"))
            .build();
        name.set_id(Some("name"));
        view.append_column(&name);

        // Fixed rather than sized to content: a status is an icon *and* a
        // word, and an ellipsized "D…" conveys the state by colour alone,
        // which is exactly what this application must not do. The width fits
        // the longest label at the default text scale; a larger scale pushes
        // the name column narrower instead of truncating the status.
        let status = gtk4::ColumnViewColumn::builder()
            .title("Status")
            .factory(&status_factory())
            .sorter(&string_sorter("status-key"))
            .fixed_width(124)
            .build();
        status.set_id(Some("status"));
        view.append_column(&status);

        let hostnames = gtk4::ColumnViewColumn::builder()
            .title("Hostnames")
            .factory(&text_factory("hostnames", &["numeric"], 1.0))
            .sorter(&numeric_sorter("hostname-count"))
            .build();
        hostnames.set_id(Some("hostnames"));
        view.append_column(&hostnames);

        let connectors = gtk4::ColumnViewColumn::builder()
            .title("Connectors")
            .factory(&text_factory("connectors", &["numeric"], 1.0))
            .sorter(&numeric_sorter("connector-count"))
            .build();
        connectors.set_id(Some("connectors"));
        view.append_column(&connectors);

        let last_active = gtk4::ColumnViewColumn::builder()
            .title("Last active")
            .factory(&text_factory("last-active", &["dimmed"], 0.0))
            .sorter(&string_sorter("active-key"))
            .build();
        last_active.set_id(Some("last-active"));
        view.append_column(&last_active);

        let hosts = gtk4::ColumnViewColumn::builder()
            .title("Managed hosts")
            .factory(&text_factory("hosts", &["dimmed"], 0.0))
            .sorter(&string_sorter("hosts"))
            .build();
        hosts.set_id(Some("hosts"));
        view.append_column(&hosts);

        let created = gtk4::ColumnViewColumn::builder()
            .title("Created")
            .factory(&text_factory("created", &["dimmed"], 0.0))
            .sorter(&string_sorter("created-key"))
            .build();
        created.set_id(Some("created"));
        view.append_column(&created);

        // Worst-first is the useful default for an operations list.
        view.sort_by_column(Some(&status), gtk4::SortType::Ascending);
    }

    /// Drops columns as the page narrows, least important first.
    fn add_column_breakpoints(&self, bin: &adw::BreakpointBin, view: &gtk4::ColumnView) {
        let column = |id: &str| -> Option<gtk4::ColumnViewColumn> {
            let columns = view.columns();
            (0..columns.n_items())
                .filter_map(|index| columns.item(index))
                .filter_map(|item| item.downcast::<gtk4::ColumnViewColumn>().ok())
                .find(|column| column.id().map(|value| value == id).unwrap_or(false))
        };

        // Each stage lists every column hidden at that width, not just the
        // ones the previous stage did not hide. `AdwBreakpointBin` applies only
        // the last breakpoint whose condition matches and unapplies the others,
        // so a stage that listed only its own delta would silently restore
        // everything the narrower stages had hidden.
        let stages: &[(&str, &[&str])] = &[
            ("max-width: 1000sp", &["created", "hosts"]),
            ("max-width: 800sp", &["created", "hosts", "last-active"]),
            (
                "max-width: 620sp",
                &["created", "hosts", "last-active", "connectors"],
            ),
            (
                "max-width: 460sp",
                &["created", "hosts", "last-active", "connectors", "hostnames"],
            ),
        ];

        for (condition, ids) in stages {
            let Ok(parsed) = adw::BreakpointCondition::parse(condition) else {
                continue;
            };
            let breakpoint = adw::Breakpoint::new(parsed);
            for id in *ids {
                if let Some(column) = column(id) {
                    breakpoint.add_setter(&column, "visible", Some(&false.to_value()));
                }
            }
            bin.add_breakpoint(breakpoint);
        }
    }

    // ── Behaviour ────────────────────────────────────────────────────────

    fn matches(&self, tunnel: &TunnelObject) -> bool {
        let imp = self.imp();

        let query = imp.query.borrow();
        if !query.is_empty() && !tunnel.search().contains(query.as_str()) {
            return false;
        }

        let Some(filter) = imp.active_filter.get() else {
            return true;
        };
        match tunnel.row() {
            Some(row) => filter.matches(&row.tunnel),
            None => true,
        }
    }

    fn reapply_filter(&self) {
        if let Some(filter) = self.imp().filter.borrow().as_ref() {
            // `Different` rather than `MoreStrict`/`LessStrict`: the filter
            // combines a search box and a status chip, and either can move in
            // either direction independently.
            filter.changed(gtk4::FilterChange::Different);
        }
        self.update_summary();
    }

    /// Selects a filter chip from outside — the dashboard's stat cards do this.
    pub fn set_filter_by_name(&self, name: &str) {
        if let Some(toggles) = self.imp().toggles.borrow().as_ref() {
            toggles.set_active_name(Some(name));
        }
    }

    fn clear_filters(&self) {
        if let Some(search) = self.imp().search.borrow().as_ref() {
            search.set_text("");
        }
        self.set_filter_by_name(TunnelFilter::All.name());
    }

    fn open(&self, tunnel_id: &str) {
        // Routed through the window's action rather than reaching for the
        // window directly, so the page needs no reference to its ancestor.
        if let Some(root) = self.root().and_downcast::<gtk4::Window>() {
            let _ = root.activate_action("win.open-tunnel", Some(&tunnel_id.to_variant()));
        }
    }

    fn refresh(&self) {
        let state = self.state();
        let imp = self.imp();

        let Some(store) = imp.store.borrow().clone() else {
            return;
        };
        let Some(stack) = imp.stack.borrow().clone() else {
            return;
        };

        let rows = state.tunnels();
        let objects: Vec<TunnelObject> = rows.iter().map(TunnelObject::new).collect();
        models::replace(&store, objects);

        // With no tunnels at all there is nothing to search, filter or
        // count, so the toolbar and the summary step aside for the status
        // page rather than sitting over it as dead controls.
        let has_rows = !rows.is_empty();
        if let Some(toolbar) = imp.toolbar.borrow().as_ref() {
            toolbar.set_visible(has_rows);
        }
        if let Some(summary) = imp.summary_bar.borrow().as_ref() {
            summary.set_visible(has_rows);
        }

        if rows.is_empty() {
            stack.set_visible_child_name(if state.is_busy("tunnels") {
                "loading"
            } else if state.tunnels_error().is_some() {
                "error"
            } else {
                "empty"
            });
        } else {
            stack.set_visible_child_name("content");
        }

        self.reapply_filter();
        self.update_local_config_banner(&rows);
    }

    fn update_summary(&self) {
        let imp = self.imp();
        let Some(label) = imp.summary_label.borrow().clone() else {
            return;
        };
        let Some(selection) = imp.selection.borrow().clone() else {
            return;
        };
        let Some(stack) = imp.stack.borrow().clone() else {
            return;
        };

        let total = self.state().tunnels().len();
        let shown = selection.n_items() as usize;

        label.set_label(&if shown == total {
            w::plural(total, "tunnel", "tunnels")
        } else {
            format!("{shown} of {}", w::plural(total, "tunnel", "tunnels"))
        });

        // Distinguish "the account has nothing" from "the filter matched
        // nothing"; they need different words and different buttons.
        if total > 0 && shown == 0 {
            stack.set_visible_child_name("no-matches");
        } else if total > 0 {
            stack.set_visible_child_name("content");
        }
    }

    fn update_local_config_banner(&self, rows: &[tunnels::TunnelRow]) {
        let Some(area) = self.imp().notice_area.borrow().clone() else {
            return;
        };

        while let Some(child) = area.first_child() {
            area.remove(&child);
        }

        let local = rows
            .iter()
            .filter(|row| !row.tunnel.config_source.is_remotely_managed())
            .count();

        if local == 0 {
            return;
        }

        // Short on purpose: the full explanation lives on the tunnel's own
        // page, where it is acted on.
        area.append(&w::notice(
            "dialog-information-symbolic",
            &format!(
                "{} configured by a file on its own machine, so hostname editing is \
                 disabled for those.",
                w::plural(local, "tunnel is", "tunnels are")
            ),
            "status-neutral",
            None,
        ));
    }
}

/// The name column: the tunnel's name, with its configuration source and ID
/// underneath. Two lines, because at 360px there is no room for a separate
/// column and the source is exactly the thing that changes what the user can
/// do with the row.
fn two_line_factory() -> gtk4::SignalListItemFactory {
    let factory = gtk4::SignalListItemFactory::new();

    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };

        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(0)
            .valign(gtk4::Align::Center)
            .build();

        let name = gtk4::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        name.add_css_class("heading");
        column.append(&name);

        let subtitle = gtk4::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        subtitle.add_css_class("caption");
        subtitle.add_css_class("dimmed");
        column.append(&subtitle);

        item.set_child(Some(&column));
    });

    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let (Some(tunnel), Some(column)) = (
            item.item().and_downcast::<TunnelObject>(),
            item.child().and_downcast::<gtk4::Box>(),
        ) else {
            return;
        };

        let Some(name) = column.first_child().and_downcast::<gtk4::Label>() else {
            return;
        };
        name.set_label(&tunnel.name());

        if let Some(subtitle) = name.next_sibling().and_downcast::<gtk4::Label>() {
            subtitle.set_label(&format!("{} · {}", tunnel.source(), tunnel.id()));
        }

        let locally_configured = tunnel
            .row()
            .map(|row| !row.tunnel.config_source.is_remotely_managed())
            .unwrap_or(false);
        // Dim the subtitle only: dimming the whole cell drops the name to
        // 55% opacity and can fail contrast, and a screen reader needs the
        // state as text rather than a tooltip.
        if let Some(subtitle) = name.next_sibling().and_downcast::<gtk4::Label>() {
            if locally_configured {
                subtitle.add_css_class("dimmed");
            } else {
                subtitle.remove_css_class("dimmed");
            }
        }
        if locally_configured {
            column.set_tooltip_text(Some(
                "This tunnel is configured by a config.yml on the machine running its \
                 connector, so its hostnames are not editable from here.",
            ));
            column.update_property(&[gtk4::accessible::Property::Label(&format!(
                "{}, locally configured",
                tunnel.name()
            ))]);
        } else {
            column.set_tooltip_text(None);
            column.update_property(&[gtk4::accessible::Property::Label(&tunnel.name())]);
        }
    });

    factory
}

/// The status column: an icon and a word, tinted. Never the tint alone.
fn status_factory() -> gtk4::SignalListItemFactory {
    let factory = gtk4::SignalListItemFactory::new();

    factory.connect_setup(|_, item| {
        if let Some(item) = item.downcast_ref::<gtk4::ListItem>() {
            item.set_child(Some(&w::status_badge(
                "dialog-question-symbolic",
                "",
                "dimmed",
            )));
        }
    });

    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let (Some(tunnel), Some(badge)) = (
            item.item().and_downcast::<TunnelObject>(),
            item.child().and_downcast::<gtk4::Box>(),
        ) else {
            return;
        };
        let Some(row) = tunnel.row() else {
            return;
        };
        let status = &row.tunnel.status;

        if let Some(icon) = badge.first_child().and_downcast::<gtk4::Image>() {
            icon.set_icon_name(Some(status.icon_name()));
        }
        if let Some(label) = badge.last_child().and_downcast::<gtk4::Label>() {
            label.set_label(status.label());
            label.set_ellipsize(gtk4::pango::EllipsizeMode::None);
        }

        for class in ["success", "warning", "error", "dimmed"] {
            badge.remove_css_class(class);
        }
        badge.add_css_class(status.css_class());
        badge.set_tooltip_text(Some(status.explanation()));
        badge.update_property(&[gtk4::accessible::Property::Label(status.label())]);
    });

    factory
}

/// A plain text column bound to one string property.
fn text_factory(
    property: &'static str,
    classes: &'static [&'static str],
    xalign: f32,
) -> gtk4::SignalListItemFactory {
    let factory = gtk4::SignalListItemFactory::new();

    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let label = gtk4::Label::builder()
            .xalign(xalign)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .valign(gtk4::Align::Center)
            .build();
        for class in classes {
            label.add_css_class(class);
        }
        item.set_child(Some(&label));
    });

    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let (Some(object), Some(label)) = (item.item(), item.child().and_downcast::<gtk4::Label>())
        else {
            return;
        };
        let value: String = object.property(property);
        label.set_label(&value);
    });

    factory
}

fn string_sorter(property: &str) -> gtk4::StringSorter {
    gtk4::StringSorter::builder()
        .expression(gtk4::PropertyExpression::new(
            TunnelObject::static_type(),
            gtk4::Expression::NONE,
            property,
        ))
        .ignore_case(true)
        .build()
}

fn numeric_sorter(property: &str) -> gtk4::NumericSorter {
    gtk4::NumericSorter::builder()
        .expression(gtk4::PropertyExpression::new(
            TunnelObject::static_type(),
            gtk4::Expression::NONE,
            property,
        ))
        .build()
}
