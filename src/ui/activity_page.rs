//! The activity log.
//!
//! This page answers one question — *what has this application done?* — and it
//! has to answer it without being mistaken for something it is not. Cloudflare
//! keeps an audit log of everything that happens to an account; this
//! application never reads it. A local record presented as if it were the
//! account's authoritative history would be a lie exactly when it matters
//! most, so [`NOT_AN_AUDIT_LOG`] is on the page as a sentence rather than as a
//! footnote, and it is repeated at the top of every diagnostic report.
//!
//! The list is a `GtkListView` of two-line rows rather than a `GtkColumnView`:
//! the window must stay usable at 360px, and a summary is prose of
//! unpredictable length that cannot be split into columns without either
//! truncating the one thing worth reading or forcing a horizontal scrollbar.
//! Everything a column would have carried — severity, category, time, target —
//! fits in the badge and the caption line.
//!
//! Nothing here has to redact anything: an `ActivityEvent` cannot be
//! constructed without going through
//! [`crate::domain::activity::redact`]. The diagnostic report redacts a second
//! time anyway, because it also carries log lines and a filesystem path, and
//! it is the one thing on this page a user will paste in public.

use std::cell::{Cell, RefCell};

use gtk4::prelude::*;
use gtk4::subclass::prelude::*;
use gtk4::{gio, glib};
use libadwaita as adw;
use libadwaita::prelude::*;
use libadwaita::subclass::prelude::*;

use crate::branding;
use crate::domain::activity::{
    redact, ActivityCategory, ActivityEvent, ActivitySeverity, REDACTED,
};
use crate::services::{logging, runtime};
use crate::storage;
use crate::ui::models::{self, ActivityObject};
use crate::ui::state::{AppState, Event};
use crate::ui::widgets as w;

/// The correctness statement this page exists to make. Shown on the page, in
/// the empty state and at the top of every diagnostic report.
const NOT_AN_AUDIT_LOG: &str = "These are operations this application performed. Cloudflare \
     keeps its own audit log; this application does not read it, so anything done from the \
     Cloudflare dashboard, the API or another tool will not appear here.";

/// How many events a diagnostic report lists. A bug report wants the shape of
/// what happened, not a database dump, and a clipboard payload that large is
/// unusable in a browser text area.
const REPORT_EVENT_LIMIT: usize = 200;

