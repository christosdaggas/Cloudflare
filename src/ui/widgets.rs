//! The handful of composite widgets the product pages share.
//!
//! Everything here is built out of stock GTK4 and libadwaita. The rules from
//! `AGENTS.md` apply throughout and two of them shape almost every function:
//!
//! * **No colour literal, ever.** Semantic state comes from Adwaita's
//!   `.success` / `.warning` / `.error` / `.accent` classes, which remap the
//!   accent for their whole subtree and follow light, dark, high contrast and
//!   the user's accent colour without a single value written down here.
//! * **Never colour alone.** Every status in this application carries an icon
//!   *and* a word as well as a tint, because a red dot says nothing to a
//!   colour-blind user and nothing at all to a screen reader.
//!
//! `add_css_class` is used throughout rather than `set_css_classes`: the
//! latter replaces the whole list, including the classes GTK put there itself.

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

/// A status pill: icon, label, and a semantic tint.
///
/// The tint is applied to the *box*, not the label, so the symbolic icon —
/// which follows `currentColor` — is recoloured with it.
pub fn status_badge(icon_name: &str, label: &str, css_class: &str) -> gtk4::Box {
    let badge = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk4::Align::Center)
        .build();
    badge.add_css_class("status-badge");
    badge.add_css_class(css_class);

    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_pixel_size(16);
    badge.append(&icon);

    // Deliberately not ellipsized. A status that has been cut down to "Gr…"
    // or "No…" is a coloured dot with extra steps: the word is the part a
    // screen reader reads and the part a colour-blind user relies on. The
    // row title beside it can shrink or ellipsize instead.
    let text = gtk4::Label::builder()
        .label(label)
        .ellipsize(gtk4::pango::EllipsizeMode::None)
        .build();
    text.add_css_class("caption-heading");
    badge.append(&text);

    // The icon is decorative once the label says the same thing; giving the
    // box one accessible name stops a screen reader reading the state twice.
    badge.update_property(&[gtk4::accessible::Property::Label(label)]);
    icon.set_accessible_role(gtk4::AccessibleRole::Presentation);

    badge
}

/// A symbolic icon carrying a semantic tint.
///
/// The class goes on the wrapper rather than on the image, because
/// `.success` and its siblings remap the foreground for a whole subtree and a
/// symbolic icon follows `currentColor`. The tooltip carries the word, so the
/// tint is never the only thing saying what the state is.
pub fn tinted_icon(icon_name: &str, css_class: &str, label: &str) -> gtk4::Box {
    let wrapper = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
    wrapper.add_css_class(css_class);
    wrapper.set_valign(gtk4::Align::Center);
    wrapper.append(&gtk4::Image::from_icon_name(icon_name));
    wrapper.set_tooltip_text(Some(label));
    wrapper
}

/// A dashboard figure: a number, what it counts, and — optionally — how it
/// relates to a total ("3 of 7 tunnels") and a hairline bar showing that
/// share.
///
/// Built with chained setters rather than passed as positional arguments,
/// because the optional parts are exactly the parts a caller should have to
/// name.
///
/// `activates()` makes the finished card a real button rather than a styled
/// box, so it is reachable from the keyboard and announced as actionable.
pub struct StatCard {
    value: String,
    label: String,
    icon_name: String,
    css_class: Option<String>,
    detail: Option<String>,
    proportion: Option<f64>,
    activates: bool,
}

impl StatCard {
    pub fn new(value: &str, label: &str, icon_name: &str) -> Self {
        Self {
            value: value.to_string(),
            label: label.to_string(),
            icon_name: icon_name.to_string(),
            css_class: None,
            detail: None,
            proportion: None,
            activates: false,
        }
    }

    /// A semantic class (`success`, `warning`, `error`, …) tinting the icon
    /// and the figure together.
    pub fn tint(mut self, class: &str) -> Self {
        self.css_class = Some(class.to_string());
        self
    }

    /// A secondary line under the label: the share, the scope, or the change
    /// the figure should be read against.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A hairline bar under everything, filled to `fraction` (clamped to
    /// 0–1). The bar repeats what the detail line says in words, so it is
    /// reinforcement, never the only carrier of the information.
    pub fn proportion(mut self, fraction: f64) -> Self {
        self.proportion = Some(fraction.clamp(0.0, 1.0));
        self
    }

