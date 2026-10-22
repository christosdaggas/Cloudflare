//! Theme access.
//!
//! Almost nothing belongs here. libadwaita already resolves light/dark,
//! high-contrast and the GNOME accent colour into CSS variables, and every
//! stock widget picks them up on its own. Application code only needs this
//! module for what CSS cannot do: cross-fade a stack transition for reduced
//! motion, and re-run a callback when the accent, the scheme or high
//! contrast changes (so the window can keep its scheme classes in step).

use libadwaita as adw;
use libadwaita::prelude::*;

/// Re-runs `update` whenever anything that could change a widget's colours
/// changes: the accent, the light/dark scheme, or high contrast. It also runs
/// once immediately, so the caller never has to prime the widget itself.
///
/// The widget is held **weakly**, and the handlers are disconnected when it is
/// destroyed. That matters more than it looks: `AdwStyleManager` is a
/// process-wide singleton, so a handler that captures a widget strongly keeps
/// that widget alive for the lifetime of the process and keeps firing after
/// its window has closed. Always go through this function rather than
/// connecting to the style manager directly.
pub fn track_appearance<W>(widget: &W, update: impl Fn(&W) + 'static)
where
    W: IsA<gtk4::Widget> + Clone,
{
    update(widget);

    let manager = adw::StyleManager::default();
    let weak = widget.downgrade();
    let update = std::rc::Rc::new(update);

    let run = std::rc::Rc::new(move || {
        if let Some(widget) = weak.upgrade() {
            update(&widget);
        }
    });

    let handlers = vec![
        manager.connect_accent_color_rgba_notify({
            let run = run.clone();
            move |_| run()
        }),
        manager.connect_dark_notify({
            let run = run.clone();
            move |_| run()
        }),
        manager.connect_high_contrast_notify(move |_| run()),
    ];

    let handlers = std::cell::RefCell::new(handlers);
    widget.connect_destroy(move |_| {
        let manager = adw::StyleManager::default();
        for handler in handlers.borrow_mut().drain(..) {
            manager.disconnect(handler);
        }
    });
}

/// Zero out an animation duration when the user has asked for reduced motion.
///
/// libadwaita honours the preference inside its own widgets; a plain
/// `GtkStack` transition is the application's own responsibility.
pub fn transition_duration(widget: &impl IsA<gtk4::Widget>, default_ms: u32) -> u32 {
    if adw::is_animations_enabled(widget) {
        default_ms
    } else {
        0
    }
}