/// The name a severity is filtered by. `ActivitySeverity` has no stable
/// identifier of its own, and its label is already unique.
fn severity_name(severity: ActivitySeverity) -> String {
    severity.label().to_lowercase()
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ActivityPage {
        pub state: RefCell<Option<AppState>>,
        pub store: RefCell<Option<gio::ListStore>>,
        pub filter: RefCell<Option<gtk4::CustomFilter>>,
        pub selection: RefCell<Option<gtk4::SingleSelection>>,
        pub stack: RefCell<Option<gtk4::Stack>>,
        pub banner: RefCell<Option<adw::Banner>>,
        pub search: RefCell<Option<gtk4::SearchEntry>>,
        pub toggles: RefCell<Option<adw::ToggleGroup>>,
        pub categories: RefCell<Option<gtk4::DropDown>>,
        pub summary_label: RefCell<Option<gtk4::Label>>,

        /// The toolbar, the actions group and the summary bar, kept so
        /// `refresh` can hide them when there is nothing to filter, export
        /// or count. Like on the Tunnels page they live outside the state
        /// stack: a filter that matches nothing must not take the filter
        /// controls away with it.
        pub toolbar: RefCell<Option<gtk4::Widget>>,
        pub actions: RefCell<Option<gtk4::Widget>>,
        pub summary_bar: RefCell<Option<gtk4::Widget>>,

        /// The live filter, read by the filter closure every time it runs.
        pub query: RefCell<String>,
        pub severity: Cell<Option<ActivitySeverity>>,
        pub category: Cell<Option<ActivityCategory>>,

        /// How many events the database holds. The in-memory list is capped,
        /// so without this the counter would quietly under-report the log.
        pub stored: Cell<usize>,
        /// False until the first count comes back — the gap between "nothing
        /// happened" and "we have not looked yet".
        pub counted: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ActivityPage {
        const NAME: &'static str = "TunnelManagerActivityPage";
        type Type = super::ActivityPage;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for ActivityPage {}
    impl WidgetImpl for ActivityPage {}
    impl BinImpl for ActivityPage {}
}

glib::wrapper! {
    pub struct ActivityPage(ObjectSubclass<imp::ActivityPage>)
        @extends gtk4::Widget, adw::Bin,
        @implements gtk4::Accessible, gtk4::Buildable, gtk4::ConstraintTarget;
}

impl ActivityPage {
    pub fn new(state: &AppState) -> Self {
        let page: Self = glib::Object::builder().build();
        page.imp().state.replace(Some(state.clone()));
        page.build();
        page.recount();

        state.subscribe(&page, {
            let page = page.downgrade();
            move |_, event| {
                if event != Event::Activity {
                    return;
                }
                if let Some(page) = page.upgrade() {
                    page.refresh();
                    // The database keeps a longer tail than memory does, so
                    // the stored figure has to be asked for again rather than
                    // inferred from the list that just changed.
                    page.recount();
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

    // ── Construction ─────────────────────────────────────────────────────

    fn build(&self) {
        let imp = self.imp();

        let store = gio::ListStore::new::<ActivityObject>();

        // One `GtkCustomFilter` for all three controls rather than a chain of
        // filter models: the search box, the severity toggles and the category
        // dropdown move independently, and a single closure that re-reads all
        // three is easier to keep consistent than three stacked models.
        let filter = {
            let page = self.downgrade();
            gtk4::CustomFilter::new(move |object| {
                let Some(page) = page.upgrade() else {
                    return true;
                };
                let Some(event) = object.downcast_ref::<ActivityObject>() else {
                    return true;
                };
                page.matches(event)
            })
        };

        let filtered = gtk4::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
        let selection = gtk4::SingleSelection::builder()
            .model(&filtered)
            .autoselect(false)
            .can_unselect(true)
            .build();

        let list = gtk4::ListView::builder()
            .model(&selection)
            .factory(&row_factory())
            .show_separators(true)
            // A row's only action is to open its details, so a single click is
            // the whole interaction; requiring a double click would hide it.
            .single_click_activate(true)
            .vexpand(true)
            .build();

        {
            let page = self.downgrade();
            list.connect_activate(move |view, position| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let Some(event) = view
                    .model()
                    .and_then(|model| model.item(position))
                    .and_downcast::<ActivityObject>()
                else {
                    return;
                };
                page.open_event(&event);
            });
        }

        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .vexpand(true)
            .hexpand(true)
            .child(&list)
            .build();

        let content = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .build();
        content.append(&w::wrappable(&self.build_banner()));
        content.append(&self.build_notice());
        let toolbar = self.build_toolbar();
        content.append(&toolbar);

        // The states share the *list region* only; the notice, the toolbar
        // and the actions stay above and below it whatever it shows. A
        // severity or category that matches nothing used to replace the
        // toolbar with the "Nothing matches" page, taking the very controls
        // that led there away.
        let stack = gtk4::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.set_vexpand(true);
        stack.add_named(
            &w::loading_state("Reading the activity log…"),
            Some("loading"),
        );
        stack.add_named(&w::table_card(&scroller), Some("content"));
        stack.add_named(&self.build_empty_state(), Some("empty"));
        stack.add_named(&self.build_no_matches_state(), Some("no-matches"));
        stack.add_named(&self.build_error_state(), Some("error"));
        content.append(&stack);

        let actions = self.build_actions();
        content.append(&actions);
        let summary = self.build_summary();
        content.append(&summary);

        // Same clamp as every other page: without it the log list stretches
        // to the full window while every other page stops at CONTENT_WIDTH.
        let clamped = adw::Clamp::builder()
            .maximum_size(crate::ui::page::CONTENT_WIDTH)
            .tightening_threshold(crate::ui::page::CONTENT_WIDTH - 200)
            .vexpand(true)
            .hexpand(true)
            .child(&content)
            .build();

        imp.store.replace(Some(store));
        imp.filter.replace(Some(filter));
        imp.selection.replace(Some(selection));
        imp.stack.replace(Some(stack.clone()));
        imp.toolbar.replace(Some(toolbar.upcast()));
        imp.actions.replace(Some(actions.upcast()));
        imp.summary_bar.replace(Some(summary.upcast()));

        self.set_child(Some(&clamped));
        self.refresh();
    }

    /// The sentence the page exists to make, above everything else on it.
    ///
    /// Deliberately not `.dimmed`: dimming it would make the one statement
    /// that stops this page being mistaken for Cloudflare's audit log look
    /// like the fine print it must not be.
    fn build_notice(&self) -> gtk4::Box {
        let notice = w::inline_message("dialog-information-symbolic", NOT_AN_AUDIT_LOG, "body");
        notice.set_margin_top(12);
        notice.set_margin_start(12);
        notice.set_margin_end(12);
        notice
    }

    fn build_banner(&self) -> adw::Banner {
        let banner = adw::Banner::builder().revealed(false).build();
        self.imp().banner.replace(Some(banner.clone()));
        banner
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
            .placeholder_text("Search this log")
            .hexpand(true)
            .build();
        search.update_property(&[gtk4::accessible::Property::Label("Search this log")]);
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

        let toggles = adw::ToggleGroup::builder()
            .can_shrink(true)
            .homogeneous(false)
            .build();
        toggles.add(adw::Toggle::builder().name("all").label("All").build());
        for severity in ActivitySeverity::ALL {
            toggles.add(
                adw::Toggle::builder()
                    .name(severity_name(*severity))
                    .label(severity.label())
                    .build(),
            );
        }
        toggles.set_active_name(Some("all"));
        toggles.update_property(&[gtk4::accessible::Property::Label("Filter by severity")]);
        {
            let page = self.downgrade();
            toggles.connect_active_notify(move |group| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let name = group.active_name().unwrap_or_default();
                page.imp().severity.set(
                    ActivitySeverity::ALL
                        .iter()
                        .copied()
                        .find(|severity| severity_name(*severity) == name),
                );
                page.reapply_filter();
            });
        }

        // "All categories" first, so the list of names doubles as the reset.
        let names = gtk4::StringList::new(&["All categories"]);
        for category in ActivityCategory::ALL {
            names.append(category.label());
        }
        let categories = gtk4::DropDown::builder()
            .model(&names)
            .selected(0)
            .tooltip_text("Filter by what the operation touched")
            .valign(gtk4::Align::Center)
            .build();
        categories.update_property(&[gtk4::accessible::Property::Label("Filter by category")]);
        {
            let page = self.downgrade();
            categories.connect_selected_notify(move |dropdown| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let selected = dropdown.selected() as usize;
                page.imp().category.set(
                    selected
                        .checked_sub(1)
                        .and_then(|index| ActivityCategory::ALL.get(index).copied()),
                );
                page.reapply_filter();
            });
        }

        // AdwWrapBox: at 360px the toggles and the dropdown cannot share a
        // line, and a GtkBox would force a horizontal scrollbar rather than
        // reflow them.
        let wrap = adw::WrapBox::builder()
            .child_spacing(6)
            .line_spacing(6)
            .build();
        wrap.append(&toggles);
        wrap.append(&categories);
        bar.append(&wrap);

        self.imp().search.replace(Some(search));
        self.imp().toggles.replace(Some(toggles));
        self.imp().categories.replace(Some(categories));
        bar
    }

    fn build_actions(&self) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .build();

        let report = adw::ButtonRow::builder()
            .title("Copy Diagnostic Report")
            .start_icon_name("edit-copy-symbolic")
            .build();
        report.set_tooltip_text(Some(
            "Copies a redacted plain-text summary of this application, its recent log \
             output and the activity listed here.",
        ));
        {
            let page = self.downgrade();
            report.connect_activated(move |row| {
                if let Some(page) = page.upgrade() {
                    page.copy_report(row);
                }
            });
        }
        group.add(&report);

        let clear = adw::ButtonRow::builder()
            .title("Clear Activity")
            .start_icon_name("user-trash-symbolic")
            .build();
        clear.add_css_class("destructive-action");
        {
            let page = self.downgrade();
            clear.connect_activated(move |row| {
                if let Some(page) = page.upgrade() {
                    page.confirm_clear(row);
                }
            });
        }
        group.add(&clear);

        group
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
        label.set_tooltip_text(Some(
            "Only the most recent events are listed here. The database on disk keeps a \
             longer tail and prunes the oldest.",
        ));
        bar.append(&label);

        self.imp().summary_label.replace(Some(label));
        bar
    }

    fn build_empty_state(&self) -> adw::StatusPage {
        w::empty_state(
            "document-open-recent-symbolic",
            "Nothing recorded yet",
            &format!(
                "Every operation you run — creating a tunnel, publishing a hostname, \
                 restarting a connector — is listed here with what came back. {NOT_AN_AUDIT_LOG}"
            ),
            None,
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
            "No recorded event matches the current search, severity and category.",
            Some(button),
        )
    }

    /// Shown when the log could not be opened *and* nothing has been recorded
    /// since. A diagnostic report still works, and is the one useful thing to
    /// offer someone whose database is unreadable.
    fn build_error_state(&self) -> adw::StatusPage {
        let problem = self
            .state()
            .storage_problem()
            .unwrap_or_else(|| "The activity database could not be opened.".into());

        let button = gtk4::Button::builder()
            .label("Copy Diagnostic Report")
            .build();
        {
            let page = self.downgrade();
            button.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    page.copy_report(button);
                }
            });
        }

        w::error_state(
            "dialog-warning-symbolic",
            "The activity log is unavailable",
            &format!(
                "{problem}\n\nOperations still run normally and are still logged to the \
                 terminal, but nothing is being saved and this page will stay empty."
            ),
            Some(button),
        )
    }

    // ── Filtering ────────────────────────────────────────────────────────

    /// Matched against the display strings rather than the domain values: the
    /// model already stores `label()` for both enums, and comparing two
    /// borrowed strings avoids cloning an event per item per keystroke.
    fn matches(&self, event: &ActivityObject) -> bool {
        let imp = self.imp();

        let query = imp.query.borrow();
        if !query.is_empty() && !event.search().contains(query.as_str()) {
            return false;
        }

        if let Some(severity) = imp.severity.get() {
            if event.severity() != severity.label() {
                return false;
            }
        }

        if let Some(category) = imp.category.get() {
            if event.category() != category.label() {
                return false;
            }
        }

        true
    }

    fn reapply_filter(&self) {
        if let Some(filter) = self.imp().filter.borrow().as_ref() {
            // `Different`: three independent controls feed this filter, and
            // any of them can loosen while another tightens.
            filter.changed(gtk4::FilterChange::Different);
        }
        self.update_summary();
    }

    fn clear_filters(&self) {
        let imp = self.imp();
        if let Some(search) = imp.search.borrow().as_ref() {
            search.set_text("");
        }
        if let Some(toggles) = imp.toggles.borrow().as_ref() {
            toggles.set_active_name(Some("all"));
        }
        if let Some(categories) = imp.categories.borrow().as_ref() {
            categories.set_selected(0);
        }
    }

    // ── Refreshing ───────────────────────────────────────────────────────

    fn refresh(&self) {
        let state = self.state();
        let imp = self.imp();

        let Some(store) = imp.store.borrow().clone() else {
            return;
        };
        let Some(stack) = imp.stack.borrow().clone() else {
            return;
        };

        let events = state.activity();
        models::replace(
            &store,
            events.iter().map(ActivityObject::new).collect::<Vec<_>>(),
        );

        if let Some(banner) = imp.banner.borrow().clone() {
            match state.storage_problem() {
                // Only worth saying once there is something to lose: with an
                // empty log the error state says it more plainly.
                Some(_) if !events.is_empty() => {
                    banner.set_title(
                        "These events are only in memory: the activity database could not be \
                         opened, so nothing is being saved.",
                    );
                    banner.set_revealed(true);
                }
                _ => banner.set_revealed(false),
            }
        }

        // With an empty log there is nothing to search, filter, export or
        // count, so those controls step aside for the status page instead
        // of sitting over it dead.
        let has_events = !events.is_empty();
        for slot in [
            imp.toolbar.borrow().as_ref(),
            imp.actions.borrow().as_ref(),
            imp.summary_bar.borrow().as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            slot.set_visible(has_events);
        }

        if events.is_empty() {
            stack.set_visible_child_name(if state.storage_problem().is_some() {
                "error"
            } else if imp.counted.get() {
                "empty"
            } else {
                "loading"
            });
        } else {
            stack.set_visible_child_name("content");
        }

        self.reapply_filter();
    }

    /// Asks the database how much it is holding. SQLite is blocking, so this
    /// goes through the runtime even though it is one `count(*)`.
    ///
    /// It races the insert that `AppState::record` spawns, so immediately
    /// after an event the figure can be one behind. The next event corrects
    /// it, and a status bar that is briefly one short is a better trade than
    /// serialising the two.
    fn recount(&self) {
        let store = self.state().store();
        let page = self.downgrade();

        runtime::spawn_blocking(
            move || {
                store
                    .with(|connection| storage::activity::count(connection).unwrap_or(0))
                    .unwrap_or(0)
            },
            move |stored| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.imp().stored.set(stored);
                // The first count is what turns "we have not looked yet" into
                // an answer, so it settles which state the page shows. Every
                // later one only moves a figure in the status bar, and
                // rebuilding every row for that would be absurd.
                if page.imp().counted.replace(true) {
                    page.update_summary();
                } else {
                    page.refresh();
                }
            },
        );
    }

    fn update_summary(&self) {
        let imp = self.imp();
        let Some(label) = imp.summary_label.borrow().clone() else {
            return;
        };
        let Some(selection) = imp.selection.borrow().clone() else {
            return;
        };
        let Some(store) = imp.store.borrow().clone() else {
            return;
        };
        let Some(stack) = imp.stack.borrow().clone() else {
            return;
        };

        // The store rather than `state.activity()`: this runs on every
        // keystroke, and the model already holds exactly those events.
        let total = store.n_items() as usize;
        let shown = selection.n_items() as usize;
        let stored = imp.stored.get();

        let mut text = if shown == total {
            w::plural(total, "event", "events")
        } else {
            format!("{shown} of {}", w::plural(total, "event", "events"))
        };
        if stored > total {
            text.push_str(&format!(" · {stored} recorded on disk"));
        }
        label.set_label(&text);

        // "Nothing has been recorded" and "the filter matched nothing" need
        // different words and a different way out.
        if total > 0 {
            stack.set_visible_child_name(if shown == 0 { "no-matches" } else { "content" });
        }
    }

    // ── The event dialog ─────────────────────────────────────────────────

    /// The whole event, for someone about to paste it into a bug report.
    fn open_event(&self, object: &ActivityObject) {
        let Some(event) = object.value() else {
            return;
        };

        let dialog = adw::Dialog::builder()
            .title("Event")
            .content_width(640)
            .content_height(560)
            .build();

        let body = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let group = w::group("Event", "");

        let summary = w::property_row("Summary", &event.summary);
        // A summary is a sentence, not a field: it wraps rather than being cut
        // off at the width of a dialog.
        summary.set_subtitle_lines(0);
        group.add(&summary);

        group.add(&w::status_row(
            "Severity",
            event.severity.icon_name(),
            event.severity.label(),
            event.severity.css_class(),
            None,
        ));
        group.add(&w::property_row("Category", event.category.label()));
        group.add(&w::property_row(
            "When",
            &w::format_exact_time(Some(event.at)),
        ));
        group.add(&w::property_row(
            "Target",
            event.target.as_deref().unwrap_or(""),
        ));
        body.append(&group);

        let heading = gtk4::Label::builder()
            .label("Detail")
            .halign(gtk4::Align::Start)
            .xalign(0.0)
            .build();
        heading.add_css_class("heading");
        body.append(&heading);

        match event.detail.as_deref() {
            Some(detail) => body.append(&w::code_block(detail)),
            None => body.append(&w::inline_message(
                "dialog-information-symbolic",
                "This event carried no further detail — the summary is everything that \
                 was recorded.",
                "dimmed",
            )),
        }

        body.append(&w::inline_message(
            "security-high-symbolic",
            &format!(
                "Everything above was redacted when it was recorded: anything shaped like \
                 an API token, a tunnel token, a passphrase or an Authorization header was \
                 replaced with {REDACTED} before it reached the log. This is safe to paste \
                 into a bug report."
            ),
            "dimmed",
        ));

        let scroller = gtk4::ScrolledWindow::builder()
            .hscrollbar_policy(gtk4::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&body)
            .build();

        let header = adw::HeaderBar::new();
        let copy = gtk4::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy this event")
            .build();
        copy.add_css_class("flat");
        copy.update_property(&[gtk4::accessible::Property::Label("Copy this event")]);
        {
            // The toast belongs to the window, and a dialog is not inside the
            // window's toast overlay — so it is raised on the page instead.
            let page = self.downgrade();
            let text = format_event(&event);
            copy.connect_clicked(move |_| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.clipboard().set_text(&text);
                w::toast(&page, "Event copied to the clipboard");
            });
        }
        header.pack_end(&copy);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&scroller));
        dialog.set_child(Some(&toolbar));
        dialog.present(Some(self));
    }

    // ── Clearing ─────────────────────────────────────────────────────────

    fn confirm_clear(&self, row: &adw::ButtonRow) {
        let state = self.state();
        let page = self.downgrade();

        w::confirm_destructive(
            row,
            "Clear the activity log?",
            "Every event recorded on this computer is deleted, in memory and on disk. \
             Nothing on Cloudflare changes: this log is only this application's own \
             record of what it did.",
            &["A diagnostic report copied afterwards will have no activity in it.".to_string()],
            "Clear",
            move || {
                state.clear_activity();
                if let Some(page) = page.upgrade() {
                    w::toast(&page, "Activity log cleared");
                }
            },
        );
    }

    // ── The diagnostic report ────────────────────────────────────────────

    /// The events currently listed, newest first and bounded.
    fn visible_events(&self) -> Vec<ActivityEvent> {
        let Some(selection) = self.imp().selection.borrow().clone() else {
            return Vec::new();
        };

        (0..selection.n_items())
            .take(REPORT_EVENT_LIMIT)
            .filter_map(|index| selection.item(index))
            .filter_map(|object| object.downcast::<ActivityObject>().ok())
            .filter_map(|object| object.value())
            .collect()
    }

    /// Assembles a redacted report and puts it on the clipboard.
    ///
    /// The inputs that need a widget or the shared state are read here; the
    /// text itself — which reads the log buffer, resolves the database path
    /// and redacts a few hundred lines — is assembled off the main thread.
    fn copy_report(&self, trigger: &impl IsA<gtk4::Widget>) {
        let state = self.state();
        let events = self.visible_events();
        let selection_count = self
            .imp()
            .selection
            .borrow()
            .as_ref()
            .map(|selection| selection.n_items() as usize)
            .unwrap_or(0);

        let inputs = ReportInputs {
            events,
            shown: selection_count,
            total: state.activity().len(),
            stored: self.imp().stored.get(),
            account: state.account_id(),
            secrets_available: state.secrets_available(),
            storage_problem: state.storage_problem(),
            gtk_version: format!(
                "{}.{}.{}",
                gtk4::major_version(),
                gtk4::minor_version(),
                gtk4::micro_version()
            ),
            adw_version: format!(
                "{}.{}.{}",
                adw::major_version(),
                adw::minor_version(),
                adw::micro_version()
            ),
        };

        // Assembling the report reads the whole log buffer, so the control
        // that started it goes insensitive rather than inviting a second run.
        trigger.as_ref().set_sensitive(false);
        let trigger = trigger.as_ref().downgrade();

        let page = self.downgrade();
        runtime::spawn_blocking(
            move || build_report(inputs),
            move |report| {
                if let Some(widget) = trigger.upgrade() {
                    widget.set_sensitive(true);
                }
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.clipboard().set_text(&report);
                w::sticky_toast(
                    &page,
                    "Diagnostic report copied. It carries no token or passphrase — every \
                     line went through the same redaction as the log.",
                    None,
                );
            },
        );
    }
}

