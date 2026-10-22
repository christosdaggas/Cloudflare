# Working on this codebase

Rules for anyone — human or agent — writing UI code here. They are short because
most of GNOME's design is already encoded in libadwaita; the job is mostly to
stop overriding it.

Verified against **GTK 4.22.4** and **libadwaita 1.9.3**. Where a claim below is
version-sensitive it names the version.

---

## 1. Never hardcode a colour

libadwaita resolves light, dark, high contrast and the user's accent colour into
CSS variables. Use them and all four work for free.

```css
/* Yes */   background-color: var(--accent-bg-color);
/* Yes */   color: color-mix(in srgb, currentColor 55%, transparent);
/* No  */   background-color: #3584e4;
```

The variables you will actually reach for:

| Variable | Light | Dark | Use for |
|---|---|---|---|
| `--window-bg-color` | `#fafafb` | `#222226` | The main content surface |
| `--window-fg-color` | `rgb(0 0 6 / 80%)` | `#ffffff` | Text on it |
| `--view-bg-color` | `#ffffff` | `#1d1d20` | Text views, list views, entries |
| `--sidebar-bg-color` | `#ebebed` | `#2e2e32` | Sidebars |
| `--secondary-sidebar-bg-color` | `#f3f3f5` | `#28282c` | A second, nested sidebar |
| `--card-bg-color` | `#ffffff` | `rgb(255 255 255 / 8%)` | `.card` and boxed lists |
| `--headerbar-bg-color` | `#ffffff` | `#2e2e32` | Header bars |
| `--popover-bg-color` | `#ffffff` | `#36363a` | Popovers |
| `--dialog-bg-color` | `#fafafb` | `#36363a` | Dialogs |
| `--accent-bg-color` | the user's accent | same | Filled accent surfaces |
| `--accent-fg-color` | `#ffffff` | `#ffffff` | Text on those |
| `--accent-color` | accent, darkened | accent, lightened | Accent **text**, on any surface |
| `--success-*`, `--warning-*`, `--error-*`, `--destructive-*` | | | Semantic states |
| `--border-color` | `color-mix(in srgb, currentColor 15%, transparent)` | | Hairlines |
| `--dim-opacity` | `55%` | `90%` in high contrast | Secondary text |

Use `--accent-bg-color` behind white text and `--accent-color` for coloured text
on a normal surface. They are different colours and swapping them fails contrast.

The legacy `@accent_bg_color` spelling still resolves, but it does **not** pick
up per-widget `--*` overrides, and four variables (`--overview-*`,
`--active-toggle-*`) have no `@` form at all. Prefer `var()`.

**Cairo is the one exception.** A `GtkDrawingArea` paints pixels and inherits
nothing from CSS, so it must read colours itself. Do it through `src/theme.rs`,
and call `theme::on_appearance_changed` to redraw — GTK does not invalidate a
drawing area when the theme changes. Nothing in this application draws with
Cairo any more — the module is kept because the moment something does, this is
where it belongs.

## 2. Never restyle typography

Adwaita sizes text in **percentages**, so it follows the user's text-scale
factor. `.title-1 { font-size: 24pt }` looks fine on your machine and breaks
accessibility on someone else's.

| Class | Weight | Size |
|---|---|---|
| `.title-1` | 800 | 181% |
| `.title-2` | 800 | 136% |
| `.title-3` | 700 | 136% |
| `.title-4` | 700 | 118% |
| `.heading` | 700 | inherit |
| `.body` | 400 | inherit, 140% line height |
| `.caption` | 400 | 82% |
| `.caption-heading` | 700 | 82% |
| `.dimmed` | — | `opacity: var(--dim-opacity)` |
| `.numeric` | — | tabular figures |
| `.monospace` | — | the user's monospace font |

`.dimmed` is the current name; `.dim-label` is the same rule kept for
compatibility. Prefer `.dimmed` in new code.