    pub fn activates(mut self) -> Self {
        self.activates = true;
        self
    }

    pub fn build(&self) -> gtk4::Widget {
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(3)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();

        let top = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Horizontal)
            .spacing(6)
            .build();
        let icon = gtk4::Image::from_icon_name(&self.icon_name);
        icon.set_pixel_size(16);
        icon.set_valign(gtk4::Align::Center);
        top.append(&icon);

        let number = gtk4::Label::builder()
            .label(&self.value)
            .halign(gtk4::Align::Start)
            .build();
        number.add_css_class("title-1");
        number.add_css_class("numeric");
        top.append(&number);
        column.append(&top);

        if let Some(class) = &self.css_class {
            // On the container so the icon and the figure are tinted together.
            top.add_css_class(class);
        }

        let caption = gtk4::Label::builder()
            .label(&self.label)
            .halign(gtk4::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .build();
        caption.add_css_class("caption");
        column.append(&caption);

        if let Some(detail) = &self.detail {
            let line = gtk4::Label::builder()
                .label(detail)
                .halign(gtk4::Align::Start)
                .xalign(0.0)
                .ellipsize(gtk4::pango::EllipsizeMode::End)
                .build();
            line.add_css_class("caption");
            line.add_css_class("dimmed");
            line.add_css_class("numeric");
            column.append(&line);
        }

        if let Some(fraction) = self.proportion {
            let bar = gtk4::ProgressBar::builder()
                .fraction(fraction)
                .valign(gtk4::Align::End)
                .hexpand(true)
                .css_classes(["stat-proportion"])
                .build();
            if let Some(class) = &self.css_class {
                // The fill follows the same semantic tint as the figure —
                // Adwaita draws the progress node with the accent colours a
                // `.success` / `.error` / … subtree remaps.
                bar.add_css_class(class);
            }
            column.append(&bar);
        }

        let accessible = format!("{} {}", self.value, self.label);
        if self.activates {
            let button = gtk4::Button::builder().child(&column).build();
            button.add_css_class("card");
            button.add_css_class("activatable");
            button.add_css_class("stat-card");
            button.update_property(&[gtk4::accessible::Property::Label(&accessible)]);
            button.upcast()
        } else {
            let card = gtk4::Box::builder()
                .orientation(gtk4::Orientation::Vertical)
                .build();
            card.add_css_class("card");
            card.add_css_class("stat-card");
            card.append(&column);
            // A plain box exposes no name to assistive technology; label it
            // with the same "value label" the button variant announces.
            card.update_property(&[gtk4::accessible::Property::Label(&accessible)]);
            card.upcast()
        }
    }
}

/// An empty-state pitch that can sit inside a page which has other content
/// above and below it.
///
/// `AdwStatusPage` is built to be a whole page: it reports a minimum height of
/// just its action, because the icon, title and description are allowed to
/// shrink away. A scrolling column allocates its children at their *minimum*
/// height, so an `AdwStatusPage` used inline is clipped to a sliver of its
/// icon. This is the same message in an ordinary box, at a size that suits a
/// section rather than a page.
pub fn inline_empty_state(
    icon_name: &str,
    title: &str,
    description: &str,
    action: Option<gtk4::Button>,
) -> gtk4::Box {
    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .halign(gtk4::Align::Center)
        .margin_top(18)
        .margin_bottom(18)
        .build();

    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_pixel_size(64);
    icon.add_css_class("dimmed");
    icon.set_accessible_role(gtk4::AccessibleRole::Presentation);
    column.append(&icon);

    let heading = gtk4::Label::builder()
        .label(title)
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .build();
    heading.add_css_class("title-3");
    column.append(&heading);

    let body = gtk4::Label::builder()
        .label(description)
        .wrap(true)
        .justify(gtk4::Justification::Center)
        .max_width_chars(64)
        .build();
    body.add_css_class("body");
    body.add_css_class("dimmed");
    column.append(&body);

    if let Some(button) = action {
        button.set_halign(gtk4::Align::Center);
        button.set_margin_top(6);
        button.add_css_class("pill");
        button.add_css_class("suggested-action");
        column.append(&button);
    }

    column
}

