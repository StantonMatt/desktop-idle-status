# Desktop Idle Status widget

Plasma 6.6+ system-tray applet for the session-bus service described in
`../docs/service-dbus.md`. The applet stays active in every state; the user can
choose its tray visibility with Plasma's normal tray settings. App rows offer
**Ignore** / **Stop Ignoring** for screensaver blockers; the service persists
these app-wide preferences. Ignored apps are excluded from new blocking history
and, with input-only idle tracking and exact attribution available, from blocker
status and tray counts. Sleep blockers are unaffected.

All production code is QML/JavaScript. The installed
`org.kde.plasma.workspace.dbus` module supplies async method calls, owner watching
and the `Changed` / `BlockedWhileAway` signal subscriptions. Typed D-Bus values
are normalized at the client boundary. Failed snapshot/history reads retry up to
three times (one, two, and four seconds); owner changes and explicit service starts
allow recovery after exhaustion. History read/clear failures use fixed inline text.
The legacy executable DataSource is used only for the fixed, timeout-bounded
`systemctl --user start desktop-idle-status.service` command
and a one-shot `locale -k LC_TIME` query. The latter honors POSIX's 24-hour Chilean
hour convention, which differs from Qt's es_CL CLDR short-time pattern. No native
QML plugin is needed.

The 22/32 px symbolic SVGs are extracted from the approved `design/mockup.html`
glyph function, including its masks, moon, slash and warning emblem. Breeze color
classes recolor them. Application icons come from the service / icon theme.

## Packaging

`CMakeLists.txt` installs the applet, the `.notifyrc`, its notification desktop
identity and the symbolic app icon together. Installing only the KPackage does
not install the companion notification event. The package is not installed or
enabled by the test harness. Configure with `cmake -S plasmoid -B <build-dir>`;
installation is a separate deployment step for the orchestrator.

The shell supplies the popup heading, back arrow, Keep Open pin and contextual
Clear History icon. The content representation intentionally has no duplicate
heading. Settings rows use `KCMUtils.KCMLauncher.openSystemSettings`; activating a
window relies on the shell's usual focus-loss collapse behavior (including its
Keep Open setting).

## Verification

From the repository root:

```
/usr/lib/qt6/bin/qmllint -I /usr/lib/x86_64-linux-gnu/qt6/qml plasmoid/contents/ui/*.qml
tests/plasmoid/unit.sh
tests/plasmoid/render-outputs.sh
tests/plasmoid/run.sh
DIS_STATES='blocked-one late screensaver-off service-down' DIS_THEMES=light DIS_EXERCISE=1 tests/plasmoid/run.sh
```

The test scripts create private buses with no activation directories, scratch
HOME and XDG directories, offscreen Qt and software rendering, and remove their
transient session directories. No real compositor, notification server, service
or user configuration is touched. The fake service implements the documented
properties, methods and signals. The capture probe is a **test-only** C++ shared
library loaded into `plasmoidviewer`; it captures the actual compact/full items,
composites them on their theme background, and clicks actual QML controls. It
exits the viewer after capture rather than running its broken offscreen teardown
(the viewer aborts during destruction on this host). The mock process and bus
are stopped on success and failure. There is no production C++ code.

Screenshots and fixture/viewer logs are retained under
`~/.cache/agent-scratch/desktop-idle-status/plasmoid-shots/run-XXXXXX/` as
`<state>-<light|dark>-<compact|full>.png`. Each invocation prints its unique
output directory, including when `DIS_SHOT_DIR` selects a different parent.
Logs, action traces, state files and theme overviews stay in that same directory.
`render-outputs.sh` checks simultaneous renders sharing one output parent.
`run.sh` accepts `DIS_STATES`, `DIS_THEMES`, `DIS_SHOT_DIR` and `DIS_EXERCISE` to select focused checks. The screenshots capture
the applet's representations, not the outer system-tray navigation header.
Default captures include subminute and fractional-minute deadlines, PowerDevil lock conflicts, unknown-timeout, diagnostic-code,
history-error, literal-markup, caption, and clock-advance fixtures. Raw diagnostic
strings appear only in viewer logs. Every run asserts
that the widget never starts the service automatically; only an exercised
service-down Start Service action may invoke the fake systemctl, exactly once.
Set `DIS_INSTANCES=2` with `DIS_STATES=ready DIS_EXERCISE=1` to check two
simultaneous applets: both claim the broadcast notice, but only one notifies.
The unidentified exercise also checks its inert row and themed notification
fallback icon. `run.sh` also refreshes the theme overviews through
`overview.py`, which requires Python Pillow (`python3-pil`).

## Contract limitations

The return signal adds an unsigned notice ID before start/end. Each instance calls
`ClaimReturnNotice(id)` and delivers only after a true result, coordinating the
once-only sender across applets. The return signal exposes `appName`, `iconName`,
`caption` and unsigned
`seconds` per window, ordered longest-first (ties use earliest interval start).
The notification uses the first row's icon and preserves this order in its
“By …” app names. An unidentified first row uses the widget icon.
`end-start` is the service's union duration, so gaps and overlaps are handled by
the service rather than guessed by the widget.

`BlockedUnattributed` is a boolean with no count or start timestamp. The
unidentified row therefore has no “Since” timestamp or numeric window count.

The service-down state uses Plasma's standard `PlaceholderMessage` icon size,
which is larger than the HTML illustration. Its disabled Start Service button
stays visible while starting via a placeholder child because this Plasma version
otherwise hides a disabled `helpfulAction`.
