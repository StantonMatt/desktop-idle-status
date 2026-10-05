pragma ComponentBehavior: Bound
import QtQuick
import QtTest
import org.kde.plasma.workspace.dbus as DBus
import "../../plasmoid/contents/ui" as Widget

TestCase {
    id: test
    name: "ServiceClient"
    property var reads: []
    property var calls: []
    property var commands: []
    Component {
        id: simulated
        Widget.ServiceClient {
            serviceOwner: ""
            function queryOwner() {}
            function requestSnapshot(resolve, reject) { test.reads.push({resolve:resolve, reject:reject}); }
            function call(member, args, resolve, reject) { test.calls.push({member:member, args:args, resolve:resolve, reject:reject}); }
            function executeServiceCommand(source) { test.commands.push(source); }
        }
    }
    Component { id: actual; Widget.ServiceClient {} }
    Component { id: spyComponent; SignalSpy { signalName: "returned" } }
    function init() { reads = []; calls = []; commands = []; failOnWarning(/.*TypeError.*/); }
    function makeClient() { return createTemporaryObject(simulated, test); }
    function snapshot(index) { reads[index || 0].resolve({value:{State:"ready"}}); }
    function history(index, rows) { calls[index || 0].resolve({value:rows || []}); }
    function test_ignore_confirmation_failure_and_owner_change() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        reads[0].resolve({value:{State:"blocked", IgnoredApps:[], Blockers:[{appId:"app",appName:"App",ignored:false}]}});
        history();
        client.setAppIgnored("app", true); client.setAppIgnored("app", true);
        compare(calls.length, 2); compare(calls[1].member, "SetAppIgnored");
        verify(client.ignorePending.app !== undefined);
        compare(client.snapshot.Blockers[0].ignored, false);
        client.snapshot = {State:"ready", IgnoredApps:["app"], Blockers:[{appId:"app",appName:"App",ignored:true}]};
        verify(client.ignorePending.app !== undefined); // Snapshot alone does not acknowledge the method.
        calls[1].resolve(); verify(client.ignorePending.app === undefined);
        client.setAppIgnored("app", false); calls[2].reject();
        compare(client.ignoreErrors.app, "Couldn't stop ignoring App");
        compare(client.snapshot.Blockers[0].ignored, true);
        client.clearIgnoreErrors(); compare(Object.keys(client.ignoreErrors).length, 0);
        client.setAppIgnored("app", false);
        client.serviceOwner = ":fixture.2"; calls[3].reject();
        compare(Object.keys(client.ignorePending).length, 0);
        compare(Object.keys(client.ignoreErrors).length, 0);
    }
    function test_ignore_failure_and_timeout() {
        const client = makeClient(); client.serviceOwner = ":fixture.1"; snapshot(); history();
        client.snapshot = {State:"blocked", IgnoredApps:[], Blockers:[{appId:"app",appName:"App",ignored:false}]};
        client.setAppIgnored("app", true); calls[1].reject();
        compare(client.ignoreErrors.app, "Couldn't ignore App");
        client.snapshot = {State:"ready", IgnoredApps:[], Blockers:[]};
        compare(Object.keys(client.ignoreErrors).length, 0);
        client.setAppIgnored("app", true);
        const token = client.ignorePending.app.token;
        client.ignorePending = {app:{token:token, ignored:true, deadline:Date.now() - 1}};
        tryVerify(() => client.ignorePending.app === undefined, 1000);
        compare(client.ignoreErrors.app, "Couldn't ignore app");
    }
    function test_ignore_app_ids_do_not_use_object_prototypes() {
        const client = makeClient(); client.serviceOwner = ":fixture.1"; snapshot(); history();
        for (const id of ["constructor", "__proto__", "toString"]) {
            client.setAppIgnored(id, true);
            verify(Object.keys(client.ignorePending).includes(id));
        }
        compare(calls.length, 4);
    }
    function test_getall_retry_and_bound() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        compare(reads.length, 1);
        reads[0].reject(); compare(client.available, false);
        tryVerify(() => reads.length === 2, 1500);
        reads[1].reject(); tryVerify(() => reads.length === 3, 2500);
        reads[2].reject(); tryVerify(() => reads.length === 4, 4500);
        reads[3].reject(); compare(client.readRetryCount, 3);
        // Exhaustion schedules no further timer. A manual start resets the burst.
        client.startService(); client.finishServiceCommand(commands[0], 0);
        compare(reads.length, 5); snapshot(4); history();
        compare(client.available, true); compare(client.readRetryCount, 0);
        compare(client.startingService, false);
    }
    function test_history_failure_and_recovery() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        client.history = [{start:1,end:2}]; snapshot(); calls[0].reject();
        compare(client.historyFailed, true); compare(client.history.length, 0);
        compare(client.available, true);
        tryVerify(() => reads.length === 2, 1500);
        snapshot(1); history(1, [{start:3,end:4}]);
        compare(client.historyFailed, false); compare(client.history.length, 1);
    }
    function test_clear_failure_retry_and_stale_read() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        snapshot(); history(0, [{start:1,end:2}]);
        client.clearHistory(); client.clearHistory(); compare(calls.length, 2);
        calls[1].reject(); compare(client.clearHistoryFailed, true);
        compare(client.clearingHistory, false); compare(client.history.length, 1);
        client.refresh(); snapshot(1); // History read is in flight during clear.
        client.clearHistory(); compare(client.clearHistoryFailed, false);
        calls[3].resolve(); compare(client.history.length, 0);
        calls[2].resolve({value:[{start:1,end:2}]}); // Pre-clear reply is obsolete.
        compare(client.history.length, 0); compare(reads.length, 3);
        snapshot(2); calls[4].reject(); compare(client.historyFailed, true);
        compare(client.history.length, 0);
    }
    function test_owner_change_invalidates_reads_clear_and_claim() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        client.clearHistory(); client.claimReturn(1, 1, 601, []);
        const spy = createTemporaryObject(spyComponent, test, {target:client});
        client.serviceOwner = "";
        reads[0].resolve({value:{State:"ready"}});
        calls[0].reject(); calls[1].resolve({value:true});
        compare(client.available, false); compare(client.clearHistoryFailed, false);
        compare(client.clearingHistory, false); compare(spy.count, 0);
    }
    function test_direct_owner_change_invalidates_snapshot_and_actions() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        client.clearHistory(); client.claimReturn(1, 1, 601, []);
        client.startScreensaver(); client.activateWindow("old-window");
        const spy = createTemporaryObject(spyComponent, test, {target:client});
        const generation = client.generation;
        client.serviceOwner = ":fixture.2";
        compare(client.generation, generation + 1);
        verify(client.serviceRegistered); compare(reads.length, 2);
        snapshot(1); history(4, [{start:3, end:4}]);
        reads[0].resolve({value:{State:"stale"}});
        calls[0].resolve(); calls[0].reject();
        calls[1].resolve({value:true}); calls[2].resolve({value:false}); calls[2].reject();
        calls[3].resolve({value:true}); calls[3].reject();
        compare(client.snapshot.State, "ready"); compare(client.history[0].start, 3);
        compare(client.clearingHistory, false); compare(client.clearHistoryFailed, false);
        compare(spy.count, 0);
        // A new owner's screensaver attempt also ignores the previous reply.
        client.startScreensaver(); calls[2].reject();
        compare(client.startingScreensaver, true); compare(client.screensaverStartFailed, false);
        calls[5].reject(); compare(client.screensaverStartFailed, true);
    }
    function test_direct_owner_change_invalidates_history() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        snapshot(); client.serviceOwner = ":fixture.2";
        snapshot(1); history(1, [{start:3,end:4}]);
        calls[0].resolve({value:[{start:1,end:2}]}); calls[0].reject();
        compare(client.history[0].start, 3); compare(client.historyFailed, false);
        compare(client.fetching, false);
    }
    function test_queued_signals_across_owner_replacement() {
        const client = makeClient(); client.serviceOwner = ":fixture.1";
        snapshot(); history();
        const spy = createTemporaryObject(spyComponent, test, {target:client});
        const old = client.ownerSignals;
        const token = old.ownerGeneration;
        const changed = old.dbusChanged, returned = old.dbusBlockedWhileAway;
        let delivered = false;
        // Model native deliveries already queued before owner replacement.
        Qt.callLater(function() {
            changed(); returned(1, 1, 601, [{appName:"Old owner"}]);
            delivered = true;
        });
        client.serviceOwner = ":fixture.2";
        compare(old.ownerGeneration, token); compare(old.service, ":fixture.1");
        snapshot(1); history(1);
        tryVerify(() => delivered);
        compare(reads.length, 2); compare(calls.length, 2); compare(spy.count, 0);
        // A successor may reuse the same ID. Only its payload can be claimed.
        client.ownerSignals.dbusBlockedWhileAway(1, 2, 602, [{appName:"New owner"}]);
        compare(calls[2].member, "ClaimReturnNotice"); calls[2].resolve({value:true});
        compare(spy.count, 1); compare(spy.signalArguments[0][2][0].appName, "New owner");
        client.ownerSignals.dbusChanged(); compare(reads.length, 3);
    }
    function fixture(member) {
        let finished = false, result;
        DBus.SessionBus.asyncCall({service:"org.example.Fixture", path:"/Fixture",
            iface:"org.example.Fixture", member:member}, function(reply) { result = reply.value; finished = true; }, function(error) { fail(error.message); finished = true; });
        tryVerify(() => finished, 2500);
        return result;
    }
    function test_mock_direct_owner_replacement() {
        const client = createTemporaryObject(actual, test);
        const spy = createTemporaryObject(spyComponent, test, {target:client});
        tryCompare(client, "available", true);
        const owner = client.serviceOwner, generation = client.generation;
        fixture("HoldRequests");
        try {
            client.refresh(); client.clearHistory(); client.claimReturn(1, 1, 601, []);
            client.startScreensaver(); client.activateWindow("old-window");
            tryVerify(() => Number(client.unwrap(fixture("PendingCount"))) === 5, 2500);
            fixture("ReplaceOwner");
            tryVerify(() => client.serviceOwner !== owner, 2500);
            compare(client.generation, generation + 1); verify(client.serviceRegistered);
            tryCompare(client, "available", true);
            tryCompare(client, "fetching", false);
            compare(client.snapshot.State, "running"); compare(client.history.length, 0);
            client.startScreensaver(); // Replacement answers false.
            tryCompare(client, "screensaverStartFailed", true);
            fixture("ReleaseOldReplies"); wait(100);
            compare(client.snapshot.State, "running"); compare(client.history.length, 0);
            compare(client.clearHistoryFailed, false); compare(client.clearingHistory, false);
            compare(spy.count, 0);
        } finally {
            fixture("ReleaseOldReplies"); fixture("RestoreOwner");
        }
    }
    function test_screensaver_attempts() {
        const client = makeClient(); client.startScreensaver();
        const first = client.screensaverAttempt;
        client.failScreensaverStart(first); client.startScreensaver();
        calls[0].reject(); calls[0].resolve({value:false});
        client.failScreensaverStart(first);
        compare(client.startingScreensaver, true); compare(client.screensaverStartFailed, false);
        client.screensaverRegistered(); calls[1].reject(); calls[1].resolve({value:false});
        compare(client.startingScreensaver, false); compare(client.screensaverStartFailed, false);
        client.startScreensaver(); calls[2].reject(); compare(client.screensaverStartFailed, true);
    }
    function test_service_attempts() {
        const client = makeClient(); client.startService();
        const first = client.serviceAttempt, source = commands[0];
        verify(source.includes("timeout --signal=TERM --kill-after=1s 8s"));
        client.failServiceStart(first); client.startService();
        compare(commands.length, 2); verify(commands[0] !== commands[1]);
        client.finishServiceCommand(source, 1); client.finishServiceCommand(source, 0);
        client.failServiceStart(first);
        compare(client.startingService, true); compare(client.serviceStartFailed, false);
        client.serviceOwner = ":fixture.1"; snapshot(); history();
        client.finishServiceCommand(commands[1], 1);
        compare(client.startingService, false); compare(client.serviceStartFailed, false);
    }
    function test_claim_false_failure_and_values() {
        const client = makeClient();
        const spy = createTemporaryObject(spyComponent, test, {target:client});
        client.claimReturn(new DBus.uint32(1), new DBus.int64(1), new DBus.int64(601), []);
        compare(client.unwrap(calls[0].args[0]), 1);
        calls[0].resolve({value:new DBus.bool(false)}); compare(spy.count, 0);
        client.claimReturn(2, 1, 601, []); // Rejection uses the boundary's no-op handler.
        calls[1].reject(); compare(spy.count, 0);
        client.claimReturn(3, 1, 601, []); calls[2].resolve({value:new DBus.bool(true)});
        compare(spy.count, 1); compare(Array.from(spy.signalArguments[0]), [1,601,[]]);
    }
    function test_two_instances_claim_one_return() {
        const first = createTemporaryObject(actual, test), second = createTemporaryObject(actual, test);
        tryCompare(first, "available", true); tryCompare(second, "available", true);
        const one = createTemporaryObject(spyComponent, test, {target:first});
        const two = createTemporaryObject(spyComponent, test, {target:second});
        DBus.SessionBus.asyncCall({service:"org.example.Fixture", path:"/Fixture", iface:"org.example.Fixture", member:"EmitReturn"});
        tryVerify(() => one.count + two.count === 1, 2500);
        wait(100); compare(one.count + two.count, 1);
    }
}