/// A full-page empty state.
pub fn empty_state(
    icon_name: &str,
    title: &str,
    description: &str,
    action: Option<gtk4::Button>,
) -> adw::StatusPage {
    let status = adw::StatusPage::builder()
        .icon_name(icon_name)
        .title(title)
        .description(description)
        .build();

    if let Some(button) = action {
        button.set_halign(gtk4::Align::Center);
        button.add_css_class("pill");
        button.add_css_class("suggested-action");
        status.set_child(Some(&button));
    }

    status
}

/// The spinner shown while a page's first load is in flight.
///
/// `adw::Spinner` has no start/stop — it animates whenever it is visible — so
/// showing and hiding it is the whole API.
pub fn loading_state(message: &str) -> gtk4::Box {
    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(18)
        .valign(gtk4::Align::Center)
        .halign(gtk4::Align::Center)
        .vexpand(true)
        .hexpand(true)
        .build();

    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .build();
    column.append(&spinner);

    let label = gtk4::Label::builder().label(message).build();
    label.add_css_class("dimmed");
    column.append(&label);

    column
}

/// An error state that offers the one thing worth doing about it.
pub fn error_state(
    icon_name: &str,
    title: &str,
    message: &str,
    action: Option<gtk4::Button>,
) -> adw::StatusPage {
    let status = adw::StatusPage::builder()
        .icon_name(icon_name)
        .title(title)
        .description(message)
        .build();

    if let Some(button) = action {
        button.set_halign(gtk4::Align::Center);
        button.add_css_class("pill");
        status.set_child(Some(&button));
    }

    status
}

/// A titled group. `AdwPreferencesGroup` outside a preferences page is the
/// right container: it draws the heading, the optional description and the
/// boxed-list frame from stock styling.
pub fn group(title: &str, description: &str) -> adw::PreferencesGroup {
    let group = adw::PreferencesGroup::builder().title(title).build();
    if !description.is_empty() {
        group.set_description(Some(description));
    }
    group
}

/// A read-only fact: a quiet label and the value that matters.
///
/// `.property` swaps Adwaita's emphasis so the *subtitle* reads as the value
/// and the title as its label, which is the right way round for a detail page.
pub fn property_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(if value.is_empty() { "—" } else { value })
        .build();
    row.add_css_class("property");
    row.set_subtitle_selectable(true);
    row
}

/// A property row whose value is a status badge rather than text.
pub fn status_row(
    title: &str,
    icon_name: &str,
    label: &str,
    css_class: &str,
    detail: Option<&str>,
) -> adw::ActionRow {
    let builder = adw::ActionRow::builder().title(title);
    let row = match detail {
        Some(detail) => builder.subtitle(detail).build(),
        None => builder.build(),
    };
    row.add_suffix(&status_badge(icon_name, label, css_class));
    row
}

/// A row with a button on the end.
pub fn action_row(
    title: &str,
    subtitle: &str,
    button_label: &str,
    button_classes: &[&str],
) -> (adw::ActionRow, gtk4::Button) {
    let row = adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build();

    let button = gtk4::Button::builder()
        .label(button_label)
        .valign(gtk4::Align::Center)
        .build();
    for class in button_classes {
        button.add_css_class(class);
    }

    row.add_suffix(&button);
    (row, button)
}

/// A row whose value can be copied to the clipboard.
///
/// Copying is offered rather than the value being selectable-only because the
/// values this is used for — a tunnel ID, a CNAME target, a command — are
/// exactly the ones a user needs to paste somewhere else.
pub fn copyable_row(title: &str, value: &str) -> adw::ActionRow {
    let row = property_row(title, value);

    let button = gtk4::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy")
        .valign(gtk4::Align::Center)
        .build();
    button.add_css_class("flat");
    button.update_property(&[gtk4::accessible::Property::Label(&format!("Copy {title}"))]);

    let value = value.to_string();
    button.connect_clicked(move |button| {
        button.clipboard().set_text(&value);
        toast(button, "Copied to the clipboard");
    });

    row.add_suffix(&button);
    row
}

