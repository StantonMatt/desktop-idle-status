// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.kirigami as Kirigami
import org.kde.notification as KNotification
import "Formatting.js" as Format

PlasmoidItem {
    id: root
    readonly property var snapshot: client.snapshot
    readonly property string state: client.loading ? "loading" : !client.available ? "service-down"
        : snapshot.State === "ready" && conflicts.length ? "late" : snapshot.State || "unknown"
    readonly property string iconState: state === "screensaver-off" ? "notrunning"
        : ["ready", "blocked", "late", "running"].includes(state) ? state : "unknown"
    readonly property var conflicts: Format.conflicts(snapshot.TimeoutConflicts || [])
    readonly property var history: Format.history(client.history, now)
    readonly property var policies: Format.policies(snapshot.LockSleepBlockers || [])
    readonly property var timeLocale: Qt.locale()
    readonly property string timePattern: timeLocaleProvider.pattern
    property double now: Date.now() / 1000
    readonly property string mainText: Format.statusTitle(state)
    // Plasma supplies KI18n's plural-aware functions through its QML context.
    // qmllint disable unqualified
    readonly property string timeoutText: Number(snapshot.ScreensaverTimeout) > 0
        ? i18np("After %1 minute of inactivity", "After %1 minutes of inactivity", Format.minutes(snapshot.ScreensaverTimeout)) : ""
    // qmllint enable unqualified
    readonly property string statusSubtext: state === "ready" || state === "late" ? timeoutText
        : state === "screensaver-off" ? client.screensaverStartFailed ? qsTr("Couldn't start Plasma Visual Screensaver")
            : qsTr("Plasma Visual Screensaver isn't running")
        : state === "running" && snapshot.RunningSince ? since(Number(snapshot.RunningSince))
        : state === "unknown" ? Format.unavailableText(snapshot.UnavailableCode) : ""
    readonly property alias blockerModel: blockerModel

    Plasmoid.status: PlasmaCore.Types.ActiveStatus
    Plasmoid.icon: Qt.resolvedUrl("../icons/ready-22.svg")
    toolTipTextFormat: Text.PlainText
    toolTipMainText: mainText
    toolTipSubText: state === "loading" ? "" : state === "service-down" ? qsTr("The Desktop Idle Status service isn't running")
        : state === "blocked" ? Format.blockedTooltip(snapshot.Blockers, root.namesText)
        : state === "late" ? settingTooltip(conflicts[0]) : statusSubtext
    Plasmoid.contextualActions: [clearAction]
    PlasmaCore.Action {
        id: clearAction
        objectName: "clearHistoryAction"
        text: qsTr("Clear History")
        icon.name: "edit-clear-history"
        visible: client.available && !client.loading && root.history.length > 0
        enabled: !client.clearingHistory
        onTriggered: client.clearHistory()
    }
    ListModel { id: blockerModel }
    function syncBlockers() {
        let rows = Format.blockers(snapshot.Blockers || []);
        if (snapshot.State === "blocked" && rows.length === 0) rows = [{internalId: "unattributed", unidentified: true, since: 0}];
        const ids = rows.map(row => row.internalId);
        for (let index = blockerModel.count - 1; index >= 0; --index)
            if (!ids.includes(blockerModel.get(index).record.internalId)) blockerModel.remove(index);
        for (let index = 0; index < rows.length; ++index) {
            let found = -1;
            for (let candidate = index; candidate < blockerModel.count; ++candidate)
                if (blockerModel.get(candidate).record.internalId === rows[index].internalId) { found = candidate; break; }
            if (found < 0) blockerModel.insert(index, {record: rows[index]});
            else { if (found !== index) blockerModel.move(found, index, 1); blockerModel.set(index, {record: rows[index]}); }
        }
    }
    onSnapshotChanged: syncBlockers()
    onExpandedChanged: if (!root.expanded) client.clearIgnoreErrors()
    function namesText(names, notification) {
        if (names.length === 1) return names[0];
        if (names.length === 2) return qsTr("%1 and %2").arg(names[0]).arg(names[1]);
        if (names.length === 3 && !notification) return qsTr("%1, %2 and %3").arg(names[0]).arg(names[1]).arg(names[2]);
        return names.length === 3 ? qsTr("%1, %2 and 1 other").arg(names[0]).arg(names[1])
            : qsTr("%1, %2 and %3 others").arg(names[0]).arg(names[1]).arg(names.length - 2);
    }
    function day(seconds) {
        const distance = Format.dayDistance(seconds, now);
        return distance === 0 ? qsTr("Today") : distance === 1 ? qsTr("Yesterday")
            : distance < 7 ? Format.shortDay(seconds, timeLocale) : Format.shortDate(seconds, timeLocale);
    }
    function since(seconds) {
        const distance = Format.dayDistance(seconds, now), time = Format.time(seconds, timeLocale, timePattern);
        return distance === 0 ? qsTr("Since %1").arg(time) : distance === 1 ? qsTr("Since yesterday %1").arg(time)
            : distance < 7 ? qsTr("Since %1 %2").arg(Format.shortDay(seconds, timeLocale)).arg(time)
            : qsTr("Since %1").arg(Format.shortDate(seconds, timeLocale));
    }
    function settingText(setting) {
        return setting === "lock" ? qsTr("Screen locks") : setting === "screen-off" ? qsTr("Screen turns off")
            : setting === "suspend" ? qsTr("Computer sleeps") : qsTr("Screen dims");
    }
    function settingTooltip(row) {
        // qmllint disable unqualified
        return Format.settingTooltip(row, function(singular, plural, count) {
            return i18np(singular, plural, count);
        });
        // qmllint enable unqualified
    }
    function policyText(row) {
        const name = Format.appName(row), reason = row.reason || qsTr("Unknown reason.");
        if (row.what === "idle:sleep") return qsTr("%1 is blocking sleep and screen locking. (%2)").arg(name).arg(reason);
        return row.what === "idle" ? qsTr("%1 is blocking screen locking. (%2)").arg(name).arg(reason)
            : qsTr("%1 is blocking sleep. (%2)").arg(name).arg(reason);
    }
    function notifyReturn(start, end, windows) {
        let body;
        // Preserve the service's longest-first order in the “By …” text.
        const names = Format.distinctNames(windows);
        if (!names.length || (windows.length === 1 && windows[0].appName === "Unidentified window")) body = qsTr("By an unidentified window");
        else if (windows.length === 1) body = Format.caption(windows[0]) ? qsTr("By %1 (%2)").arg(names[0]).arg(Format.caption(windows[0])) : qsTr("By %1").arg(names[0]);
        else body = qsTr("By %1").arg(namesText(names, true));
        const notification = notificationComponent.createObject(root, {
            title: qsTr("Screensaver was blocked for %1").arg(Format.duration(end - start)),
            text: Format.escapeMarkup(body),
            iconName: Format.notificationIcon(windows, "desktop-idle-status")
        });
        sendNotification(notification);
    }
    function sendNotification(notification: KNotification.Notification) { notification.sendEvent(); }
    TimeLocale { id: timeLocaleProvider }
    ServiceClient { id: client; onReturned: (start, end, windows) => root.notifyReturn(start, end, windows) }
    Timer { interval: 30000; running: true; repeat: true; onTriggered: root.now = Date.now() / 1000 }
    Component {
        id: notificationComponent
        KNotification.Notification {
            componentName: "desktop-idle-status"
            eventId: "blocked-while-away"
            hints: ({"desktop-entry": "io.github.StantonMatt.DesktopIdleStatus"})
            urgency: KNotification.Notification.NormalUrgency
            autoDelete: true
            defaultAction: KNotification.NotificationAction {
                label: qsTr("Open Desktop Idle Status")
                onActivated: { root.expanded = true; Qt.callLater(root.openHistory); }
            }
        }
    }
    function openHistory() { const item = fullRepresentationItem as FullRepresentation; if (item) item.showHistory(); }
    compactRepresentation: MouseArea {
        implicitWidth: Kirigami.Units.iconSizes.smallMedium
        implicitHeight: Kirigami.Units.iconSizes.smallMedium
        acceptedButtons: Qt.LeftButton | Qt.MiddleButton
        onClicked: mouse => { if (mouse.button === Qt.LeftButton) root.expanded = !root.expanded; }
        StatusIcon { anchors.centerIn: parent; state: root.iconState; size: 22 }
    }
    fullRepresentation: FullRepresentation { controller: root; client: client }
}