`.success`, `.warning` and `.error` do more than set a colour: they **remap
`--accent-bg-color`, `--accent-fg-color` and `--accent-color` for the whole
subtree**. A `.suggested-action` button inside a `.error` box comes out red
without touching the button. `w::confirm_destructive` relies on it.

`.accent` is the exception — its only rule is `color: var(--accent-color)`, so
it tints text and nothing else.

## 2b. Three CSS traps this project already hit

All three were found by building this app and measuring it, not by reading docs.

**`@media (prefers-color-scheme: dark)` does not work in an application
stylesheet.** Adwaita's own stylesheet uses it, so it looks like it should, but
GTK only evaluates that media feature for the theme provider. In an app
stylesheet it never matches — verified on GTK 4.22.4 by setting the dark value
to red and finding no red pixels. Define scheme-dependent tokens under a class
instead: this project keeps `.scheme-light` / `.scheme-dark` on the window in
step with `AdwStyleManager` (`sync_scheme_class` in `ui/window.rs`), and the
colours stay in CSS where they belong.

**`set_css_classes` replaces the whole list.** GTK puts its own classes on a
widget — `csd` and `background` on a client-side-decorated window are what draw
the rounded corners and shadow. Calling `set_css_classes` wipes them and squares
the window off. Always `add_css_class` / `remove_css_class`.

**`.content-pane` only exists while the split view is expanded.** Style your own
class on the content widget rather than Adwaita's, or the surface will change
colour the moment the narrow breakpoint fires. Same reasoning applies to any
internal class a container applies conditionally.

## 3. Spacing comes from a six-value scale

**3, 6, 12, 18, 24, 36.** Compose layouts from these: six inside a group, twelve
between related groups, twenty-four between sections, thirty-six for hero
whitespace. In Adwaita's own stylesheet 6px, 3px and 12px are by some distance
the three most-used values.

Adwaita does also use 1, 2, 4, 5, 8 and 9px — `.navigation-sidebar > row` is
`padding: 0 8px; margin: 0 6px 2px`, for instance. Those are *intra-widget*
values: reach for them only when drawing the inside of a widget yourself, never
for the space between things.

Other measured defaults worth knowing: `AdwClamp` is 600px wide (tightening at
400) in `sp` units; `AdwPreferencesPage` uses `margin: 24px 12px` with 24px
between groups; buttons and entries are 34px tall with a 9px radius; cards and
boxed lists use a 12px radius; `AdwActionRow` is 50px; header bars are 46px
inside an `AdwToolbarView`. Symbolic icons are drawn at 16px and used at 16, 32,
64 or 128 — no other size renders crisply.

## 4. Use the current widget, not the one you remember

GTK 4 deprecated fifty widget classes and libadwaita seven. The ones most likely
to reach for by habit:

| Do not use | Use instead | Deprecated |
|---|---|---|
| `GtkComboBox`, `GtkComboBoxText` | `GtkDropDown` + `GtkStringList` | 4.10 |
| `GtkTreeView`, `GtkListStore`, `GtkTreeStore`, all `GtkCellRenderer*` | `GtkColumnView` / `GtkListView` + a factory | 4.10 |
| `GtkIconView` | `GtkGridView` | 4.10 |
| `GtkInfoBar` | `AdwBanner` | 4.10 |
| `GtkStatusbar` | `AdwToastOverlay` | 4.10 |
| `GtkColorButton`, `GtkFontButton` | `GtkColorDialogButton`, `GtkFontDialogButton` | 4.10 |
| `GtkDialog`, `GtkMessageDialog` | `AdwAlertDialog` | 4.10 |
| `GtkFileChooserDialog`, `GtkFileChooserNative` | `GtkFileDialog` | 4.10 |
| `GtkShortcutsWindow`, `GtkShortcutLabel` | `AdwShortcutsDialog`, `AdwShortcutLabel` | 4.18 |
| `AdwFlap` | `AdwOverlaySplitView` | 1.4 |
| `AdwLeaflet` | `AdwNavigationView` / `AdwNavigationSplitView` | 1.4 |
| `AdwSqueezer` | `AdwBreakpoint` setters | 1.4 |
| `AdwMessageDialog` | `AdwAlertDialog` | 1.6 |
| `AdwAboutWindow` | `AdwAboutDialog` | 1.6 |
| `AdwPreferencesWindow` | `AdwPreferencesDialog` | 1.6 |

