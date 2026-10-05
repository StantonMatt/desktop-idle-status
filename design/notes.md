# Desktop Idle Status: interaction rules

Companion to `mockup.html`. It covers the tray icon, the tooltip, the popup and the return notification. This is a desktop tray widget, so phone width doesn't apply. The mockup page itself does work at narrow widths.

Decided with the user: the name stays "Desktop Idle Status"; the tray icon is always visible; history keeps 7 days, ignores blocks under 1 minute and stores the window title from when the block started.

## Tray icon

| State | Icon (22 px symbolic) | Tooltip main text | Tooltip subtext |
|---|---|---|---|
| Ready | Monitor with a moon | Screensaver will start | After 10 minutes of inactivity |
| Blocked | Same icon, with a red slash | Screensaver won't start | Blocked by Firefox |
| Blocked, window unidentified | Same as Blocked | Screensaver won't start | Blocked by an unidentified window |
| Screensaver not running | Monitor with a red cross where the moon was | Screensaver won't start | Plasma Visual Screensaver isn't running |
| Starts too late | Ready icon with an orange warning emblem | Screensaver starts too late | Screen locks after 5 minutes |
| Screensaver running | Filled screen with the moon cut out | Screensaver is running | Since 21:40 |
| Unknown | Monitor with a question mark | Screensaver status unknown | The reason (see below) |

- The icons are our own SVGs in the Breeze line style. They use the `ColorScheme-Text`, `ColorScheme-NegativeText` and `ColorScheme-NeutralText` classes so they follow light and dark themes. Ship each at 22 px for the tray and 32 px for the popup.
  - The moon matches the screensaver's app icon.
  - The red slash is the one Breeze's `system-suspend-inhibited` uses.
  - The warning emblem is Breeze's 8 px `emblem-warning`.
  - Red means the screensaver won't start; orange means it will start, but not in time to be seen.
- **Left click** opens and closes the popup. **Middle click** does nothing. **Right click** shows the standard applet menu.
- `Plasmoid.status` is `ActiveStatus` in every state, so the icon is always shown in the tray. There is no dot or badge.
- **Blocked tooltip:** list distinct app names in the order the rows appear. Two apps read "A and B", three read "A, B and C", and more read "A, B and 3 others". Several windows from the same app count as one name.
- **Starts too late tooltip:** names the earliest conflicting setting. If two settings share the same time, the more serious one wins, in this order: lock, turn off, sleep, dim.
- Only the timeout number changes in the Ready text. Use plural forms, so 1 reads "After 1 minute of inactivity".

## Popup layout (top to bottom)

1. **Heading.** The system tray provides the back arrow, the title (the Plasmoid name) and the Keep Open pin.
   - **Clear History** (`edit-clear-history`) sits here as an icon button with a tooltip. It is the applet's only contextual action, so the tray shows it as a single icon button, the same way it shows Notifications' "Clear All Notifications".
   - There is no configure button, because there are no settings.
2. **Status.** A 32 px state icon, the same main text as the tooltip, and a secondary line when there is one:
   - Ready and Starts too late show the timeout.
   - Screensaver not running shows "Plasma Visual Screensaver isn't running" and a **Start** button.
   - Running shows "Since 21:40".
   - Unknown shows the reason.
   - Blocked has no secondary line.
3. **Rows under the status,** indented to line up with the status text, so they read as the reason for it:
   - Blocked: the blocking windows, or the unidentified-window row.
   - Starts too late: the conflicting settings.
4. **Blocked While Away.** The history section. It is always present, except when the service is down.
5. **Sleep and Screen Locking.** Lock and sleep inhibitions from PowerDevil and systemd. The section is shown only when there are entries.

The popup uses the tray's default size (24 × 24 grid units) and scrolls when content overflows. Nothing is capped.

## Controls

- **Window row** (`ItemDelegate`): hover highlight. Click, Enter or Space activates the window through KWin, which switches desktop or activity and unminimizes it if needed. The popup then closes, as tray popups do when focus leaves, unless it is pinned. If the window has already closed, the row disappears on the next update and no error is shown.
- **Unidentified window row:** same layout as a window row, with the `preferences-system-windows` icon. Its title is "Unidentified window" (or "2 unidentified windows" if the count is known), the second line is "KWin plugin isn't loaded", and it shows "since" when known. It is not interactive: no hover, no click.
- **Settings row** (Starts too late): same delegate as a window row.
  - Content: the KCM's icon, what happens ("Screen locks", "Screen dims", "Screen turns off", "Computer sleeps"), the System Settings page name, and "After 5 min".
  - Click, Enter or Space opens that page with `KCMUtils.KCMLauncher.openSystemSettings`: `kcm_powerdevilprofilesconfig` for Power Management, `kcm_screenlocker` for Screen Locking. The popup then closes.
