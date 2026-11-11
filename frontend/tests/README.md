# Isolated design preview

Start Vite with `npm --prefix frontend run dev`, then open:

- `/tests/design-preview.html` — dark theme, populated account
- `/tests/design-preview.html?theme=light` — light theme
- `/tests/design-preview.html?hosts` — registered hosts and reachability
- `/tests/design-preview.html?scenario=empty` — empty states
- `/tests/design-preview.html?scenario=error` — loading failure
- `/tests/design-preview.html?scenario=onboarding` — connection screen

This entry uses the Tauri SDK's built-in mocks, never the real bridge. Unknown
commands and operational mutations are refused. Appearance changes are local
to the fixture. The production build has only `index.html` as its entry.

Check at 1280×900 and the minimum 760×600 window size, in both themes:

- Header and content share a surface; no horizontal or sidebar border.
- Dashboard columns align, then stack in compact windows.
- Sidebar SVG clicks and F9 toggle at full width; the automatic rail has labels.
- Tunnel names are keyboard-accessible; Ctrl+F focuses the named search field.
- F10 opens the main menu; arrows/Home/End navigate; Escape returns focus.
- Dialogs focus the form (or safe confirmation), cycle Tab, block background
  interaction, and restore focus on Escape. Ctrl+N cannot replace an open dialog.
- Long forms scroll without hiding footer actions; no page-wide overflow.
- Reduced motion and high-contrast settings retain visible control states.