/// A monospace block for a command, a unit file or a log excerpt.
pub fn code_block(text: &str) -> gtk4::Widget {
    let view = gtk4::TextView::builder()
        .editable(false)
        .monospace(true)
        .cursor_visible(false)
        .wrap_mode(gtk4::WrapMode::WordChar)
        .top_margin(12)
        .bottom_margin(12)
        .left_margin(12)
        .right_margin(12)
        .build();
    view.buffer().set_text(text);
    view.add_css_class("code-block");

    let scroller = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Automatic)
        .propagate_natural_height(true)
        .max_content_height(280)
        .child(&view)
        .build();

    let frame = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .build();
    frame.add_css_class("card");
    frame.append(&scroller);
    frame.upcast()
}

/// A code block with a copy button above it.
pub fn copyable_code_block(text: &str, description: &str) -> gtk4::Box {
    let column = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .spacing(6)
        .build();

    let header = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .build();

    if !description.is_empty() {
        let label = gtk4::Label::builder()
            .label(description)
            .halign(gtk4::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .hexpand(true)
            .build();
        label.add_css_class("caption");
        label.add_css_class("dimmed");
        header.append(&label);
    }

    let copy = gtk4::Button::builder()
        .child(
            &adw::ButtonContent::builder()
                .icon_name("edit-copy-symbolic")
                .label("Copy")
                .build(),
        )
        .valign(gtk4::Align::Center)
        .build();
    copy.add_css_class("flat");

    let payload = text.to_string();
    copy.connect_clicked(move |button| {
        button.clipboard().set_text(&payload);
        toast(button, "Copied to the clipboard");
    });
    header.append(&copy);

    column.append(&header);
    column.append(&code_block(text));
    column
}

/// A persistent notice: an icon, a sentence and an optional action in the
/// same card frame as the rest of the page.
///
/// This replaces the full-width `AdwBanner` strip on pages that already use
/// cards. It keeps the notice's semantic tint on the text and icon only, so
/// the surface stays the normal card colour in every theme.
pub fn notice(
    icon_name: &str,
    text: &str,
    css_class: &str,
    action: Option<gtk4::Button>,
) -> gtk4::Box {
    let card = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .build();
    card.add_css_class("card");
    card.add_css_class("notice");

    let line = inline_message(icon_name, text, css_class);
    line.set_hexpand(true);
    card.append(&line);

    if let Some(button) = action {
        button.set_valign(gtk4::Align::Center);
        card.append(&button);
    }

    card
}

/// One line of an inline message: icon, text, semantic tint.
pub fn inline_message(icon_name: &str, text: &str, css_class: &str) -> gtk4::Box {
    let line = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Horizontal)
        .spacing(12)
        .build();
    line.add_css_class(css_class);

    let icon = gtk4::Image::from_icon_name(icon_name);
    icon.set_valign(gtk4::Align::Start);
    line.append(&icon);

    let label = gtk4::Label::builder()
        .label(text)
        // Fill, not Start: with Start the label is allocated its natural
        // width even when it expands, so a wrapped sentence stops at its
        // natural width instead of using the space beside an action button.
        .halign(gtk4::Align::Fill)
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .build();
    label.add_css_class("body");
    line.append(&label);

    line
}

/// A header-bar-height button that shows a spinner while its work runs.
///
/// The spinner replaces the icon rather than sitting beside it, so the header
/// does not reflow every time a refresh starts.
pub struct BusyButton {
    pub button: gtk4::Button,
    stack: gtk4::Stack,
}

impl BusyButton {
    pub fn new(icon_name: &str, tooltip: &str) -> Self {
        let stack = gtk4::Stack::new();
        stack.add_named(&gtk4::Image::from_icon_name(icon_name), Some("idle"));
        stack.add_named(
            &adw::Spinner::builder()
                .width_request(16)
                .height_request(16)
                .build(),
            Some("busy"),
        );

        let button = gtk4::Button::builder()
            .child(&stack)
            .tooltip_text(tooltip)
            .valign(gtk4::Align::Center)
            .build();
        button.update_property(&[gtk4::accessible::Property::Label(tooltip)]);

        Self { button, stack }
    }

    pub fn set_busy(&self, busy: bool) {
        self.stack
            .set_visible_child_name(if busy { "busy" } else { "idle" });
        self.button.set_sensitive(!busy);
    }
}

