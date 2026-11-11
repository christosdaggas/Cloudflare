# Tunnel Manager — local Fedora RPM
#
# The tarball ships a release binary built with `cargo build --release`
# (strip = true), so there are no debug symbols or sources to split out. The
# exact staging commands are in README.md next to this file.
%global debug_package %{nil}

Name:           tunnel-manager
Version:        1.0.0
Release:        1%{?dist}
Summary:        Manage remotely configured Cloudflare Tunnels

License:        MIT
URL:            https://chrisdaggas.com
Source0:        %{name}-%{version}.tar.gz

# Tauri 2 uses WebKitGTK 4.1 with GTK 3 on Linux. The soname dependencies are
# detected automatically; these name the stack the interface needs.
Requires:       webkit2gtk4.1
Requires:       gtk3
Requires:       libsoup3
Recommends:     gnome-keyring

%description
Tunnel Manager is a desktop client for the Cloudflare API. It connects
with a scoped API token, lists the tunnels on an account, and lets you create
them, publish public hostnames to them, inspect their connectors and remove
them again — all through remotely managed tunnel configuration, so nothing
has to be edited on a server by hand.

Registering a remote Linux host over SSH is optional. When you do, the
application can start, stop, restart and inspect the cloudflared systemd
service on that host through a fixed set of allowed operations, with strict
host-key verification and no arbitrary command execution.

This is an unofficial third-party client. It is not affiliated with,
endorsed by, or produced by Cloudflare, Inc.

%prep
%setup -q

%build
# The release binary is built beforehand with `cargo build --release` after
# the frontend build, and shipped in the source tarball; there is nothing to
# compile here.

%install
install -Dm755 tunnel-manager %{buildroot}%{_bindir}/tunnel-manager
install -Dm644 com.chrisdaggas.tunnelmanager.desktop \
    %{buildroot}%{_datadir}/applications/com.chrisdaggas.tunnelmanager.desktop
install -Dm644 com.chrisdaggas.tunnelmanager.metainfo.xml \
    %{buildroot}%{_datadir}/metainfo/com.chrisdaggas.tunnelmanager.metainfo.xml
install -Dm644 icons/hicolor/scalable/apps/com.chrisdaggas.tunnelmanager.svg \
    %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/com.chrisdaggas.tunnelmanager.svg
install -Dm644 icons/hicolor/symbolic/apps/com.chrisdaggas.tunnelmanager-symbolic.svg \
    %{buildroot}%{_datadir}/icons/hicolor/symbolic/apps/com.chrisdaggas.tunnelmanager-symbolic.svg
install -Dm644 LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE

%files
%license LICENSE
%doc README.md
%{_bindir}/tunnel-manager
%{_datadir}/applications/com.chrisdaggas.tunnelmanager.desktop
%{_datadir}/metainfo/com.chrisdaggas.tunnelmanager.metainfo.xml
%{_datadir}/icons/hicolor/scalable/apps/com.chrisdaggas.tunnelmanager.svg
%{_datadir}/icons/hicolor/symbolic/apps/com.chrisdaggas.tunnelmanager-symbolic.svg

%changelog
* Thu Oct 22 2026 Christos A. Daggas <info@chrisdaggas.com> - 1.0.0-1
- First stable release for Fedora 44 and the GNOME desktop.
- Manage tunnels, public hostnames, DNS records and zones with a scoped token.
- Optional SSH remote control and local cloudflared control.
