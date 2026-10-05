# Desktop Idle Status service (slice 2)

Rust edition 2024, Tokio, zbus 5, wayland-client/protocols, rusqlite, and an
inotify-backed config watcher. The service only observes the desktop, except
for the explicit `ActivateWindow`, `StartScreensaver`, and `ClearHistory`
methods. It never installs/loads the KWin plugin,
changes settings, or acquires an idle inhibitor.

## Session-bus contract

- Name: `io.github.StantonMatt.DesktopIdleStatus`
- Path: `/io/github/StantonMatt/DesktopIdleStatus`
- Interface: `io.github.StantonMatt.DesktopIdleStatus1`

All properties are read-only. Each `GetAll` validates attribution once and
serializes all properties from one captured view. Validation results apply only
to the observation generation they queried; unload/recovery cannot let an older
failure invalidate a newer observation. Continuous observation churn returns a
coherent conservative unavailable view. Clients fetch an initial `GetAll`, subscribe to
`Changed()`, and fetch again. Standard `PropertiesChanged` is also emitted,
with an empty changed-properties dictionary and the property names in the
invalidated array. The custom `Changed()` signal (no args) also fires when
persisted history changes. An unchanged `Get`/`GetAll` validation still emits
`Changed` if it commits a pending history retry or deletes expired rows.
History has its own method; it is not a property. Clients should treat extra
dictionary keys as extensible.
The other custom signal is `BlockedWhileAway(u id, x start, x end, aa{sv} windows)`;
its exact payload and duration semantics are specified below.

| Property | Signature | Meaning |
| --- | --- | --- |
| `State` | `s` | `ready`, `blocked`, `running`, `screensaver-off`, or `unknown` |
| `ExactAttribution` | `b` | A compatible bridge snapshot is available |
| `UnavailableCode` | `s` | Stable machine-readable failure code (listed below); empty when available |
| `UnavailableReason` | `s` | Diagnostic text for logs/debugging only; may contain raw D-Bus errors. Never display it in the UI |
| `ScreensaverTimeout` | `u` | Seconds, matching the screensaver's clamped 1–240 minute setting; clients hide timeout text if absent or zero |
| `Blockers` | `aa{sv}` | Effective named windows only; oldest first |
| `BlockedUnattributed` | `b` | Fresh paired notifications demonstrate compositor inhibition without exact attribution; false while the screensaver is showing |
| `LockSleepBlockers` | `aa{sv}` | Separate PowerDevil/logind policies, ordered by app name |
| `TimeoutConflicts` | `aa{sv}` | Enabled active-profile/locking settings firing at or before the screensaver timeout |
| `RunningSince` | `x` | Observed start of the showing state, Unix seconds; zero otherwise |
| `ScreensaverOffReason` | `s` | Process not running; empty otherwise |

`UnavailableCode` values are `initializing`, `bridge-missing` (bridge metadata
cannot be read), `bridge-version-mismatch` (interface or KWin version mismatch),
`bridge-error` (snapshot/metadata/live-version failure, D-Bus deadline, or owner
resync), and `policyagent-unavailable` (the bridge is available but screensaver
activity cannot be observed). They accompany the same diagnostics in
`UnavailableReason`, including while a bridge failure coexists with `running`
or `screensaver-off`. Recovery clears both fields. Codes are extensible:
clients must map unknown/missing codes to safe fixed wording rather than display
the diagnostic. The plasmoid uses "Can't read idle inhibitors from KWin" for
bridge/unknown codes, "Checking screensaver status" while initializing, and
"Can't read screensaver activity from PowerDevil" for PolicyAgent failure.
`ScreensaverOffReason` is likewise diagnostic-only; policy `reason`, app names,
and window captions are intentional display content, not service errors.

`Blockers` dictionaries:

| Key | Variant type | Meaning |
| --- | --- | --- |
| `internalId` | `s` | KWin window UUID; pass unchanged to `ActivateWindow` |
| `appId` | `s` | Bridge app ID, falling back to desktop-file ID then resource class |
| `appName` | `s` | Desktop-file `Name`, falling back to class, app ID, executable basename |
| `iconName` | `s` | Desktop-file `Icon`, otherwise `application-x-executable` |
| `caption` | `s` | Caption with a matching trailing ` — AppName` or ` - AppName` removed |
| `since` | `x` | First observation of continuous effective inhibition, Unix seconds |