/// Lets a widget whose own minimum width is wider than the window reflow
/// instead of being clipped.
///
/// `AdwBanner` is the case that needs it. Its label wraps, but a wrapping
/// `GtkLabel` still reports a minimum width from its own heuristics, so a
/// banner packed straight into a column is clipped at the right edge once the
/// window is narrower than that. A `GtkScrolledWindow` with both policies set
/// to `Never` drops the minimum to zero and lets the label reflow to whatever
/// width there is; nothing ever actually scrolls.
pub fn wrappable(widget: &impl IsA<gtk4::Widget>) -> gtk4::ScrolledWindow {
    gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .vscrollbar_policy(gtk4::PolicyType::Never)
        // Height propagates so the banner is as tall as its wrapped text;
        // width deliberately does not, because propagating it would put the
        // child's wide minimum straight back on the scroller and undo the
        // whole point.
        .propagate_natural_height(true)
        .child(widget)
        .build()
}

/// A table or log list in the same card frame as every group.
///
/// `GtkColumnView` and `GtkListView` draw no frame of their own. Wrapped
/// here, a table reads as one more card on a page of cards — same
/// background, radius and hairline (see `.table-card` in style.css).
/// `overflow: hidden` clips the scrolled rows to the rounded corners; the
/// side margins line the frame up with the toolbars above it, which carry
/// their own 12px margins.
pub fn table_card(child: &impl IsA<gtk4::Widget>) -> gtk4::Box {
    let frame = gtk4::Box::builder()
        .orientation(gtk4::Orientation::Vertical)
        .margin_start(12)
        .margin_end(12)
        .margin_bottom(12)
        .vexpand(true)
        .hexpand(true)
        .build();
    frame.add_css_class("table-card");
    frame.set_overflow(gtk4::Overflow::Hidden);
    frame.append(child);
    frame
}

/// Finds the window's toast overlay from any widget inside it.
///
/// Dialogs (`AdwDialog`) present above the window rather than inside the
/// overlay, so the ancestor walk fails for anything in a dialog. The
/// fallback searches the root window's subtree instead, which keeps copy
/// buttons in dialogs from silently losing their feedback.
pub fn toast_overlay(widget: &impl IsA<gtk4::Widget>) -> Option<adw::ToastOverlay> {
    if let Some(overlay) = widget
        .as_ref()
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast()
    {
        return Some(overlay);
    }
    let window = widget
        .as_ref()
        .ancestor(gtk4::Window::static_type())
        .and_downcast::<gtk4::Widget>()?;
    let mut queue = vec![window];
    while let Some(node) = queue.pop() {
        if let Ok(overlay) = node.clone().downcast::<adw::ToastOverlay>() {
            return Some(overlay);
        }
        let mut child = node.first_child();
        while let Some(next) = child {
            queue.push(next.clone());
            child = next.next_sibling();
        }
    }
    None
}

/// Raises a plain toast from anywhere in the window.
pub fn toast(widget: &impl IsA<gtk4::Widget>, message: &str) {
    if let Some(overlay) = toast_overlay(widget) {
        overlay.add_toast(adw::Toast::new(message));
    }
}

/// A toast that stays until dismissed, for something the user must not miss —
/// a partial failure, most often.
pub fn sticky_toast(widget: &impl IsA<gtk4::Widget>, message: &str, button: Option<(&str, &str)>) {
    let Some(overlay) = toast_overlay(widget) else {
        return;
    };

    let toast = adw::Toast::new(message);
    toast.set_timeout(0);
    toast.set_priority(adw::ToastPriority::High);

    match button {
        Some((label, action)) => {
            toast.set_button_label(Some(label));
            toast.set_action_name(Some(action));
        }
        None => {
            toast.set_button_label(Some("Dismiss"));
            toast.connect_button_clicked(|toast| toast.dismiss());
        }
    }

    overlay.add_toast(toast);
}

