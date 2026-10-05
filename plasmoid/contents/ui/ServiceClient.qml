// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import org.kde.plasma.workspace.dbus as DBus
import org.kde.plasma.plasma5support as Plasma5Support

Item {
    id: client
    readonly property string service: "io.github.StantonMatt.DesktopIdleStatus"
    readonly property string path: "/io/github/StantonMatt/DesktopIdleStatus"
    readonly property string iface: "io.github.StantonMatt.DesktopIdleStatus1"
    property var snapshot: ({})
    property var ignorePending: Object.create(null)
    property var ignoreErrors: Object.create(null)
    property int ignoreAttempt: 0
    property string ignoreSnapshot: ""
    property var history: []
    property bool historyFailed: false
    property bool clearHistoryFailed: false
    property bool clearingHistory: false
    property int historyRevision: 0
    property string serviceOwner: ""
    readonly property bool serviceRegistered: serviceOwner !== ""
    property int readRetryCount: 0
    property int screensaverAttempt: 0
    property int serviceAttempt: 0
    property string serviceCommand: ""
    property bool loading: true
    property bool available: false
    property bool startingScreensaver: false
    property bool startingService: false
    property bool screensaverStartFailed: false
    property bool serviceStartFailed: false
    property int generation: 0
    property bool fetching: false
    property bool refreshPending: false
    property var ownerSignals: null
    signal returned(double start, double end, var windows)

    // The Plasma module wraps D-Bus primitives in typed value objects. Convert
    // them at the boundary so strict comparisons and booleans remain reliable.
    function unwrap(value) {
        if (value === null || value === undefined) return value;
        if (typeof value === "object") {
            if (value.value !== undefined) return unwrap(value.value);
            if (value.length !== undefined) {
                const rows = [];
                for (let i = 0; i < value.length; ++i) rows.push(unwrap(value[i]));
                return rows;
            }
            const result = {};
            for (const key of Object.keys(value)) result[key] = unwrap(value[key]);
            return result;
        }
        return value;
    }
    function call(member, args, resolve, reject) {
        if (!serviceOwner) { if (reject) reject(); return; }
        DBus.SessionBus.asyncCall({service: serviceOwner, path: path, iface: iface,
            member: member, arguments: args || []}, resolve || function() {}, reject || function() {});
    }
    // All method callbacks share the same owner generation guard. Calls are also
    // pinned to that unique owner, so a queued action cannot reach a successor.
    function ownerCallback(callback) {
        const token = generation;
        return function(reply) {
            if (token === client.generation && callback) callback(reply);
        };
    }
    function guardedCall(member, args, resolve, reject) {
        call(member, args, ownerCallback(resolve), ownerCallback(reject));
    }
    function guardedSnapshot(resolve, reject) {
        requestSnapshot(ownerCallback(resolve), ownerCallback(reject));
    }
    function queryOwner() {
        loadingDeadline.start();
        DBus.SessionBus.asyncCall({service: "org.freedesktop.DBus", path: "/org/freedesktop/DBus",
            iface: "org.freedesktop.DBus", member: "GetNameOwner", arguments: [service]}, ownerCallback(function(reply) {
                client.serviceOwner = String(unwrap(reply.value));
            }), ownerCallback(function() {
                client.serviceOwner = "";
                client.loading = false;
                loadingDeadline.stop();
            }));
    }
    function requestSnapshot(resolve, reject) {
        DBus.SessionBus.asyncCall({service: serviceOwner, path: path, iface: "org.freedesktop.DBus.Properties",
            member: "GetAll", arguments: [iface]}, resolve, reject);
    }
    function scheduleReadRetry() {
        // At most three retries per failure burst, with capped backoff.
        if (serviceRegistered && readRetryCount < 3) {
            readRetry.interval = 1000 * Math.pow(2, readRetryCount++);
            readRetry.restart();
        }
    }
    function refresh() {
        if (!serviceOwner) return;
        if (fetching) { refreshPending = true; return; }
        fetching = true;
        const revision = historyRevision;
        guardedSnapshot(function(reply) {
                client.snapshot = unwrap(reply.value);
                if (client.snapshot.UnavailableReason)
                    console.warn("Desktop Idle Status:", client.snapshot.UnavailableCode || "unknown", client.snapshot.UnavailableReason);
                if (client.snapshot.ScreensaverOffReason)
                    console.debug("Desktop Idle Status:", client.snapshot.ScreensaverOffReason);
                client.loading = false;
                client.available = true;
                loadingDeadline.stop();
                if (client.startingService) { client.startingService = false; serviceDeadline.stop(); }
                client.serviceStartFailed = false;
                client.guardedCall("History", [new DBus.uint32(7)], function(historyReply) {
                    if (revision !== client.historyRevision) { client.finishRefresh(); return; }
                    client.history = unwrap(historyReply.value) || [];
                    client.historyFailed = false;
                    client.readRetryCount = 0;
                    readRetry.stop();
                    client.finishRefresh();
                }, function() {
                    if (revision !== client.historyRevision) { client.finishRefresh(); return; }
                    client.history = [];
                    client.historyFailed = true;
                    client.scheduleReadRetry();
                    client.finishRefresh();
                });
            }, function() {
                client.loading = false;
                client.available = false;
                client.history = [];
                client.historyFailed = true;
                client.scheduleReadRetry();
                client.finishRefresh();
            });
    }
    function finishRefresh() {
        fetching = false;
        if (refreshPending) { refreshPending = false; refresh(); }
    }
    function startScreensaver() {
        if (startingScreensaver) return;
        const token = ++screensaverAttempt;
        screensaverStartFailed = false;
        startingScreensaver = true;
        screensaverDeadline.restart();
        guardedCall("StartScreensaver", [], function(reply) {
            if (!unwrap(reply.value)) client.failScreensaverStart(token);
        }, function() { client.failScreensaverStart(token); });
    }
    function failScreensaverStart(token) {
        if (token !== screensaverAttempt || !startingScreensaver) return;
        screensaverDeadline.stop(); startingScreensaver = false; screensaverStartFailed = true;
    }
    function screensaverRegistered() {
        startingScreensaver = false;
        screensaverStartFailed = false;
        screensaverDeadline.stop();
        refresh();
    }
    function executeServiceCommand(source) { executable.connectSource(source); }
    function finishServiceCommand(source, exitCode) {
        if (source !== serviceCommand || !startingService) return;
        if (exitCode !== 0) failServiceStart(serviceAttempt);
        else {
            readRetryCount = 0;
            refresh(); // Starting an already-owned service emits no owner change.
        }
    }
    function startService() {
        if (startingService) return;
        ++serviceAttempt;
        serviceStartFailed = false;
        startingService = true;
        serviceDeadline.restart();
        // timeout bounds the process as well as the UI. The attempt suffix makes
        // every executable source unique even if a previous one is still exiting.
        serviceCommand = "timeout --signal=TERM --kill-after=1s 8s systemctl --user start desktop-idle-status.service # attempt " + serviceAttempt;
        executeServiceCommand(serviceCommand);
    }
    function failServiceStart(token) {
        if (token !== serviceAttempt || !startingService) return;
        startingService = false; serviceStartFailed = true; serviceDeadline.stop();
    }
    function clearHistory() {
        if (clearingHistory) return;
        clearingHistory = true;
        clearHistoryFailed = false;
        guardedCall("ClearHistory", [], function() {
            client.clearingHistory = false;
            client.historyRevision++;
            client.history = [];
            client.historyFailed = false;
            client.readRetryCount = 0;
            client.refresh();
        }, function() {
            client.clearingHistory = false;
            client.clearHistoryFailed = true;
        });
    }
    function claimReturn(id, start, end, windows) {
        guardedCall("ClaimReturnNotice", [new DBus.uint32(Number(unwrap(id)))], function(reply) {
            if (unwrap(reply.value))
                client.returned(Number(unwrap(start)), Number(unwrap(end)), unwrap(windows));
        });
    }
    function clearIgnoreErrors() { ignoreErrors = Object.create(null); }
    function setAppIgnored(appId, ignored) {
        if (!appId || !serviceOwner || ignorePending[appId] !== undefined) return;
        const token = ++ignoreAttempt;
        ignoreErrors = Object.assign(Object.create(null), ignoreErrors, {[appId]: ""});
        ignorePending = Object.assign(Object.create(null), ignorePending, {[appId]: {token:token, ignored:ignored, acknowledged:false, deadline:Date.now() + 5000}});
        guardedCall("SetAppIgnored", [appId, ignored], function() {
            const pending = client.ignorePending[appId];
            if (!pending || pending.token !== token) return;
            client.ignorePending = Object.assign(Object.create(null), client.ignorePending, {[appId]: Object.assign({}, pending, {acknowledged:true})});
            client.confirmIgnores(); client.refresh();
        },
            function() { client.failIgnore(appId, token); });
    }
    function failIgnore(appId, token) {
        const pending = ignorePending[appId];
        if (!pending || pending.token !== token) return;
        const rows = snapshot.Blockers || [];
        const row = rows.find(row => row.appId === appId);
        const name = row ? row.appName || appId : appId;
        ignoreErrors = Object.assign(Object.create(null), ignoreErrors, {[appId]: pending.ignored
            ? qsTr("Couldn't ignore %1").arg(name) : qsTr("Couldn't stop ignoring %1").arg(name)});
        const next = Object.assign(Object.create(null), ignorePending); delete next[appId]; ignorePending = next;
    }
    onSnapshotChanged: {
        const serialized = JSON.stringify(snapshot);
        if (serialized !== ignoreSnapshot) { clearIgnoreErrors(); ignoreSnapshot = serialized; }
        confirmIgnores();
    }
    function confirmIgnores() {
        const next = Object.assign(Object.create(null), ignorePending);
        for (const id of Object.keys(next)) {
            const ignored = (snapshot.IgnoredApps || []).includes(id);
            if (next[id].acknowledged && ignored === next[id].ignored) delete next[id];
        }
        ignorePending = next;
    }
    function activateWindow(id) { guardedCall("ActivateWindow", [id]); }

    onServiceOwnerChanged: {
        generation++;
        if (ownerSignals) {
            ownerSignals.enabled = false;
            ownerSignals.destroy();
            ownerSignals = null;
        }
        if (serviceOwner) ownerSignals = ownerSignalComponent.createObject(client, {
            // No bindings to the current owner: queued deliveries keep their
            // original identity even during deferred QObject destruction.
            service: serviceOwner,
            ownerToken: Object.freeze({generation: generation, owner: serviceOwner})
        });
        ignorePending = Object.create(null); clearIgnoreErrors();
        fetching = false;
        refreshPending = false;
        readRetryCount = 0;
        readRetry.stop();
        clearingHistory = false;
        clearHistoryFailed = false;
        historyFailed = false;
        available = false; snapshot = {}; history = [];
        failScreensaverStart(screensaverAttempt);
        if (serviceOwner) { loading = true; loadingDeadline.restart(); refresh(); }
        else { loading = false; loadingDeadline.stop(); }
    }
    DBus.SignalWatcher {
        service: "org.freedesktop.DBus"
        path: "/org/freedesktop/DBus"
        iface: "org.freedesktop.DBus"
        function dbusNameOwnerChanged(name, oldOwner, newOwner) {
            if (String(client.unwrap(name)) === client.service)
                client.serviceOwner = String(client.unwrap(newOwner));
        }
    }
    DBus.DBusServiceWatcher {
        id: screensaverOwner
        watchedService: "org.kde.PlasmaVisualScreensaver"
        onRegisteredChanged: if (registered) client.screensaverRegistered()
    }
    Component {
        id: ownerSignalComponent
        DBus.SignalWatcher {
            required property var ownerToken
            // QML clears var properties during destruction; stale deliveries
            // must also be harmless during that final teardown phase.
            readonly property int ownerGeneration: ownerToken ? ownerToken.generation : -1
            path: client.path
            iface: client.iface
            function dbusChanged() {
                if (ownerGeneration === client.generation) client.refresh();
            }
            function dbusBlockedWhileAway(id, start, end, windows) {
                if (ownerGeneration === client.generation) client.claimReturn(id, start, end, windows);
            }
        }
    }
    Plasma5Support.DataSource {
        id: executable
        engine: "executable"
        onNewData: (sourceName, data) => {
            disconnectSource(sourceName);
            client.finishServiceCommand(sourceName, data["exit code"]);
        }
    }
    Timer {
        interval: 250; running: Object.keys(client.ignorePending).length > 0; repeat: true
        onTriggered: {
            for (const id of Object.keys(client.ignorePending)) {
                const pending = client.ignorePending[id];
                if (Date.now() >= pending.deadline) client.failIgnore(id, pending.token);
            }
        }
    }
    Timer { id: loadingDeadline; interval: 5000; onTriggered: { client.loading = false; client.available = false; } }
    Timer { id: screensaverDeadline; interval: 10000; onTriggered: client.failScreensaverStart(client.screensaverAttempt) }
    Timer { id: serviceDeadline; interval: 10000; onTriggered: client.failServiceStart(client.serviceAttempt) }
    Timer { id: readRetry; onTriggered: client.refresh() }
    Component.onCompleted: queryOwner()
}