- **Start** (Screensaver not running): a `PlasmaComponents3.Button` that runs `plasma-visual-screensaver --background` detached.
  - The button stays disabled until the screensaver's D-Bus name appears, at which point the state updates.
  - If the name hasn't appeared after 10 s, the second line reads "Couldn't start Plasma Visual Screensaver" and the button is enabled again.
- **Start Service** (service down): the `PlaceholderMessage` `helpfulAction`. It starts the user service (`systemctl --user start …`).
  - Same disabled-while-starting rule, also up to 10 s.
  - On failure the explanation reads "Couldn't start the Desktop Idle Status service".
  - Because the placeholder is actionable, its icon and text show at full opacity, as Kirigami does.
- **Clear History:**
  - Deletes all history entries at once, including the stored window titles.
  - There is no confirmation and no undo, the same as Notifications' "Clear All Notifications".
  - The action is hidden when history is empty, and also in the Loading and service-down states.
  - The popup stays open, and the section then reads "None in the last 7 days".
- **History rows:** not interactive (no hover, no click). They are a record.
- **Sleep and Screen Locking lines:** information only, with no buttons. They use the exact sentences from the Power and Battery applet ("Elisa is blocking sleep. (Playing music)"). Unblocking stays in that applet.
- **Truncated text:** app names and titles elide on the right. Hovering a truncated row shows a tooltip with the full app name and title.
- **Keyboard:** Tab and the arrow keys move between rows and buttons; Enter activates.

## Live updates while the popup is open

- Status text, icon and rows update in place.
- **Window rows are ordered oldest first** (by "since"). A new blocker is added at the bottom, so the row under the pointer never moves. A removed row collapses in place, using the standard ListView remove transition.
- New history entries appear at the top of their section.

## Ordering

- **Blocking windows:** by "since", oldest first. Ties go by app name, then window title.
- **Settings rows:** by time, earliest first. Ties follow the tooltip order: lock, turn off, sleep, dim.
- **History:** by start time, newest first.
- **Sleep and Screen Locking:** by app name, as in the Power applet.

## Text rules

- **App name:** the desktop file's Name. If there is none, use the window class or app id (for example `steam_app_367520`).
- **App icon:** the desktop file's icon. If there is none, use `application-x-executable`.
- **Window title:** remove a trailing " — App" or " - App" when it matches the app name, as the task manager does. If the title is empty, omit the line and show a single-line row.
- **Unidentified blocks in history:** recorded as "Unidentified window", with no second line.
- **Blocker reasons:** shown as the app reports them. A missing reason reads "Unknown reason.", as in the Power applet.

## Time formatting

All times use the locale's short time format (`LC_TIME`). On this machine that is `es_CL`, which gives 24-hour times. Weekday and date names also follow `LC_TIME`, so "Thu" in the mockup would show as "jue." here. "Today", "Yesterday" and the other wording are UI strings that follow the UI language.

- **Since (window rows):**
  - Today: "Since 21:14"
  - Yesterday: "Since yesterday 23:58"
  - Within the last 6 days: "Since Thu 22:15"
  - Older: "Since 27 Sep"
- **History range:** "01:10–06:40", with an en dash and no spaces.
  - A range that crosses midnight is still written "23:40–00:55". It is grouped under the day it started, and the duration makes it unambiguous.
  - The line below the range reads "Today · 5 h 30 min". The day is "Today", "Yesterday", a short weekday within 7 days, or else the short date.
- **Duration:** "45 min", "1 h 15 min", "5 h" (no "0 min"), rounded to the nearest minute. Use non-breaking spaces inside a duration.
- **Settings rows:** "After 5 min". The tooltip uses the full word: "Screen locks after 5 minutes".
- **Running:** "Since 21:40", following the same day rules as window rows.

## Starts too late (timeout conflict)

- The screensaver timeout is compared with Plasma's settings for the active power profile (AC, battery or low battery). It is re-checked when the profile or either config changes.
  - **Power Management:** dim, turn off screen, and sleep when inactive.
  - **Screen Locking:** lock automatically after a set time.
- A setting conflicts when it is enabled and its time is **at or before** the screensaver's. At equal times it is a race, so it counts.
- Dimming is included: the screensaver would appear on a dimmed screen.
- **Precedence:** this state shows only when the screensaver would otherwise be Ready. If something is also blocking, Blocked is shown, and Starts too late appears once the block ends. That keeps one reason at a time.

## Return notification

- **When it is sent:** once per away period, when input resumes, if the screensaver was blocked for **10 minutes or more** in that period. Overlapping blockers are counted once, so this is the union of the blocked time, not the sum. Shorter blocks are still recorded in history; they just don't notify.
- **Content:**
  - Summary: "Screensaver was blocked for 5 h 30 min".
  - One window: "By Firefox (Lo-fi Radio – YouTube)".
  - Two apps: "By Firefox and Haruna".
  - Three or more: "By Firefox, Haruna and 2 others".
  - Unidentified: "By an unidentified window".
  - The large icon is the app that blocked longest, or the widget icon if it is unidentified.
