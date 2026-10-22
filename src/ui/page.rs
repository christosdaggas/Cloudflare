//! The page frame every product page sits inside.
//!
//! One constant, which every page reads so that a paragraph on the Dashboard
//! and a paragraph on Settings are the same width. The pages themselves are
//! `AdwBin` subclasses that build their own content — see
//! `ui/dashboard_page.rs` for the shape they all follow.
//!
//! Every page wraps its content column in an `AdwClamp` at this width, with
//! the tightening threshold 200 below it. No page is exempt, and no page
//! uses `AdwPreferencesPage`: the page widget narrows its groups to about
//! two thirds of the available width, so its cards never line up with the
//! full-clamp groups everywhere else.

/// How wide page content is allowed to grow before `AdwClamp` stops it.
///
/// GNOME's default is 600 (tightening at 400) — right for a preferences page.
/// These pages carry wider rows — a hostname, its origin service and a status
/// badge on one line — so they get a little more room. Anything above about
/// 1000 starts to read as an unbounded line length.
pub const CONTENT_WIDTH: i32 = 860;
