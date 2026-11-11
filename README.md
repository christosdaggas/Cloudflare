# Tunnel Manager

![Tunnel Manager](docs/app.png)

A desktop client for managing remotely configured Cloudflare Tunnels: their
hostnames, their DNS records and — optionally, over SSH — the `cloudflared`
service on the machines that serve them.

Built with **Tauri 2**, **Rust** and **React + TypeScript**. The window uses the
system webview (WebKitGTK on Linux), follows the desktop's light or dark
preference and its accent colour, and keeps the UI and the network work off the
same thread.

Tunnel Manager is an unofficial, third-party tool. It is not affiliated with or
endorsed by Cloudflare, Inc.

## What it does

- Create, rename and delete tunnels, and see which are running.
- Publish hostnames on your domains with one reviewed click, and remove them
  cleanly. Every ingress rule and `originRequest` field the application did not
  set is preserved exactly.
- Read Cloudflare's connector health separately from systemd state on the
  machine. The two are never merged into one misleading switch.
- Optionally control `cloudflared` over SSH with pinned host keys and a fixed
  allowlist of operations, or control the local service with no SSH at all.
- Pin QUIC or HTTP/2 for a connector.

Your Cloudflare **API token** is stored in the desktop credential store
(GNOME Keyring or KWallet) — never in a file — and only the permissions it needs
are asked for.

## Build from source (Fedora / GNOME)

Tauri 2 on Linux uses **WebKitGTK 4.1** and **GTK 3**; the GTK 4/libadwaita
stack the previous build used is no longer required.

```sh
sudo dnf install rust cargo nodejs npm \
  webkit2gtk4.1-devel libsoup3-devel gtk3-devel \
  javascriptcoregtk4.1-devel pkgconf-pkg-config gcc gcc-c++
```

Then build the frontend and the binary:

```sh
git clone https://github.com/christosdaggas/Cloudflare.git
cd Cloudflare
npm --prefix frontend ci
npm --prefix frontend run build
cargo build --release --locked
./target/release/tunnel-manager
```

For development, run Vite and Tauri separately:

```sh
npm --prefix frontend run dev      # terminal 1: http://localhost:1420
cargo run                          # terminal 2: the Tauri window
```

The debug binary loads the Vite dev server; the release binary embeds the built
frontend.

## Getting started

1. Create an API token at **Cloudflare → My Profile → API Tokens**. It needs
   *Cloudflare Tunnel → Edit*, *Zone → Read* and *DNS → Edit* — nothing else.
2. Paste it into the app and tick **Remember this token**. That's it.

To control `cloudflared` on another machine, register it on the Remote Hosts
page. Everything else works without any setup.

## Removing the app

The API token is in the desktop credential store; use **Settings → Remove Saved
Token** to delete it. Your data is not deleted: settings and the activity log
are in `~/.local/share/tunnel-manager/`.

## Packaging

See [dist/rpm/README.md](dist/rpm/README.md) for the local Fedora RPM build.
Only the release binary, desktop entry, AppStream metadata and icons are
installed; the frontend assets are compiled into the binary.

## Links

- Website: <https://chrisdaggas.com>
- Source and issues: <https://github.com/christosdaggas/Cloudflare>
- Contact: <info@chrisdaggas.com>
- Licence: [MIT](LICENSE)
