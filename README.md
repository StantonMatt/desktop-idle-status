# Desktop Idle Status

A KDE Plasma status widget for identifying desktop idle blockers.

Components: a KWin C++ bridge, a Rust service, and a QML plasmoid. Slice 1
implements only the bridge and its isolated integration test. It reports KWin's
effective Wayland idle inhibitors and latent inhibitors from minimized, hidden,
or off-desktop windows. The D-Bus contract is in
[docs/kwin-bridge-dbus.md](docs/kwin-bridge-dbus.md).

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

Do not install or load this experimental plugin in the real desktop session.
It defaults to disabled. For a future authorized installation, the persistent
kill switch is:

```sh
kwriteconfig6 --file kwinrc --group Plugins --key desktopidlestatusbridgeEnabled false
```

That command disables loading on the next compositor startup; it does not unload
a running plugin. Do not run it against the real session during development or
testing. Native KWin plugins run inside the compositor and must be rebuilt for
every KWin release.