/// One row: a tinted severity badge, the summary, and what and when.
fn row_factory() -> gtk4::SignalListItemFactory {
    let factory = gtk4::SignalListItemFactory::new();

    factory.connect_setup(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };

        let row = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(12)
            .margin_end(12)
            .build();

        // Icon and word together, tinted on the box so the symbolic icon takes
        // the colour with the text. Never the tint alone. It starts on a real
        // severity's class rather than an invented one so that the reset in
        // `bind` — which knows only the classes the enum hands out — can take
        // it off again.
        let badge = w::status_badge(
            ActivitySeverity::Info.icon_name(),
            "",
            ActivitySeverity::Info.css_class(),
        );
        badge.set_valign(gtk4::Align::Start);
        row.append(&badge);

        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .hexpand(true)
            .valign(gtk4::Align::Center)
            .build();

        let summary = gtk4::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        summary.add_css_class("heading");
        column.append(&summary);

        let caption = gtk4::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk4::pango::EllipsizeMode::End)
            .build();
        caption.add_css_class("caption");
        caption.add_css_class("dimmed");
        column.append(&caption);
        row.append(&column);

        // The affordance for "this opens something", the way an activatable
        // row carries one.
        let chevron = gtk4::Image::from_icon_name("go-next-symbolic");
        chevron.set_valign(gtk4::Align::Center);
        chevron.add_css_class("dimmed");
        chevron.set_accessible_role(gtk4::AccessibleRole::Presentation);
        row.append(&chevron);

        item.set_child(Some(&row));
    });

    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk4::ListItem>() else {
            return;
        };
        let (Some(object), Some(row)) = (
            item.item().and_downcast::<ActivityObject>(),
            item.child().and_downcast::<gtk4::Box>(),
        ) else {
            return;
        };
        let Some(event) = object.value() else {
            return;
        };

        let Some(badge) = row.first_child().and_downcast::<gtk4::Box>() else {
            return;
        };
        if let Some(icon) = badge.first_child().and_downcast::<gtk4::Image>() {
            icon.set_icon_name(Some(event.severity.icon_name()));
        }
        if let Some(label) = badge.last_child().and_downcast::<gtk4::Label>() {
            label.set_label(event.severity.label());
        }
        // Rows are recycled, so the previous row's tint has to come off first.
        // Taken from the enum rather than written out here, so a severity that
        // changes its class does not leave a stale tint behind.
        for severity in ActivitySeverity::ALL {
            badge.remove_css_class(severity.css_class());
        }
        badge.add_css_class(event.severity.css_class());
        badge.update_property(&[gtk4::accessible::Property::Label(event.severity.label())]);

        let Some(column) = badge.next_sibling().and_downcast::<gtk4::Box>() else {
            return;
        };
        if let Some(summary) = column.first_child().and_downcast::<gtk4::Label>() {
            summary.set_label(&object.summary());
        }
        if let Some(caption) = column.last_child().and_downcast::<gtk4::Label>() {
            let mut text = format!("{} · {}", object.category(), object.when());
            if !object.target().is_empty() {
                text.push_str(&format!(" · {}", object.target()));
            }
            caption.set_label(&text);
        }

        row.set_tooltip_text(Some(&w::format_exact_time(Some(event.at))));
    });

    factory
}