`GtkListBox` is **not** deprecated — it is what `.boxed-list` and every
`Adw*Row` is built on. Use it for short, static, heterogeneous lists and
`GtkListView` for anything long or dynamic.

`AdwDialog` and its subclasses present into a **parent widget**, not a window:
`dialog.present(Some(&widget))`. That is what lets them animate in-window and
become bottom sheets on a narrow screen.

## 5. Let the shell place things

- `AdwApplicationWindow` has no titlebar slot. Use `AdwToolbarView` and put an
  `AdwHeaderBar` in its top bar.
- Two header bars inside a split view divide the window controls between
  themselves. Do not add `GtkWindowControls`.
- Put the `AdwToastOverlay` outside the split view so toasts float over the
  whole window.
- Collapse responsively with `AdwBreakpoint` in `sp` units, not `px` — `sp`
  tracks the text-scale factor, so large-text users collapse at the right
  content width. Nautilus uses `max-width: 682sp`, GNOME Settings `550sp`.

## 6. Deviations this project makes on purpose

Both are marked in `data/style.css`. If you fork this, they are the first things
to reconsider.

- **Accent-tinted sidebar rows.** Stock Adwaita tints hover and selection with a
  neutral `currentColor` mix. This project uses a translucent
  `--accent-bg-color` instead, because it was asked for. It still follows the
  user's accent and still works in both schemes.
- **Flat cards.** Adwaita lifts `.card` and `.boxed-list` with three stacked
  shadow layers. This project keeps only the hairline outline
  (`--card-shadow`), which reads flat while still separating a card from the
  surface behind it. Do not simply delete the shadow: in light mode the card
  background is `#ffffff` on a `#fafafb` window and would vanish.
- **An 80px icon rail.** libadwaita has no rail: `AdwSidebarMode` is only
  `sidebar` or `page`, and the HIG documents only full sidebars and utility
  panes. A rail is a macOS/VS Code idiom. It is implemented here anyway, and the
  standard breakpoint collapse is wired up alongside it.
- **A `.status-neutral` class.** Adwaita's `.dimmed` drops a whole subtree to
  55% opacity. That is right for secondary text and wrong for a *status*: an
  "Inactive" badge is not less important than a "Down" one, and at 55% on a
  light surface the word stops meeting contrast. `.status-neutral` gives the
  badge the same quiet background as the semantic classes and leaves the text
  at full foreground.

## 7. Never colour alone

Every state this application shows carries an **icon and a word** as well as a
tint. A red dot says nothing to a colour-blind user and nothing at all to a
screen reader, and a badge whose label has been ellipsized to "D…" is a red dot
with extra steps.

`w::status_badge` in `ui/widgets.rs` is the only way a status should be drawn.
It puts the semantic class on the *box*, not the label, so the symbolic icon —
which follows `currentColor` — is recoloured with the text, and it sets one
accessible name on the box so a screen reader reads the state once rather than
twice.

The consequence for layout is that a status column has a floor: it is sized to
fit its longest label rather than to its content, and a narrow window drops
other columns first. See trap 4 in `LAYOUT.md`.

## 8. Before you commit

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build          # must be zero errors AND zero warnings
```

All four are enforced by CI.

Then look at it, in both schemes and at more than one accent, and at the
narrowest width the window supports:

```sh
gsettings set org.gnome.desktop.interface color-scheme prefer-dark
gsettings set org.gnome.desktop.interface accent-color purple
```

`contrib/fixture_api.py` populates every page without a Cloudflare account —
see `LAYOUT.md` for how to drive it.

A layout that is only ever checked in one theme is a layout that is broken in
the other.
