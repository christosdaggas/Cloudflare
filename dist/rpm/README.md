# RPM installation on Fedora

Tunnel Manager is built for **Fedora 44 with the GNOME desktop**. The spec in
this directory produces a small binary RPM: the release binary, desktop entry,
AppStream metadata, icons and licence.

The package is built locally rather than downloaded, because the repository
does not publish prebuilt RPMs. The binary is compiled on the machine that
builds the package, so build it on the same Fedora release you intend to
install it on.

## 1. Install the build tools

```sh
sudo dnf install rpm-build gtk4-devel libadwaita-devel rust cargo
```

## 2. Build the release binary

From the repository root:

```sh
cargo build --release --locked
```

## 3. Stage the RPM source tarball

The spec expects a tarball containing the release binary and the desktop
files. The commands below stage exactly that under `dist/rpm/build/`:

```sh
version=1.0.0
stage="dist/rpm/build/SOURCES/tunnel-manager-${version}"
rm -rf "$stage"
mkdir -p "$stage/icons/hicolor/scalable/apps" \
         "$stage/icons/hicolor/symbolic/apps"

install -m755 target/release/tunnel-manager "$stage/"
install -m644 data/com.chrisdaggas.tunnelmanager.desktop "$stage/"
install -m644 data/com.chrisdaggas.tunnelmanager.metainfo.xml "$stage/"
install -m644 data/icons/hicolor/scalable/apps/com.chrisdaggas.tunnelmanager.svg \
    "$stage/icons/hicolor/scalable/apps/"
install -m644 data/icons/hicolor/symbolic/apps/com.chrisdaggas.tunnelmanager-symbolic.svg \
    "$stage/icons/hicolor/symbolic/apps/"
install -m644 LICENSE README.md "$stage/"

tar -C dist/rpm/build/SOURCES -czf \
    "dist/rpm/build/SOURCES/tunnel-manager-${version}.tar.gz" \
    "tunnel-manager-${version}"
```

## 4. Build the RPM

```sh
rpmbuild \
  --define "_topdir $PWD/dist/rpm/build" \
  --define "_buildhost localhost" \
  -ba dist/rpm/tunnel-manager.spec
```

The package is written to:

```
dist/rpm/build/RPMS/x86_64/tunnel-manager-1.0.0-1.fc44.x86_64.rpm
```

The build directory and RPM files are excluded from git by `.gitignore`.

## 5. Install it

```sh
sudo dnf install ./dist/rpm/build/RPMS/x86_64/tunnel-manager-1.0.0-1.fc44.x86_64.rpm
```

Then launch **Tunnel Manager** from the GNOME application grid, or run:

```sh
tunnel-manager
```

GNOME Keyring is recommended (and is normally already running on a GNOME
desktop). Without a Secret Service provider the application still works, but it
cannot remember the Cloudflare API token between launches.

## Remove it

```sh
sudo dnf remove tunnel-manager
```

Your settings and activity log live in `~/.local/share/tunnel-manager/` and
are deliberately left in place. The API token is in GNOME Keyring; use
**Settings → Remove Saved Token** before removing the package if you want it
deleted too.

## Package details

- Name: `tunnel-manager`
- Version: `1.0.0`
- Licence: MIT
- Runtime: GTK 4.22+, libadwaita 1.9+
- Recommended: `gnome-keyring`
- Installed files: binary, desktop entry, AppStream metadata, two icons and
  the licence