Order is `since`, app name, caption, then UUID. A caption change preserves
`since`. A window that becomes ineffective and later returns gets a new
`since`. The service cannot reconstruct inhibitor acquisition times before
it started. Desktop lookup honors `XDG_DATA_HOME` then `XDG_DATA_DIRS`, including
nested desktop-file IDs. Names and icons use the message locale (`LC_ALL`,
`LC_MESSAGES`, then `LANG`) with Desktop Entry locale fallback to base values.

All identity hints use one desktop index. Matching tries, in order: desktop ID
(case-insensitive, with or without `.desktop`), `StartupWMClass`, `Exec` binary
basename, localized/base `Name`, then localized/base `Icon`. Bridge hints are
checked in desktop-file, app-ID, resource-class, executable-path order within
each rule. Absolute paths also supply their basename as an additional candidate,
including for desktop IDs whose `Exec` uses a different launcher. PowerDevil and
logind `who` strings use the same rules; an unmatched `who` is preserved verbatim
with `application-x-executable`.

`Exec` lookup extracts applications from absolute/quoted paths, leading
environment assignments and the launcher table below. All supported launchers
share a table-driven option parser: exact long names, short flag clusters,
attached short operands (`-n5`), separate required operands, `--option=value`,
optional operands attached with `=`, and `--` where documented. Parsing stops at
the application boundary; application arguments never supply aliases or override
launcher options. `env` also accepts assignments among its options before that
boundary, as verified with the installed implementation. A short and its long
spelling share one canonical key; repeated options retain only their final value.
Only the final Flatpak `--command` supplies a command alias.

| Launcher | Option/positional grammar and exercised forms |
| --- | --- |
| `env` | Flags `-i`, `-v`; required `-u`, `-C`, `-f`, `-a`; optional `--ignore-signal[=SIG]`, `--default-signal[=SIG]`, `--block-signal[=SIG]`; `--list-signal-handling`; bare `-`; `NAME=VALUE`; `-iv`, `-uNAME`, repeated `-a`/`--argv0`, `--` |
| `nice` | Required `-n`/`--adjustment`; signed numeric values, `-n5`, `-n 5`, `--adjustment=5`, historical `-5`/`--5`/`-+5`, repeated adjustments, `--` |
| `ionice` | Required `-c`/`--class`, `-n`/`--classdata`; flag `-t`/`--ignore`; `-tc2`, `-n4`, repeated class, `--`; process/group/user selector forms rejected |
| `dbus-run-session` | Required `--dbus-daemon` and `--config-file`, separate or `=` operands, repeated daemon, `--` |
| `dbus-launch` | Documented syntax, stderr and session/X11 flags; `--config-file=FILE` only; repeated config; undocumented `--` and autolaunch rejected |
| `setsid` | Flags `-c`/`--ctty`, `-f`/`--fork`, `-w`/`--wait`; `-cfw`, repeated fork, `--` |
| `nohup` | Command with no launcher options, optional `--`; command's own `--help` remains an argument |
| `systemd-run` | Installed help's complete flag/required-operand table, including `-qGd`, `-uUNIT`, `-pNAME=VALUE`, `-ENAME=VALUE`, boolean `--expand-environment=BOOL`, timer/path/socket operands, repeated unit and `--`; `-S`/`--shell` rejected |
| `taskset` | Flags `-c`/`--cpu-list`, `-a`/`--all-tasks`, `-p`/`--pid`; always one mask/list positional before the command; `-c -- 0,1 app`, `-c -a 0,1 app`, `-ac 0-31:2,33 app`, `-- ff app`, repeated flags; all `-p` forms rejected |
| `flatpak run` | Installed help's complete flag/required-operand table; `-udpv`, `--arch=ARCH`, `--branch BRANCH`, repeated `--command=old --command=new`, `--`; ref parsed as `APP_ID[/ARCH[/BRANCH]]`, supplying only `APP_ID`, never the basename `BRANCH` |
| `snap run` | Documented `--debug-log`/`--trace-exec` flags, repeated flags, `--`, snap application identifier; shell/debugger forms and undocumented internal options rejected |

The authoritative option arities are `LAUNCHERS` in `service/src/identity.rs`.
They were checked on this system against `--help` and installed man pages:
uutils coreutils 0.10.0 (`env`, `nice`, `nohup`), util-linux 2.41.3 (`ionice`,
`setsid`, `taskset`), D-Bus 1.16.2, systemd 259.5, Flatpak 1.16.6, and Snap
2.77.1. Help probes also confirmed that Flatpak `--commit`, `--runtime-commit`
and `--instance-id-fd` consume an operand despite omitting `=VALUE` in help.
No launcher commands are executed during identity resolution.

