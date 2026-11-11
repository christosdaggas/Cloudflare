# Working on this codebase

Tunnel Manager is a **Tauri 2** desktop application: a Rust core, a Tauri
command bridge, and a React + TypeScript frontend rendered in the system webview
(WebKitGTK on Linux). The previous GTK 4/libadwaita implementation has been
removed; do not reintroduce GTK widget code.

## Layout

| Path | What lives there |
|---|---|
| `src/domain` | The application's own model. No HTTP, no Tauri, no SSH. |
| `src/cloudflare` | Typed v4 client and the wire DTOs that map on to it. |
| `src/remote` | Optional SSH/systemd adapter, behind a trait. |
| `src/storage` | SQLite for non-secrets, the OS credential store for secrets. |
| `src/services` | Use cases and the blocking pool. |
| `src/bridge` | The only module that may mention Tauri: state, commands, DTOs. |
| `frontend/src` | React UI, design tokens, API wrappers, page components. |
| `frontend/src/pages` | One file per page. |
| `frontend/src/dialogs` | Global dialogs, mounted by `DialogHost`. |
| `frontend/src/components` | Shell and shared primitives. |

The layering is enforced by convention: nothing below `bridge` knows that a
webview exists, and nothing in `frontend` talks to SQLite, SSH or Cloudflare
directly.

## Backend rules

- **Never log a secret.** Everything written to `tracing` goes through
  `domain::activity::redact`. The API token and a tunnel's run token are
  `SecretString`; they never enter `Debug`, storage, or a state snapshot.
- **Commands return `Result<T, CommandError>`.** `CommandError.kind` is stable
  and the frontend branches on it (`host_key_unknown`, `host_key_mismatch`,
  `permission`, `validation`, …). Add a kind rather than making the frontend
  parse a message.
- **One snapshot, not deltas.** Every mutation ends with `emit_state()`, which
  sends the whole `AppStateDto` on the `state-changed` event. The frontend
  replaces its copy. This is what keeps a dropped event from leaving the UI
  permanently stale.
- **Never block the async runtime.** SQLite, the credential store and local
  processes go through `services::runtime::blocking` or
  `tokio::task::spawn_blocking`.
- **Host keys are pinned.** No operation may run against an untrusted host.
  `probe_host_key` → the trust dialog → `trust_host_key` is the only path.
  A changed key is a refusal, never a prompt.
- **Multi-step operations return step outcomes.** Publishing, creating and
  deploying can half succeed; the UI must be able to say which half.

## Frontend rules

- **Colours live in the token block** at the top of `frontend/src/styles.css`,
  under `:root` and `[data-theme="dark"]`. Never hardcode a colour in a
  component. The accent is `--accent`, set from the GNOME accent colour; use
  `--accent-text` for coloured text on a normal surface and `--accent-fg` for
  text on a filled accent surface.
- **Status is never colour alone.** Use `StatusBadge` or `Badge` with a
  symbolic icon and a word. `status.ts` is the single source of labels, tones
  and icon names; do not invent a second vocabulary.
- **Spacing comes from the scale 4, 8, 12, 16, 24, 32.** Compose pages from the
  existing `.stack`, `.section`, `.row`, `.card`, `.group` and `.property-grid`
  primitives.
- **Every action must be reachable by keyboard.** The global shortcut map is in
  `App.tsx` and documented in `ShortcutsDialog`; local controls use native
  buttons and inputs, so Tab/Enter/Escape work for free.
- **Dialogs are mounted from `DialogHost`.** Destructive confirmations go
  through `useConfirm`, whose safe action is focused by default.
- **A page owns its data loading; the shell owns navigation and the header.**
  Commands return a fresh `AppState` snapshot; apply it with `setState`.
- **The theme follows the desktop.** `bootstrap.system_theme` (read from
  gsettings) is authoritative on Linux because WebKitGTK does not reliably
  report `prefers-color-scheme`; `matchMedia` is the fallback and the
  window-focus handler refreshes it.

## IPC conventions

- Field names are **snake_case on both sides**. The Rust structs are the source
  of truth. The one exception is `OriginRequest`, which uses Cloudflare's wire
  names (`connectTimeout`, `noTLSVerify`, …) because that document is written
  straight back to the API.
- Enums are adjacently tagged: `{ "kind": "reachable", "data": { … } }`.
- `frontend/src/types.ts` mirrors the Rust DTOs. Keep it in step with
  `src/bridge/dto.rs` and the `Serialize` derives.
- Read the state once on mount (`get_state`), then listen to `state-changed`.
  Do not poll for state.

## Checks before committing

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
npm --prefix frontend run build
```

All five are expected to pass. Build the frontend before a release
`cargo build`: `tauri::generate_context!()` embeds `frontend/dist`.

## Running it

```sh
npm --prefix frontend run dev   # terminal 1: http://localhost:1420
cargo run                       # terminal 2: the Tauri window
```

The debug binary uses `devUrl`; a release build (`cargo build --release`) uses
the embedded `frontend/dist`. A debug run without Vite shows a connection-refused
page — that is expected, not a build failure.

## Deliberate choices

- The sidebar collapses to an icon rail at 980px; there is no second sidebar.
- The window is a single instance. There is no multi-window model.
- The API token is filed in the OS credential store under
  `com.chrisdaggas.tunnelmanager`; the frontend only ever receives a secret when
  the user explicitly copies a tunnel's run token or deploys one.
- `.status-neutral` exists because a quiet status is not secondary information:
  it keeps full-contrast text instead of dropping the row to `--dim-opacity`.