/// A confirmation dialog for something destructive.
///
/// Both the default response (Enter) and the close response (Escape) are the
/// harmless one, so neither key can destroy anything.
pub fn confirm_destructive(
    parent: &impl IsA<gtk4::Widget>,
    heading: &str,
    body: &str,
    warnings: &[String],
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));

    if !warnings.is_empty() {
        let column = gtk4::Box::builder()
            .orientation(gtk4::Orientation::Vertical)
            .spacing(12)
            .build();
        for warning in warnings {
            column.append(&inline_message(
                "dialog-warning-symbolic",
                warning,
                "warning",
            ));
        }
        dialog.set_extra_child(Some(&column));
    }

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm();
        }
    });

    dialog.present(Some(parent.as_ref()));
}

/// A confirmation for something disruptive but not destructive — restarting a
/// connector, say.
pub fn confirm(
    parent: &impl IsA<gtk4::Widget>,
    heading: &str,
    body: &str,
    confirm_label: &str,
    on_confirm: impl Fn() + 'static,
) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("confirm", confirm_label);
    dialog.set_response_appearance("confirm", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("confirm"));
    dialog.set_close_response("cancel");

    dialog.connect_response(None, move |_, response| {
        if response == "confirm" {
            on_confirm();
        }
    });

    dialog.present(Some(parent.as_ref()));
}

/// A dialog that just says something, with one way out.
pub fn inform(parent: &impl IsA<gtk4::Widget>, heading: &str, body: &str) {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(parent.as_ref()));
}

/// Formats a timestamp the way a desktop application should: relative for
/// anything recent, absolute in the user's local time beyond that.
pub fn format_time(time: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(time) = time else {
        return "—".into();
    };

    let delta = chrono::Utc::now() - time;
    let seconds = delta.num_seconds();

    if seconds < 0 {
        return time
            .with_timezone(&chrono::Local)
            .format("%-d %b %Y, %H:%M")
            .to_string();
    }

    match seconds {
        0..=44 => "just now".into(),
        45..=89 => "a minute ago".into(),
        90..=3599 => format!("{} minutes ago", (seconds + 30) / 60),
        3600..=7199 => "an hour ago".into(),
        7200..=86399 => format!("{} hours ago", seconds / 3600),
        86400..=172_799 => "yesterday".into(),
        172_800..=604_799 => format!("{} days ago", seconds / 86_400),
        _ => time
            .with_timezone(&chrono::Local)
            .format("%-d %b %Y")
            .to_string(),
    }
}

/// An exact local timestamp, for a tooltip beside a relative one.
pub fn format_exact_time(time: Option<chrono::DateTime<chrono::Utc>>) -> String {
    match time {
        Some(time) => time
            .with_timezone(&chrono::Local)
            .format("%-d %B %Y at %H:%M:%S %Z")
            .to_string(),
        None => "Never".into(),
    }
}

/// Pluralises a count without the "1 items" problem.
pub fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeDelta, Utc};

    #[test]
    fn relative_time_reads_as_english() {
        let ago = |seconds: i64| format_time(Some(Utc::now() - TimeDelta::seconds(seconds)));

        assert_eq!(ago(5), "just now");
        assert_eq!(ago(60), "a minute ago");
        assert_eq!(ago(120), "2 minutes ago");
        assert_eq!(ago(3600), "an hour ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(90_000), "yesterday");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }

    #[test]
    fn a_time_far_in_the_past_becomes_a_date() {
        let old = format_time(Some(Utc::now() - TimeDelta::days(60)));
        assert!(!old.contains("ago"), "got {old}");
    }

    #[test]
    fn a_missing_time_is_an_em_dash_not_an_epoch() {
        assert_eq!(format_time(None), "—");
        assert_eq!(format_exact_time(None), "Never");
    }

    #[test]
    fn a_future_time_is_shown_absolutely_rather_than_as_negative_ago() {
        let future = format_time(Some(Utc::now() + TimeDelta::hours(3)));
        assert!(!future.contains("ago"), "got {future}");
        assert!(!future.contains('-'), "got {future}");
    }

    #[test]
    fn plurals_are_not_embarrassing() {
        assert_eq!(plural(0, "tunnel", "tunnels"), "0 tunnels");
        assert_eq!(plural(1, "tunnel", "tunnels"), "1 tunnel");
        assert_eq!(plural(2, "tunnel", "tunnels"), "2 tunnels");
    }
}