Unknown/abbreviated options, missing/empty operands, values attached to flags,
malformed masks/lists/refs, field codes in executable positions and forms without
a command supply no aliases. Tests cover these failures for every launcher,
plus nested wrappers and arguments after the command boundary. Runtime-dependent
validity (for example whether a CPU or installation exists) is not evaluated.
Flatpak application IDs and final explicit `--command` binaries are aliases;
launcher names, overridden values, architecture/branch names and ordinary
arguments are not. Unsupported forms fall through to other identity rules or
the unchanged raw `who` string.

`optirun`, `mangohud`, `gamemoderun`, `primusrun`, `prime-run` and `steam-run`
were dropped because neither tools nor man pages are installed here to verify
their option semantics. Their names are explicitly rejected so they cannot
become wrapper aliases. All shell command/script prefixes (`sh`, `bash`, `dash`,
`zsh`, `ksh`, `fish`, `csh`, `tcsh`), including `sh -c STRING [NAME [ARGS...]]`,
remain unsupported: the command string and shell positional parameters never
become executable aliases. `env -S`/`--split-string` is likewise rejected;
its separate splitting/escape/environment-expansion language is not interpreted
as ordinary command tokenization. `env -0` cannot launch a command. Snap's
`--shell`, `--strace`, `--gdbserver` and undocumented internal `--command`,
`--hook`, `--revision`/`-r` forms are rejected rather than guessed.
`Hidden` and `NoDisplay` entries remain eligible, including for exact ID
matches. Equal-strength alias collisions prefer a listed entry, then XDG
search order and stable path order. A higher-priority desktop ID replaces the
lower-priority entry and all its aliases, even when hidden.

Identity call sites are `adapters::bridge` → `DesktopIndex::resolve` and
`Sources::apply` → `model::lock_blockers` → `DesktopIndex::policy` (both
PowerDevil and logind). `api::Api::validated_view` also refreshes through
`adapters::bridge`. History (`Data::observe_view` → `Tracker` → `Store`, then
`Api::history`) and `BlockedWhileAway` (`notification::return_event` →
`api::publish_return`) preserve those resolved bridge identities. Existing
persisted history is not rewritten; unattributed rows retain their explicit
“Unidentified window” identity.

`LockSleepBlockers` dictionaries have string keys `appName`, `iconName`,
`reason`, `what` (`idle` or `sleep`), `source` (`powerdevil` or `logind`), and
`mode` (`block` only). Empty reasons become
`Unknown reason.` A combined logind `idle:sleep` entry produces two rows.
PowerDevil only contributes active entries (flags bit 1); suppressed requests
are omitted. Matching `(who, why, what, mode)` logind rows already imported by
PowerDevil are omitted. Only mode `block` is retained from either source; logind `delay` inhibitors
are ignored, matching PowerDevil, and do not appear as blockers. These policies do **not** prove that this
screensaver's KIdleTime timeout is blocked.

`TimeoutConflicts` dictionaries have `setting s` (`dim`, `screen-off`, `lock`,
`suspend`), `seconds u`, and `kcm s`. KCM IDs are
`kcm_powerdevilprofilesconfig` and `kcm_screenlocker`. Rows are ordered earliest
first; ties prefer lock, screen-off, suspend, dim. Equal timeouts count as a
race. The widget can render its “Starts too late” presentation when
`State == "ready"` and conflicts are nonempty; this does not add a sixth state.

| Method | Input | Output | Behavior |
| --- | --- | --- | --- |
| `History` | `u days` | `aa{sv}` | Completed intervals overlapping the requested trailing days (clamped to seven), newest start first; zero days selects only intervals ending now |
| `ClearHistory` | none | none | Deletes persisted rows and pending pre-clear intervals/return notices; reports storage errors as D-Bus failures; ongoing blockage can begin a new post-clear interval |
| `ClaimReturnNotice` | `u id` | `b` | Atomically claims a published return notice; true exactly once per known ID, false for unknown, claimed, or cleared IDs |
| `ActivateWindow` | `s internalId` | `b` | Revalidates interface/build/live version immediately, addresses the validated unique KWin owner, and checks ownership before/after forwarding; false if unavailable, invalid, incompatible, owner changed, or the request fails/times out |
| `StartScreensaver` | none | `b` | Spawns `plasma-visual-screensaver --background` without waiting for daemon exit; true means spawn succeeded, not that its bus name or overlays appeared |