/// One event as plain text, for the clipboard.
fn format_event(event: &ActivityEvent) -> String {
    let mut text = format!(
        "{}\n\nSeverity: {}\nCategory: {}\nWhen:     {}\n",
        event.summary,
        event.severity.label(),
        event.category.label(),
        w::format_exact_time(Some(event.at)),
    );
    if let Some(target) = &event.target {
        text.push_str(&format!("Target:   {target}\n"));
    }
    if let Some(detail) = &event.detail {
        text.push_str(&format!("\n{detail}\n"));
    }

    // Redacted at construction already; redacting the assembled text costs
    // nothing and covers whatever a future field turns out to carry.
    redact(&text)
}

/// Everything the report needs from the main thread, so that assembling it
/// does not have to touch a widget or the shared state.
struct ReportInputs {
    events: Vec<ActivityEvent>,
    shown: usize,
    total: usize,
    stored: usize,
    account: Option<String>,
    secrets_available: bool,
    storage_problem: Option<String>,
    gtk_version: String,
    adw_version: String,
}

/// The diagnostic report, redacted as a whole before it is handed back.
///
/// It names an account but never a credential: an account tag identifies which
/// account a support conversation is about and grants nothing on its own,
/// whereas an API token in a public bug report is an incident.
fn build_report(inputs: ReportInputs) -> String {
    let mut report = String::with_capacity(8 * 1024);

    report.push_str(&format!("{} diagnostic report\n", branding::APP_NAME));
    report.push_str(&format!(
        "Generated {}\n\n",
        w::format_exact_time(Some(chrono::Utc::now()))
    ));
    report.push_str(NOT_AN_AUDIT_LOG);
    report.push_str("\n\nApplication\n");

    let mut field = |name: &str, value: &str| {
        report.push_str(&format!("  {name:<18}{value}\n"));
    };
    field("Version", branding::VERSION);
    field("GTK", &inputs.gtk_version);
    field("libadwaita", &inputs.adw_version);
    field("Log level", &logging::current_level());
    field(
        "Database",
        &storage::db::database_path().display().to_string(),
    );
    field(
        "Credential store",
        if inputs.secrets_available {
            "available"
        } else {
            "unavailable — the API token is not being saved"
        },
    );
    field(
        "Account",
        inputs.account.as_deref().unwrap_or("not connected"),
    );
    if let Some(problem) = &inputs.storage_problem {
        field("Storage problem", problem);
    }

    report.push_str(&format!(
        "\nActivity — {} of {} in this session, {} recorded on disk\n",
        inputs.shown, inputs.total, inputs.stored
    ));
    if inputs.events.is_empty() {
        report.push_str("  (nothing recorded)\n");
    }
    if inputs.shown > inputs.events.len() {
        report.push_str(&format!(
            "  (the most recent {} of them are listed)\n",
            inputs.events.len()
        ));
    }
    for event in &inputs.events {
        report.push_str(&format!(
            "  [{}] {:<8} {:<22} {}\n",
            event
                .at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S"),
            event.severity.label().to_uppercase(),
            event.category.label(),
            event.summary,
        ));
        if let Some(target) = &event.target {
            report.push_str(&format!("      target: {target}\n"));
        }
        if let Some(detail) = &event.detail {
            for line in detail.lines() {
                report.push_str(&format!("      | {line}\n"));
            }
        }
    }

    let lines = logging::recent_lines();
    report.push_str(&format!(
        "\nRecent log — {}\n",
        w::plural(lines.len(), "line", "lines")
    ));
    for line in &lines {
        report.push_str(&format!("  {line}\n"));
    }

    report.push_str(&format!(
        "\nNo API token, tunnel token or passphrase appears above: anything shaped like \
         one was replaced with {REDACTED} before it was written.\n"
    ));

    redact(&report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> ReportInputs {
        ReportInputs {
            events: vec![ActivityEvent::error(
                ActivityCategory::Api,
                "Request failed: Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456",
            )
            .with_target("web.example.com")],
            shown: 1,
            total: 1,
            stored: 1,
            account: Some("a".repeat(32)),
            secrets_available: true,
            storage_problem: None,
            gtk_version: "4.22.4".into(),
            adw_version: "1.9.3".into(),
        }
    }

    #[test]
    fn a_report_says_it_is_not_cloudflares_audit_log() {
        let report = build_report(inputs());
        assert!(report.contains("does not read it"), "got {report}");
    }

    #[test]
    fn a_report_carries_the_account_but_never_a_token() {
        let report = build_report(inputs());
        assert!(
            report.contains(&"a".repeat(32)),
            "an account tag identifies the account"
        );
        assert!(!report.contains("abcdefghij"), "got {report}");
        assert!(report.contains(REDACTED));
    }

    #[test]
    fn a_formatted_event_keeps_the_target_and_loses_the_secret() {
        let event = ActivityEvent::error(ActivityCategory::Dns, "Could not create the record")
            .with_target("web.example.com")
            .with_detail(r#"{"api_token":"abcdefghijklmnopqrstuvwxyz0123456789"}"#);

        let text = format_event(&event);
        assert!(text.contains("web.example.com"));
        assert!(text.contains("Could not create the record"));
        assert!(!text.contains("abcdefghij"), "got {text}");
    }

    #[test]
    fn every_severity_has_a_distinct_filter_name() {
        let mut names: Vec<String> = ActivitySeverity::ALL
            .iter()
            .map(|severity| severity_name(*severity))
            .collect();
        names.push("all".into());
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "a toggle name collides with another");
    }
}
