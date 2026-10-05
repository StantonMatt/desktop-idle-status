# Debian packages and Launchpad

The stable archive is
[`ppa:stantonmatt/plasma-visual-screensaver`](https://launchpad.net/~stantonmatt/+archive/ubuntu/plasma-visual-screensaver).
Wait for a successful **Published** Resolute build before installing:

```sh
sudo add-apt-repository ppa:stantonmatt/plasma-visual-screensaver
sudo apt update
sudo apt install desktop-idle-status
```

The service starts with the graphical session. Add **Desktop Idle Status** to
Plasma's system tray using its normal tray settings. Its **Start Service** button
runs `systemctl --user start desktop-idle-status.service`. The package also
supports D-Bus activation; the activation file's `SystemdService` and the unit's
`Type=dbus` / `BusName` identify the same session service.

The recommended `desktop-idle-status-kwin-bridge` package is a native plugin
running inside KWin. Distribution builds enable it at the next login. After
installation, users can load it immediately with:

```sh
qdbus6 org.kde.KWin /Plugins LoadPlugin desktopidlestatusbridge
```

`LoadPlugin` is the method on `org.kde.KWin.Plugins`; it was verified in the
private nested-compositor harness. Developers must never run it on the real
session during testing. The persistent kill switch is:

```sh
kwriteconfig6 --file kwinrc --group Plugins --key desktopidlestatusbridgeEnabled false
```

Log out and back in for this switch to take effect. It does not unload a running
plugin. Remove the explicit setting, or set it to `true`, to enable it again.
The service does not load the bridge or change KWin settings.

## Build a release

Use Ubuntu/Kubuntu 26.04 (Resolute), Rust/Cargo 1.93 or newer, the dependencies
listed in `debian/control`, and `devscripts`, `lintian`, `dput`, `gnupg`,
`python3`, `xz-utils` and `git`. Install these tools in a build container/chroot
when working on a development desktop. No package installation is necessary
on the host for the container workflow below.

The upstream version is checked across both CMake projects, both plugin
metadata files, Cargo.toml, Cargo.lock and debian/changelog. Run:

```sh
scripts/check-versions.py
scripts/build-ppa-source.sh
```

The default upstream ref is `refs/tags/v<VERSION>` (a branch with that name is
rejected). The script pins the resolved commit and exports it without
`debian/`, overlays the current packaging, and builds a 3.0 (quilt) source.
Crates are vendored from the exported, committed `service/Cargo.lock` into
`desktop-idle-status_<VERSION>.orig-vendor.tar.xz`. Only vendoring requires network.
Debian generates a private Cargo source-replacement config, uses an empty
Cargo home, and builds/tests with `--frozen` and `CARGO_NET_OFFLINE=true`.
The crate license texts and source attributions are added to the generated
DEP-5 copyright file, followed by the reviewed third-party stanzas from
`debian/copyright`. `scripts/vendor-copyright-reviewed.json` pins the complete
crate file inventories; dependency updates require auditing all license/notice
files and source headers before updating those pins. The SQLite library is bundled by the locked rusqlite dependency;
there is no system SQLite build dependency.

Ubuntu's packaged `librust-rusqlite-dev` is 0.29, incompatible with the locked
0.38 dependency, so packaged Rust crates are not used. Reuse the published orig
components when bumping only a PPA revision. Do not change Cargo.lock without
bumping the upstream version and creating new orig components.

Source and binary packages, source/binary Lintian results, and checksums are
written under gitignored `dist/ppa/`. Large temporary builds use
`~/.cache/agent-scratch/desktop-idle-status/` and are removed on exit. Binary
builds invoked by the source script run through `heavy`. To build an existing
source separately:

```sh
heavy scripts/build-deb.sh dist/ppa/desktop-idle-status_<VERSION>-1ppa1~resolute1.dsc dist/ppa
```

For committed validation before the tag exists:

```sh
PPA_SOURCE_REF=HEAD PPA_BUILD_BINARY=0 scripts/build-ppa-source.sh
```

For uncommitted implementation checks only, also set
`PPA_INCLUDE_WORKTREE=1`. This preserves the Git-exported orig and captures
working-tree changes in a generated `validation-worktree` quilt patch. These
artifacts are **not for upload**. Publishing requires a clean tree (including untracked files and ignored
files under `debian/`) and the release tag; it rejects this option and `PPA_SOURCE_REF` overrides.

`dh_auto_test` runs the pure Rust library and binary unit tests. The private-bus
integration tests need their own dbus-daemon/Wayland fixtures and are excluded
from Launchpad builds; run `heavy cargo test --manifest-path service/Cargo.toml
--all-targets --offline --locked`, `tests/plasmoid/unit.sh` and
`tests/kwin-bridge/run.sh` before release. All desktop tests use private sessions.

## Prove a clean offline build

Prepare an `ubuntu:26.04` build image with build-essential, the complete
`debian/control` Build-Depends (for example via `mk-build-deps --install`),
Lintian, and dpkg-dev in a networked layer. Then extract/build the `.dsc` with
networking disabled. For an image named `desktop-idle-status-build`:

```sh
heavy docker run --rm --network none \
  -v "$PWD/dist/ppa:/artifacts" desktop-idle-status-build \
  sh -ec 'mkdir /build; cd /build; dpkg-source -x /artifacts/desktop-idle-status_<VERSION>-1ppa1~resolute1.dsc source; cd source; dpkg-buildpackage -b -us -uc; cd ..; lintian --fail-on error ./*.changes; cp ./*.deb ./*.changes ./*.buildinfo /artifacts/'
```

Install the two regular `.deb` files using `apt install` in a separate clean
Ubuntu container; inspect `dpkg -L`, generated maintainer scripts,
`systemd-analyze --user verify`, D-Bus activation and `kpackagetool6 --type
Plasma/Applet --packageroot /usr/share/plasma/plasmoids --list`. The bridge's embedded
`org.kde.kwin.PluginFactoryInterfaceVERSION` IID must match that container's
KWin upstream version. Never install these test packages on the host. Remove
task containers/images when finished; keep the Ubuntu base image.

## Sign and publish

The maintainer/key/PPA match Plasma Visual Screensaver. Launchpad must already
recognize fingerprint `F78C76E2C1BE76E07CF81202B4468B1BCFFD55F5`; the corresponding
private key must be available locally to `debsign`. Launchpad accepts signed
sources, not locally built binaries.

After committing/reviewing and creating the matching upstream tag, dry-run:

```sh
PPA_DRY_RUN=1 scripts/publish-ppa.sh
```

This builds/lints, signs with the registered key, and calls `dput --simulate`.
Then publish deliberately:

```sh
scripts/publish-ppa.sh
```

Defaults are `PPA_TARGET=ppa:stantonmatt/plasma-visual-screensaver` and
`PPA_SIGNING_KEY=F78C76E2C1BE76E07CF81202B4468B1BCFFD55F5`. They can be overridden.
Uploads require a new source version every time. Monitor Launchpad acceptance,
build and publication separately; do not announce an accepted-but-unbuilt source.

## Rebuild when Ubuntu updates KWin

KWin refuses a plugin with an IID built for another upstream KWin version.
The service reports the mismatch. `BuiltAgainstPackage` records the exact
libkwin6 package version, including epoch. The bridge depends on libkwin6 at
least that version, with **no upper bound**, so it cannot hold back KWin security
updates.

Check Ubuntu's published Release/Updates/Security pockets, using the local
latest changelog entry or the latest published PPA bridge:

```sh
scripts/check-kwin-rebuild.sh
scripts/check-kwin-rebuild.sh --ppa
```

Exit status 0 means the upstream version matches, 1 means rebuild needed, and
2 means unavailable/error. Ubuntu-only package revisions with the same upstream
version do not change the IID. The check is read-only and uses Launchpad HTTPS;
`--series` and `--arch` default to `resolute` and `amd64`.

Whenever Ubuntu publishes a new libkwin6 **upstream** version, add a no-change
changelog entry with a bumped PPA revision (for example
`<VERSION>-1ppa2~resolute1`) and the line
`Bridge built against libkwin6 NEW_VERSION.` Update the CMake minimum KWin
requirement only if APIs require it. Build with the current archive's matching
kwin-dev/libkwin6, verify IID and private-compositor behavior, review the changed
packaging, dry-run and publish. Keep the same upstream tag/orig components for
this packaging-only rebuild. Launchpad builders must have the new version
available before uploading. Resolute's current build baseline is
`4:6.6.6-0ubuntu0.1`.