History dictionaries contain `start x`, `end x`, `appName s`, `iconName s`,
`caption s`, `appId s`, and `internalId s`. History rows are records, not
activation controls. An unattributed interval has `internalId="unattributed"`,
empty `appId` and caption, `appName="Unidentified window"`, and
`iconName="preferences-system-windows"`. Never send this ID to KWin.

## Observation and state decisions

The bridge on `org.kde.KWin`, `/DesktopIdleStatus`,
`io.github.StantonMatt.DesktopIdleStatus.KWinBridge1` must report
`InterfaceVersion=1` and `BuiltForKWin` equal to the live version parsed from
`org.kde.KWin.supportInformation()` on `/KWin`. Only effective snapshot rows
are blockers. The service subscribes before the initial snapshot, rereads on
`Changed`, and refreshes on owner changes. Positive cached attribution is also
revalidated on property reads, once per `GetAll` snapshot. A cheap snapshot/metadata/version check runs
every five seconds regardless of availability, detecting object unload even
without a bus-owner change or client reads and discovering a newly loaded empty
plugin without a `Changed` signal. A failed Snapshot, missing object, version
mismatch, or KWin owner change clears cached blockers, sets
`ExactAttribution=false` with a source reason, and emits `Changed` plus property
invalidations. Owner/bridge signals trigger immediate resync; silent unloads
are detected within five seconds plus the three-second D-Bus deadline. When a
compatible bridge reappears, its snapshot restores exact attribution. Bridge
absence or a version mismatch
always sets `ExactAttribution=false` and can never yield `ready`.

Without a compatible bridge, a fresh pair of 10-second normal/input-only
ext-idle-notify notifications is recreated every 12 seconds. Input-only idle
without normal idle (after a 250 ms delivery grace) is evidence of inhibition:
`blocked`, empty `Blockers`, and `BlockedUnattributed=true`. Otherwise the
answer is `unknown`, even when normal idle fires. An independent persistent
input detector clears stale inference on resumed input. The service never
attributes a compositor block from PowerDevil, MPRIS, or application names.
Inference is delayed and is not an instantaneous inhibition-change API.

Away tracking uses an independent `get_input_idle_notification` at the
screensaver timeout, unaffected by KWin inhibitors. ext-idle-notify v2 and a
seat are required. With unavailable Wayland, no new away intervals are
recorded; exact bridge snapshots and policy rows can still be served.
Disconnection closes and persists qualifying active intervals and discards all
pending return data, so unobserved input cannot merge separate away periods.
Disconnection is retried on subsequent timeout/bridge-availability/KWin-owner
lifecycle changes, not a polling loop. Existing user inactivity before service
startup cannot be reconstructed from this protocol.

Showing is inferred from a screensaver-owned **requested** PolicyAgent tuple
with reason `Visual screensaver is active`, using the known desktop ID,
executable identity, or the portal's empty app ID. The screensaver's bus name
`org.kde.PlasmaVisualScreensaver` must also have an owner. Its control object is
`/PlasmaVisualScreensaver`, interface `org.kde.PlasmaVisualScreensaver`.
The inspected screensaver registers only `ExportScriptableSlots`; its C++
`screensaverActive` property/signal is **not exported**. KDE's
`org.freedesktop.ScreenSaver.GetActive` describes the locker, not this overlay.
Consequently showing/`RunningSince` have the approximately five-second
PolicyAgent publication delay and are observations, not exact activation
timestamps. Caller-supplied policy identities are not authentication.

State precedence: showing → missing process → exact/inferred block
→ exact ready → unknown. If the screensaver owns its bus name but PolicyAgent
cannot be read, state is `unknown` rather than claiming ready without a
showing-state observation. Screensaver-owned windows and policy entries are
excluded everywhere. A missing bridge reason remains available even in
`running` or `screensaver-off`.

