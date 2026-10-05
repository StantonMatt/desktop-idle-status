# KWin bridge D-Bus contract

Service: `org.kde.KWin` on KWin's existing session-bus connection. Object:
`/DesktopIdleStatus`. Interface:
`io.github.StantonMatt.DesktopIdleStatus.KWinBridge1`.

The plugin does not acquire a separate bus name. It unregisters the object when
unloaded. Revisions belong to one plugin instance; reset cached state when the
service owner changes or the object is unloaded/reloaded.

| Member | D-Bus type | Meaning |
| --- | --- | --- |
| `InterfaceVersion` | read-only `u` | Constant `1` |
| `BuiltForKWin` | read-only `s` | `KWIN_PLUGIN_VERSION_STRING` (currently `6.6.6`) |
| `BuiltAgainstPackage` | read-only `s` | `libkwin6` dpkg version captured at configure time, including epoch |
| `Snapshot()` | out `t revision`, out `aa{sv} windows` | Atomic full snapshot; refreshes before returning |
| `Changed(t revision)` | signal | Emitted only when the snapshot content changes |
| `ActivateWindow(s internalId)` | out `b` | Restore and activate a known client window; false for unknown/invalid/deleted ids |

Each window dictionary contains exactly these fields:

| Field | Type | Source |
| --- | --- | --- |
| `internalId` | `s` | KWin window UUID, without braces |
| `pid` | `u` | KWin window PID, or zero if unavailable |
| `executablePath` | `s` | Surface client's cached executable path, empty if unavailable |
| `appId` | `s` | `ClientConnection::securityContextAppId()` |
| `desktopFileName` | `s` | KWin desktop file name |
| `resourceClass` | `s` | KWin resource class (xdg toplevel app ID for normal Wayland clients) |
| `caption` | `s` | KWin caption |
| `effective` | `b` | Exact membership in `KWin::input()->idleInhibitors()` |
| `notEffectiveReason` | `s` | `""`, `"minimized"`, `"other-desktop"`, or `"hidden"` |

`appId` is not `xdg_toplevel.app_id`: ordinary clients without a Wayland security
context correctly have an empty value. These identity strings are not an
authorization mechanism. The protocol carries no application-supplied inhibition
reason.

The snapshot contains every effective window and also managed, non-deleted,
non-internal windows whose `surface()->inhibitsIdle()` is true while they are
ineffective. Internal/unmanaged windows are excluded from latent reporting just
as they are excluded by KWin's idle-inhibition policy. Subsurface inhibition is
included through KWin's surface predicate. Rows are sorted by `internalId`, so
workspace ordering does not produce false changes. Multiple inhibitor objects
on the same window produce one row.

Effective rows always have an empty reason. For latent rows, reason precedence
is minimized, then other desktop, then hidden (including show-desktop-hidden).
Occlusion is not a KWin inhibition test. Activity policy is not added by this
bridge. Effective state is never inferred from visibility.

Revision starts at zero with an empty snapshot and increments for any row or
field change, including captions and identity, not only inhibitor count changes.
Consumers subscribe to `Changed` and reread `Snapshot`; a signal carries no rows.
An initial nonempty state produces a nonzero revision. No unchanged poll or
method call emits a signal.

The bridge mirrors KWin 6.6.6's triggers: surface inhibition, desktops, minimized,
hidden and closed changes; workspace window add/remove and current desktop
changes. It also watches caption/class/desktop-file changes. A zero-millisecond
single-shot timer coalesces these events after KWin updates its effective list.
A one-second safety poll compares complete snapshots and catches late surface
assignment, show-desktop changes and changes lacking public signals.

`ActivateWindow` is the sole write action. It looks up any managed client window
by UUID, switches to the first assigned desktop when necessary, unminimizes it,
and calls KWin's `Workspace::activateWindow`. True means the request was issued;
it does not guarantee a focus policy outcome. It cannot remove an inhibitor or
change idle policy. All operations run on KWin's main thread; retained windows
and surfaces use `QPointer`. The plugin adds no I/O, files, threads, rendering,
effects or input filters.

Implementation references (matching the installed ABI):

- [KWin 6.6.6 idle_inhibition.cpp](https://github.com/KDE/kwin/blob/v6.6.6/src/idle_inhibition.cpp)
- [KWin 6.6.6 input.h](https://github.com/KDE/kwin/blob/v6.6.6/src/input.h)
- [KRunner integration factory](https://github.com/KDE/kwin/blob/v6.6.6/src/plugins/krunner-integration/main.cpp)
- [KRunner integration D-Bus and activation](https://github.com/KDE/kwin/blob/v6.6.6/src/plugins/krunner-integration/windowsrunnerinterface.cpp)

Testing is restricted to `tests/kwin-bridge/run.sh`. Each invocation has its own
bridge/client builds and HOME, runtime, config, data, cache and state directories
under the on-disk project scratch directory. TERM, INT and HUP reach the Python
harness; the wrapper waits for its compositor/client cleanup before removing
these directories. The run includes concurrent invocation and cancellation
checks, verifying that the harness, clients, compositor and private bus exit.

The separate test-only oracle plugin reads `input()->idleInhibitors()` directly
and controls hidden/show-desktop state in that private compositor. It is never
installed and is not part of the bridge's D-Bus contract. Coverage includes
subsurface-only inhibition, multiple objects on one window, identity-only
updates, unload with live windows, changes while unloaded, populated initial
snapshots after reload, and abrupt client disconnection.

The harness loads the plugin only
after matching the private bus's KWin owner PID to its own nested compositor.
The test bus has no service activation directories and disables its own
AppArmor hook to avoid policy-query errors in read-only sandboxes. This does
not alter the desktop bus or system security configuration.
Never call `LoadPlugin` on the user's session bus, edit real `kwinrc`, install to
`/usr` or `~/.local`, or restart/signal the real KWin.
