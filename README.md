# Desktop Idle Status

A KDE Plasma status widget for identifying desktop idle blockers.

Components: a KWin C++ bridge, a Rust session service, and a QML system-tray
widget. The bridge reports KWin's
effective Wayland idle inhibitors and latent inhibitors from minimized, hidden,
or off-desktop windows. The D-Bus contract is in
[docs/kwin-bridge-dbus.md](docs/kwin-bridge-dbus.md).

Use **Ignore** / **Stop Ignoring** in the widget to persist an app-wide
screensaver preference. Ignored apps are excluded from new blocking history.
With input-only idle tracking and exact attribution available, they do not
count in blocker status or the tray, and the service shows the screensaver
after its timeout when only ignored apps are blocking. Sleep blockers are
unaffected.

## Install on Kubuntu 26.04

After the Resolute packages are published in the PPA:

```sh
sudo add-apt-repository ppa:stantonmatt/plasma-visual-screensaver
sudo apt update
sudo apt install desktop-idle-status
```

The service and recommended bridge become active at the next login. Add
**Desktop Idle Status** using Plasma's system-tray settings. See [PPA.md](PPA.md)
for immediate activation, the bridge kill switch, offline Debian builds and
maintainer publishing instructions. The project is GPL-3.0-or-later; see
[LICENSE](LICENSE).

## Development

Build prerequisites: matching KWin 6.6.6 development files, CMake, Ninja, ECM,
Qt 6 development files (including Widgets/DBus), KF6 CoreAddons, `libdrm-dev`.
Tests also need `libwayland-dev`, `wayland-scanner`, Qt Wayland protocol XMLs,
`kwin_wayland`, `dbus-run-session`, `python3-dbus`, and `python3-gi`.

```sh
cmake -S kwin-bridge -B ~/.cache/agent-scratch/desktop-idle-status/bridge-build -G Ninja
cmake --build ~/.cache/agent-scratch/desktop-idle-status/bridge-build
tests/kwin-bridge/run.sh
```

The test builds both targets, verifies the metadata IID, and runs a virtual,
offscreen KWin on a new private bus with isolated HOME and XDG directories.
It removes the temporary session directory and stops all test processes even
on failure. Build outputs stay in the project's scratch directory.
The private bus has no service auto-activation directories. Its AppArmor hook
is disabled only within that test daemon to support sandboxed execution; the
normal desktop bus and system policy are unchanged.

Do not install or load development plugins in the real desktop session.
Development builds default to disabled. The persistent kill switch for users
of the packaged bridge is:

```sh
kwriteconfig6 --file kwinrc --group Plugins --key desktopidlestatusbridgeEnabled false
```

That command disables loading on the next compositor startup; it does not unload
a running plugin. Do not run it against the real session during development or
testing. Native KWin plugins run inside the compositor and must be rebuilt for
every KWin release.