- **Header:** the widget's icon and name, as for any notifying app.
- **Click** (the default action) opens this widget's popup, where the entry is at the top of Blocked While Away. There are no action buttons.
- **Behavior:** normal urgency and Plasma's standard timeout. A missed notification stays in Plasma's notification history.
- **Turning it off:** the header's Configure button leads to System Settings, Notifications. This needs a `.notifyrc` with one event ("Screensaver blocked while away").
- **Who sends it:** the plasmoid, with the QML `Notification` type (as the Power applet does), so the default action can set `expanded = true`. It reacts to an "away ended" signal from the service that carries the blocked duration and the blockers.

## When history entries are recorded

- **Away** means no user input for at least the screensaver timeout. It is measured from input only, ignoring inhibitors (for example through ext-idle-notify's input-idle notification). An away period starts at the last input plus the timeout, which is when the screensaver would have started. It ends at the next input.
- During an away period, every window holding a Wayland idle inhibitor gets one entry.
  - Start: the later of the away start and when the inhibitor was taken.
  - End: the earlier of the next input and when the inhibitor was released.
  - When the last blocker releases while the user is still away, the screensaver starts normally. The entries end at that moment.
- If the KWin plugin isn't loaded, a detected block is recorded as one "Unidentified window" entry.
- The screensaver's own inhibitor is never recorded or listed.
- Entries shorter than 1 minute are discarded as noise.
- Brief input (a nudged mouse) ends the away period. If the user leaves again, new entries begin.
- **Stored per entry:** app id, app name, icon name, window title, start and end. The title is taken when the entry starts.
- **When entries are written:** an entry is written when it closes. If the session ends while the user is away (logout or shutdown), the entry is closed at that time.
- **Retention:** 7 days. Old entries are trimmed when the service starts and whenever an entry is written. The empty text "None in the last 7 days" follows this value. Clear History removes everything at once.
- Window titles can be private, and they are stored on disk under the user's data directory.

## States and transitions

- **Precedence:**
  1. Unknown (service down)
  2. Screensaver not running
  3. Screensaver running
  4. Unknown (KWin integration unavailable)
  5. Blocked (named or unidentified)
  6. Starts too late
  7. Ready
- **Loading:** from plasmashell start until the service's first reply.
  - The popup shows a busy indicator.
  - The tray shows the Unknown icon with no tooltip subtext.
  - After 5 s with no reply, or when the D-Bus name has no owner, the state becomes Unknown (service down).
- **Unknown, service down:** the popup shows a `PlaceholderMessage` reading "Screensaver status unknown" and "The Desktop Idle Status service isn't running", with **Start Service**. This is the same pattern Notifications uses for "Notification service not available".
- **Screensaver not running:** the screensaver's D-Bus name has no owner. The answer is known, and it is "no", which is why this is not shown as Unknown. Blocking windows are hidden because they don't matter until the screensaver runs. History and Sleep and Screen Locking are shown.
- **Blocked, window unidentified:** the KWin plugin isn't loaded (for example after a KWin update, before it is rebuilt). Blocking is still detected but the window can't be named.
- **Unknown, KWin integration unavailable:** only when blocking can't be detected at all. The status block reads "Can't read idle inhibitors from KWin". History and Sleep and Screen Locking are still shown.
- **Screensaver running:** mostly seen during a preview, because the overlay covers the panel.

## Plasma building blocks

| Part | Component |
|---|---|
| Popup root | `PlasmaExtras.Representation` with `collapseMarginsHint: true` |
| Clear History | A `PlasmaCore.Action` in `Plasmoid.contextualActions`, `visible` only when history exists |
| Status block | Power and Battery section pattern: 32 px icon, `gridUnit` gap, `Kirigami.Heading` level 3, a secondary line at opacity 0.75, and an optional `PlasmaComponents3.Button` |
| Window, settings and unidentified rows | `PlasmaComponents3.ItemDelegate`: 32 px icon, name in body font, second line in `smallFont` at 0.75, right-hand time in `smallFont` at 0.75. The unidentified row has hover turned off. |
| Section titles | `PlasmaExtras.ListSectionHeader` |
| History rows | 22 px icon, same text styles, no highlight |
| Lock and sleep lines | Same as the Power applet's `InhibitionHint`: 16 px icon, `smallFont`, wraps up to 4 lines |
| Service-down placeholder | `PlasmaExtras.PlaceholderMessage` with `helpfulAction` |
| Loading | `PlasmaComponents3.BusyIndicator` |
| Return notification | `org.kde.notification` `Notification` with a `.notifyrc` event and a default action |

## Still open

- **Clickable history rows:** letting a history row activate its window when that window is still open. Not in v1.
- **Unknown (KWin integration unavailable):** check with the backend whether this can still happen now that the unidentified-window state exists. If it can't, drop it.
