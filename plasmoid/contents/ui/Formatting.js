.pragma library

function appName(row) { return String(row.appName || row.appId || ""); }
// Captions are already normalized by the service, including history/notices.
function caption(row) { return String(row.caption || ""); }
function distinctNames(rows) {
    const names = [];
    for (const row of rows) {
        const name = appName(row);
        if (name && !names.includes(name)) names.push(name);
    }
    return names;
}
function minutes(seconds) { return Math.round(Math.max(0, Number(seconds)) / 60); }
function duration(seconds) {
    const total = minutes(seconds);
    const hours = Math.floor(total / 60), rest = total % 60;
    return hours ? qsTr("%n\u00a0h", "", hours) + (rest ? "\u00a0" + qsTr("%n\u00a0min", "", rest) : "")
        : qsTr("%n\u00a0min", "", total);
}
// Deadlines below a minute must retain their precision; history durations
// deliberately keep their separate nearest-minute presentation.
function deadlineInSeconds(seconds) { return Number(seconds) > 0 && Number(seconds) < 60; }
function settingAfter(seconds) {
    return deadlineInSeconds(seconds) ? qsTr("After %n s", "", Number(seconds))
        : qsTr("After %n min", "", minutes(seconds));
}
function settingsPageName(kcm) {
    return kcm === "kcm_screenlocker" ? qsTr("Screen Locking") : qsTr("Power Management");
}
function settingsPageIcon(kcm) {
    return kcm === "kcm_screenlocker" ? "preferences-desktop-user-password" : "preferences-system-power-management";
}
function settingTooltip(row, plural) {
    const seconds = deadlineInSeconds(row.seconds);
    const value = seconds ? Number(row.seconds) : minutes(row.seconds);
    // Whole sentences allow translators to reorder words.
    if (row.setting === "lock") return seconds
        ? plural("Screen locks after %1 second", "Screen locks after %1 seconds", value)
        : plural("Screen locks after %1 minute", "Screen locks after %1 minutes", value);
    if (row.setting === "screen-off") return seconds
        ? plural("Screen turns off after %1 second", "Screen turns off after %1 seconds", value)
        : plural("Screen turns off after %1 minute", "Screen turns off after %1 minutes", value);
    if (row.setting === "suspend") return seconds
        ? plural("Computer sleeps after %1 second", "Computer sleeps after %1 seconds", value)
        : plural("Computer sleeps after %1 minute", "Computer sleeps after %1 minutes", value);
    return seconds ? plural("Screen dims after %1 second", "Screen dims after %1 seconds", value)
        : plural("Screen dims after %1 minute", "Screen dims after %1 minutes", value);
}
function unavailableText(code) {
    if (code === "initializing") return qsTr("Checking screensaver status");
    if (code === "policyagent-unavailable") return qsTr("Can't read screensaver activity from PowerDevil");
    // Unknown/missing codes from older or newer services get safe fixed wording.
    return qsTr("Can't read idle inhibitors from KWin");
}
function dayDistance(seconds, now) {
    const date = new Date(seconds * 1000), today = new Date(now * 1000);
    return Math.round((Date.UTC(today.getFullYear(), today.getMonth(), today.getDate())
        - Date.UTC(date.getFullYear(), date.getMonth(), date.getDate())) / 86400000);
}
function time(seconds, locale, pattern) { return new Date(seconds * 1000).toLocaleTimeString(locale, pattern || locale.timeFormat(1)); }
function shortDay(seconds, locale) { return new Date(seconds * 1000).toLocaleDateString(locale, "ddd"); }
function shortDate(seconds, locale) { return new Date(seconds * 1000).toLocaleDateString(locale, "d MMM"); }
function blockers(rows) {
    return rows.slice().sort((a, b) => Number(a.since) - Number(b.since)
        || appName(a).localeCompare(appName(b)) || caption(a).localeCompare(caption(b))
        || (a.internalId || "").localeCompare(b.internalId || ""));
}
function conflicts(rows) {
    const rank = {lock: 0, "screen-off": 1, suspend: 2, dim: 3};
    return rows.slice().sort((a, b) => Number(a.seconds) - Number(b.seconds) || rank[a.setting] - rank[b.setting]);
}
function history(rows, now) {
    return rows.filter(row => now === undefined || Number(row.end) >= now - 7 * 86400)
        .sort((a, b) => Number(b.start) - Number(a.start));
}
function policies(rows) {
    const combined = [];
    for (const row of rows) {
        const existing = combined.find(other => appName(other) === appName(row)
            && other.reason === row.reason && other.source === row.source
            && other.mode === row.mode && other.iconName === row.iconName);
        if (existing) {
            if (existing.what !== row.what) existing.what = "idle:sleep";
        } else combined.push(Object.assign({}, row));
    }
    return combined.sort((a, b) => appName(a).localeCompare(appName(b)));
}
function escapeMarkup(text) { return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"); }

// The service orders return rows longest-first, including its tie breaks.
function notificationIcon(windows, fallback) {
    const longest = windows[0];
    return longest && appName(longest) !== "Unidentified window" && longest.iconName ? longest.iconName : fallback;
}