Configuration uses the KConfig cascade for `plasma-visual-screensaverrc`,
`powerdevilrc`, and `kscreenlockerrc`. All three use `FullConfig`: the
global files supply the fallback (`/etc/kde5rc`, then `system.kdeglobals`,
then `kdeglobals`), then the application-file cascade
is merged, honoring immutability and deletion. PowerDevil's generated profile
settings and KScreenLocker's generated skeleton use the same global fallback
as the screensaver (KConfig 6.24.0, PowerDevil 6.6.6, KScreenLocker 6.6.5).
`KDE_SKIP_KDERC` disables the legacy `/etc/kde5rc` layer. Named global and
application files cascade through `$XDG_CONFIG_DIRS` (default `/etc/xdg`,
first listed directory has higher priority), then `$XDG_CONFIG_HOME` (default
`~/.config`). User values override inherited system values except immutable
system keys (`key[$i]`) and groups (`[Group][$i]`). Group immutability locks
that exact group; nested groups are independent. A standalone `[$i]` marks
the whole file immutable. Key option tokens are processed from right to left;
combined flags within a token are processed from left to right. Parsing stops
at `d`, as in KConfig. Deletion entries such as `key[$d]`
remove inherited values even without `=`; ordinary entries without `=` are
ignored. Group `[$d]`/`[$e]` tokens are literal subgroup names, not deletion or
expansion directives. Literal slashes in group names do not create subgroups.
ASCII whitespace and KConfig escapes are decoded in keys, groups and values.
`[$e]` expands environment variables for string reads, but numeric and boolean
reads remain raw, matching KConfigGroup's typed reads. Booleans use KConfig's
case-insensitive false/no/off/0 convention. Missing files remove their layer;
permission, encoding and other read failures retain the last valid layer, log
the failure, and retry on the one-second maintenance tick even without a file
change event. Before a first successful read the layer is empty. Directory
watching covers every source, including the global files,
in-place edits and atomic replacements, and ignores read/open events.
`IdleMinutes` defaults to 10, uses KConfig's signed integer QVariant conversion,
and is clamped to 1–240 before multiplying by 60. Fractional/exponent/malformed
values fall back to the default. Qt first parses a signed 64-bit decimal integer
then narrows to 32 bits (including wraparound); overflow of the 64-bit parse
uses the default. PowerDevil's dim/off/suspend seconds use the same signed
conversion; its action enum uses unsigned 64-bit parsing then 32-bit narrowing.
The locker's `Timeout` remains a double, allowing fractional minutes. The current screensaver has no native enabled key: bus-name
absence is its real off condition. The service does not invent an `Enabled` key
that the target would ignore. No setting or autostart file is written. Missing source directories are watched through their closest existing ancestor;
creation and replacement re-arm the source watches without writing config files.

PowerDevil's active profile is read from session-bus
`org.kde.Solid.PowerManagement`, `/org/kde/Solid/PowerManagement`, interface
`org.kde.Solid.PowerManagement`, method `currentProfile() -> s` (verified by
live introspection). `profileChanged(s)` and `configurationReloaded()` trigger
rereads; owner changes and config edits also recompute conflicts. Profile IDs
are exactly `AC`, `Battery`, and `LowBattery`. Unknown/unavailable profiles
omit power-action conflicts rather than assume AC; global screen locking is
still evaluated. Failed profile reads are logged.

PowerDevil 6.6.6 non-mobile defaults, verified against KDE's
`daemon/powerdevilsettingsdefaults.cpp` at tag `v6.6.6`:

| Profile | Dim | Screen off | Suspend |
| --- | --- | --- | --- |
| AC | 300 s | 600 s | 900 s |
| Battery | 120 s | 300 s | 600 s |
| LowBattery | 60 s | 120 s | 300 s |

Missing keys use the defaults for the active profile only. Dim/off default
enabled; suspend defaults on only when suspension
is supported and the machine is not virtualized. Capabilities come from logind
`CanSuspend` and systemd's `Virtualization`; failed capability reads assume a
suspend-capable non-VM until recovery. Explicit disabled actions and negative timeouts are
excluded. `LockBeforeTurnOffDisplay` is included and points to
`kcm_powerdevilprofilesconfig`. The earliest enabled locking source controls
both deadline and KCM; automatic locking points to `kcm_screenlocker` and wins
an equal deadline against screen-off locking. Screen-lock defaults are
Autolock=true, Timeout=5 minutes (fractional minutes supported); disabled/zero
locking is excluded.

PowerDevil mappings were also checked against [`PowerDevilProfileSettings.kcfg`](https://invent.kde.org/plasma/powerdevil/-/blob/v6.6.6/PowerDevilProfileSettings.kcfg),
[`daemon/powerdevilenums.h`](https://invent.kde.org/plasma/powerdevil/-/blob/v6.6.6/daemon/powerdevilenums.h),
`daemon/actions/bundled/{dimdisplay,dpms,suspendsession}.cpp`, and
`kcm/{PowerKCM.cpp,ui/ProfileConfig.qml}` at `v6.6.6`. The read keys are:

| Group | Keys | Missing-key defaults (non-mobile) |
| --- | --- | --- |
| `<profile>/Display` | `DimDisplayWhenIdle`, `DimDisplayIdleTimeoutSec` | true, profile dim time above |
| `<profile>/Display` | `TurnOffDisplayWhenIdle`, `TurnOffDisplayIdleTimeoutSec` | true, profile screen-off time above |
| `<profile>/Display` | `LockBeforeTurnOffDisplay` | false |
| `<profile>/SuspendAndShutdown` | `AutoSuspendAction`, `AutoSuspendIdleTimeoutSec` | Sleep (1) when suspend-capable and non-VM, otherwise NoAction (0); profile suspend time above |

`AutoSuspendAction` is an unsigned discrete enum, not a bitmask:

| Value | PowerDevil action | Auto-idle conflict |
| --- | --- | --- |
| 0 | No action | none |
| 1 | Sleep | suspend |
| 2 | Hibernate | suspend |
| 4 | Retired hybrid suspend | none |
| 8 | Shutdown | suspend |
| 16 | Prompt logout dialog | no forced end of the session |
| 32 | Lock screen | lock (PowerDevil KCM) |
| 64 | Turn off screen | none: not implemented by SuspendSession's auto-idle handler |
| 128 | Toggle screen on/off | none: not implemented by SuspendSession's auto-idle handler |

The KCM's auto-idle model offers 0, 1, 2, and 8. Lock (32), if manually configured,
is implemented by the daemon and uses the earliest of all lock deadlines. Zero
`AutoSuspendIdleTimeoutSec` registers no idle timeout. SleepMode 1 (RAM), 2
(hybrid), and 3 (suspend then hibernate) all use AutoSuspendAction=1; SleepMode
is not read because it does not change the conflict. PowerButtonAction,
PowerDownAction, LidAction, and display timeout when already locked are not read;
they do not supply the unlocked auto-idle deadline modeled here.

PowerDevil reads both `RequestedInhibitions` and `ActiveInhibitions` with
signature `a(ssssu)`, refetching on property invalidation. logind
`ListInhibitors` on the system bus is refetched on login1 property/owner changes.
Desktop D-Bus reads/actions have three-second deadlines. Unavailable policy
sources are logged and their stale rows cleared, rather than silently retaining
a previously observed blocker. Failed PolicyAgent, active-profile, screensaver-owner,
logind, and capability reads retry independently without requiring a signal,
with exponential delays of 1, 2, 4, 8, 16, then at most 30 seconds (plus
the main loop scheduling and three-second query deadline). Initial/closed
system-bus connections and failed system signal subscriptions also retry.
Success resets the source backoff; relevant signals trigger an immediate read.

All environment-derived XDG paths use one resolver. Empty or relative
`XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, and `XDG_STATE_HOME`
are ignored, falling back to their standard directories under an absolute
`HOME`. Without a usable HOME or explicit config/data directory, startup
fails instead of reading or writing relative to the working directory.
Unset or empty `XDG_CONFIG_DIRS` and `XDG_DATA_DIRS` use their standard defaults;
relative and empty entries in a supplied list are ignored, preserving valid
entry order. Desktop-file scanning and KConfig path expansion use the same
rules. `XDG_RUNTIME_DIR` must be absolute and nonempty when needed for a
session-bus fallback or a relative Wayland display; it has no HOME fallback.
Explicit D-Bus addresses, absolute Wayland displays, and inherited Wayland
socket descriptors continue to work without a runtime directory.

## History and notifications

Each effective window gets its own interval while input-only idle is at least
the configured screensaver timeout and state is blocked. Start is the later
of observed away start and observed inhibition start; title/identity are
captured then. Each completed bridge, policy, profile/configuration,
screensaver-presence, logind, and capability observation is projected into the
shared view and tracker at its own timestamp before awaiting another source.
Requested policy rows are applied before fetching active lock/sleep rows.
Owner loss/change and API bridge validation also apply immediately. Timeout
changes close/reset the old interval and send new Wayland settings before
unrelated reads. Wayland inference/availability changes use the same projection.
Wayland events carry their observed Unix timestamp. The receiver remains active
during every bounded session/system source refresh, so a queued input resume
closes intervals at its event timestamp without extending a duration or pushing
a short interval over the notification threshold. An interval ends on resumed input, loss of that effective
blocker, loss of usable observations, showing/off state, a timeout reset, or
service termination. Intervals under 60 seconds are discarded. Releasing all
blockers closes entries even if the user remains away. Subsequent blockers in
that away period get new intervals. An inferred unknown block is one
unidentified interval. Titles changing during an interval do not rewrite it.

SQLite: `$XDG_DATA_HOME/desktop-idle-status/history.sqlite3`, default
`~/.local/share/desktop-idle-status/history.sqlite3`. Directory mode is 0700,
database mode 0600. Entries are trimmed at startup, on writes, and on a one-second maintenance
tick, and queries also apply seven-day retention. Every insertion or retention
deletion emits `Changed`, including while the visible state is unchanged.
`secure_delete=ON` wipes deleted cells on retention and clear. A one-time
privacy migration runs `VACUUM` to wipe pages freed by earlier versions;
`ClearHistory` also attempts to vacuum the database. Committed deletion is
successful even if compaction fails: the failure is logged, tracker/pending
writes/notices are cleared, and `Changed` is published. A failed deletion
reports a D-Bus error and retains tracking state. Intervals whose end is within the retention
window are retained, including ones that began earlier. SIGTERM/SIGINT close
active intervals. Completed entries remain pending until their insertion and
retention transaction commits. Transient write failures are logged and retried
on later observations; reset/disconnection never discard pending writes.
Shutdown closes active intervals once, then retries pending writes three times
at 100 ms intervals, preserving the original end timestamps.
Successful shutdown writes publish `Changed` before the service releases its
session-bus connection, using the same view-or-history publication decision as
observations. Final publication is bounded and best effort if the bus is lost.
Persistent storage failure still exits with an error and cannot survive process
termination.
Shutdown signals are registered before startup I/O and cancel
all asynchronous observation work, including chained source deadlines, bridge
validation, signal publication, and connection/subscription retries. Flushing
uses the tracker/store lock before the Wayland task is aborted and joined; it
also runs on observation errors, session-bus stream closure, and smoke-mode
completion. SIGKILL/crashes cannot flush an open in-memory interval.

On resumed input, the service emits **one** custom signal on its service
interface, `BlockedWhileAway(u id, x start, x end, aa{sv} windows)`, if the union of
qualifying blocked intervals in that away period is at least 600 seconds.
Overlapping windows count once; gaps do not count. `end` is the last qualifying
interval's Unix end timestamp, and `start = end - unionSeconds`: **`end-start`
is the blocked duration**, not necessarily the wall-clock span when there are
gaps. Actual per-window timestamps remain in `History`. This interpretation
hands the widget an accurate duration; the leading ID arbitrates delivery
across widget instances. Nothing is emitted for shorter blocks, shutdown, config/idle reset,
or `ClearHistory`. The pending return notice is consumed on resumed input,
so repeated resumes cannot duplicate it.

Notice IDs are nonzero, increase without reuse during the service lifetime,
and are reserved while consuming tracker data under the history lock, before
any awaited signal publication. IDs are in-memory and scoped to
the service bus owner; clients must discard queued IDs when that owner changes.
Every widget receiving the signal must call `ClaimReturnNotice(id)` and only
the client receiving true may deliver a notification. Concurrent claims are
serialized atomically; claimed/unknown IDs return false. `ClearHistory` cancels
unclaimed notices, including ones reserved for a delayed publication, without
reusing their IDs. A cleared notice cannot become claimable after publication;
publication also skips an ID already cleared before it begins. Tracker updates,
SQLite writes, retention trimming and clear are serialized under the same lock;
pre-clear entries are never held across an await for a later write. If the u32 space is exhausted,
no new notice is emitted until service restart.

Each `windows` dictionary contains exactly `appName s`, `iconName s`,
`caption s`, and `seconds u`, with the same identity/title conventions as
History. `seconds` is the union of that window's qualifying intervals during
this away period: repeated intervals add their blocked time, overlaps count
once, and gaps do not count. It saturates at the maximum unsigned 32-bit value.
The identity/title comes from that window's earliest qualifying interval.
Rows are ordered longest-first by `seconds`; ties use earliest interval start,
then app name, caption, and window ID. Identical display triples are deduplicated
by retaining the longest window (using the same tie breaks), rather than adding
the durations of distinct windows. The first row therefore supplies the longest
blocker's icon and leads the notification's app names. An unidentified row is
`appName="Unidentified window"`, `iconName="preferences-system-windows"`,
`caption=""`. These rows are notification presentation data, not activation
controls.

The plasmoid owns notification delivery through QML `Notification` and its
`.notifyrc` event, so the default action can open its popup on history. The
service never calls `org.freedesktop.Notifications`, and the removed
`--no-notifications` flag is rejected as an unknown argument. The five state
strings and existing methods/properties remain unchanged.

## Build, tests, and service unit

From `service/`: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D
warnings`, `cargo fmt --check`. Runtime modules:
`model` (state/policies/probes), `config`, `identity`, `history`, `notification`,
`adapters` (desktop reads), `wayland`, `api`, and `main` (event loop).
The lockfile is included.

`cargo test` automatically reexecutes the D-Bus tests under
`dbus-run-session`, with temporary HOME and config/data/cache/state/runtime
directories under
`~/.cache/agent-scratch/desktop-idle-status/service-tests/`. Its minimal private
bus sets `KDE_SKIP_KDERC` to exclude the real legacy global config, has no
service activation directories and disables only its own AppArmor
hook, necessary in this read-only sandbox. The child's system-bus address is
also that private bus; Wayland is intentionally disconnected in the contract
test. Fake services
cover bridge snapshots/version rejection, invalidation-based policy updates,
policy dedupe, history/clear/signals, activation, showing/off, config changes,
the start command, active-profile changes/defaults, logind delay exclusion,
silent bridge unload/reappearance, silent incompatible activation rejection,
system connection and source recovery without signals, inherited config watches,
retention publication during unchanged Ready, and the four-argument return
signal with concurrent one-time claims and deterministic clear between notice
reservation and publication. A second private-bus test uses a minimal private
Wayland protocol fixture to create a real active interval, stalls successive
PowerDevil reads, and verifies that SIGTERM exits and persists that interval
within four seconds (below `TimeoutStopSec=5`). It also checks SIGINT and the
15-second smoke deadline during stalled reads. The controlled private fixture
also covers normal/input delivery and the grace period, periodic/settings pair
recreation, timeout reset, exact/fallback mode changes, persistent activity
clearing, and disconnect/recovery. A controlled private-bus API test checks one
validation per GetAll, silent object unload and a failed validation spanning an
unload/recovery generation change. Injected SQLite write failures cover update,
reset and shutdown flush recovery; a live SQLite statement forces VACUUM
failure after successful deletion. A stalled-refresh test verifies that a
599-second interval closes at the observed resume without creating a notice.
Timestamp-based regression cases cover source disappearance, attribution loss,
owner changes, policy/presence changes, profile/config/capability timeout resets,
logind rows, Wayland events, and API validation while unrelated reads remain
stalled: 59 seconds stays out of history and 599 seconds never creates a notice.
Path tests cover absent/empty/relative HOME and XDG values, list entries,
non-UTF-8 paths, KConfig aliases, and desktop-file scanning. Pure
return-decision tests cover the threshold, overlaps, gaps, and unidentified
rows. No freedesktop notification delivery adapter exists. The start
executable is a scratch shell fake. No tests call the desktop's buses or show
notifications. The shutdown test deliberately waits 63 seconds for a real
qualifying interval rather than changing production clocks or adding test hooks.
Parser tests cover bare deletions in all three config files, combined options,
exact/file group immutability, missing `=`, whitespace/escapes, and typed expansion;
those semantics were checked against installed KConfig 6.24.0 and its upstream
`kconfigini.cpp`/`kconfiggroup.cpp` parsers.

The user unit `service/desktop-idle-status.service` uses the packaged
`/usr/bin/desktop-idle-status`. It is `Type=dbus`, bound to the graphical
session and restarts on failure. `KillMode=process` keeps an explicitly launched
screensaver alive when the observer unit stops. This task does not install or
enable it during development. Debian installs it under `/usr/lib/systemd/user`
and enables it for graphical sessions through `dh_installsystemduser`. Its
D-Bus activation file names the same `BusName` and `SystemdService`. The widget's
explicit Start Service action continues to use `systemctl --user start`.
`--smoke` is a 15-second bounded observation mode: no service-name acquisition,
no methods exported, no notifications, then a printed snapshot and exit. Always
set scratch `XDG_DATA_HOME` when using it against the real desktop.

Checks (from `service/`): `cargo build`, `heavy cargo test --all-targets`,
`heavy cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
Filesystem-backed tests create temporary directories under
`$DESKTOP_IDLE_STATUS_TEST_ROOT`, defaulting to
`~/.cache/agent-scratch/desktop-idle-status/service-tests/`.

The daemon requests its service name with DoNotQueue and without replacement
flags. A second instance exits with an ownership error. A NameLost subscription
is installed before the request; name loss or bus disconnection cancels in-flight
observation and uses the same interval flush path as termination. An exclusive,
nonblocking advisory lock on `desktop-idle-status/history.lock` in XDG_DATA_HOME
also rejects competing daemon or smoke processes, including those on different
buses. The lock remains held through the final flush and is released by process
exit; the file is never unlinked.
