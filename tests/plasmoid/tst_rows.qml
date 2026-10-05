pragma ComponentBehavior: Bound
import QtQuick
import QtTest
import "../../plasmoid/contents/ui" as Widget
import "../../plasmoid/contents/ui/Formatting.js" as Format

TestCase {
    id: test
    name: "Rows"
    width: 500; height: 500; visible: true
    when: windowShown
    Component {
        id: rowComponent
        Widget.StatusRow { width: 150; name: "<b>Firefox</b>"; caption: "Report — Firefox <img src='file:///missing'>" }
    }
    Component {
        id: fullComponent
        Widget.FullRepresentation { width: 432; height: 432 }
    }
    Component {
        id: navigationComponent
        Column {
            width: 432
            Widget.StatusRow {
                objectName: "first"; width: parent.width; name: "Claude"
                actionText: ignored ? "Stop Ignoring" : "Ignore"
                time: ignored ? "Ignored" : "Since 12:00"
                onActionTriggered: ignored = !ignored
            }
            Widget.StatusRow { objectName: "unknown"; width: parent.width; name: "Unidentified window"; interactive: false }
            Widget.StatusRow { objectName: "history"; width: parent.width; name: "History"; interactive: false; historyRow: true }
            Widget.StatusRow { objectName: "setting"; width: parent.width; name: "Screen locks" }
            Widget.StatusRow { objectName: "last"; width: parent.width; name: "Google Chrome"; actionText: "Ignore" }
        }
    }
    Component { id: clickSpy; SignalSpy { signalName: "clicked" } }
    QtObject {
        id: controller
        property string iconState: "ready"
        property string mainText: Format.statusTitle(state)
        property string statusSubtext: ""
        property string state: "ready"
        property var blockerModel: []
        property var conflicts: []
        property var history: []
        property var policies: [{appName:"<b>App</b>", reason:"<b>Reason</b>", what:"sleep"}]
        property var timeLocale: Qt.locale("es_CL")
        property string timePattern: "HH:mm"
        property bool expanded: true
        function since(seconds) { return "Since 12:00"; }
        function settingText(setting) { return setting; }
        function policyText(row) { return row.appName + " " + row.reason; }
        function day(seconds) { return "Today"; }
    }
    QtObject {
        id: client
        property bool loading: false
        property bool available: true
        property bool serviceStartFailed: false
        property bool startingService: false
        property bool startingScreensaver: false
        property bool historyFailed: false
        property bool clearHistoryFailed: false
        property var ignorePending: ({})
        property var ignoreErrors: ({})
        property var ignoredCalls: []
        function setAppIgnored(id, ignored) { ignoredCalls.push({id:id, ignored:ignored}); }
        property var activated: []
        function activateWindow(id) { activated.push(id); }
    }
    function objects(root) {
        let found = [root];
        for (const child of root.children || []) found = found.concat(objects(child));
        for (const child of root.contentItem ? [root.contentItem] : [])
            if (!found.includes(child)) found = found.concat(objects(child));
        return found;
    }
    function init() {
        controller.state = "ready"; controller.blockerModel = []; controller.conflicts = []; controller.history = [];
        client.ignorePending = {}; client.ignoreErrors = {}; client.ignoredCalls = [];
        client.activated = []; client.historyFailed = false; client.clearHistoryFailed = false;
        test.forceActiveFocus();
        mouseMove(test, width - 1, height - 1);
    }
    function test_blocked_header_keeps_ignored_row_actions() {
        controller.state = "blocked";
        controller.blockerModel = [{record:{internalId:"claude", appId:"claude", appName:"Claude", ignored:true, since:1}}];
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        verify(full);
        compare(findChild(full, "statusHeading").text, "Screensaver won't start");
        let row;
        tryVerify(() => { row = findChild(full, "blocker-claude"); return !!row; });
        compare(row.ignored, true);
        compare(row.time, "Ignored");
        compare(row.actionText, "Stop Ignoring");
        compare(Format.blockedTooltip([controller.blockerModel[0].record], names => names.join(" and ")), "Blocked by Claude");
    }
    function test_mouse_actions_hide_after_leaving_data() {
        return [{tag:"ignore", ignored:false}, {tag:"stop-ignoring", ignored:true}];
    }
    function test_mouse_actions_hide_after_leaving(data) {
        const group = createTemporaryObject(navigationComponent, test);
        const first = findChild(group, "first"), last = findChild(group, "last");
        first.ignored = data.ignored;
        const button = findChild(first, "ignoreButton"), other = findChild(last, "ignoreButton");
        compare(button.focusPolicy, Qt.TabFocus);
        mouseMove(first, 40, first.height / 2);
        tryCompare(button, "visible", true);
        mouseClick(button);
        compare(first.ignored, !data.ignored);
        mouseMove(last, 40, last.height / 2);
        tryCompare(button, "visible", false);
        tryCompare(other, "visible", true);
        mouseMove(test, width - 1, height - 1);
        tryCompare(other, "visible", false);
        compare(first.time, first.ignored ? "Ignored" : "Since 12:00");
        // Clicking a row body also must not leave its action visible.
        mouseClick(first, 40, first.height / 2);
        mouseMove(last, 40, last.height / 2);
        tryCompare(button, "visible", false);
        tryCompare(other, "visible", true);
        mouseMove(test, width - 1, height - 1);
        tryCompare(other, "visible", false);
        first.actionBusy = true;
        tryCompare(button, "visible", true);
        compare(button.enabled, false);
        first.actionBusy = false;
        tryCompare(button, "visible", false);
    }
    function test_non_action_rows_hover_data() {
        return [{tag:"setting", interactive:true, historyRow:false},
            {tag:"unidentified", interactive:false, historyRow:false},
            {tag:"history", interactive:false, historyRow:true}];
    }
    function test_non_action_rows_hover(data) {
        const row = createTemporaryObject(rowComponent, test, {interactive:data.interactive, historyRow:data.historyRow});
        const button = findChild(row, "ignoreButton"), tooltip = findChild(row, "rowTooltip");
        mouseMove(row, 40, row.height / 2);
        tryCompare(tooltip, "visible", true);
        compare(button.visible, false);
        mouseClick(row, 40, row.height / 2);
        compare(button.visible, false);
        mouseMove(test, width - 1, height - 1);
        tryCompare(tooltip, "visible", false);
        compare(button.visible, false);
    }
    function test_arrows_skip_actions_and_static_rows() {
        const group = createTemporaryObject(navigationComponent, test);
        const first = findChild(group, "first"), setting = findChild(group, "setting"), last = findChild(group, "last");
        const button = findChild(first, "ignoreButton"), other = findChild(last, "ignoreButton");
        compare(findChild(group, "unknown").focusPolicy, Qt.NoFocus);
        compare(findChild(group, "history").focusPolicy, Qt.NoFocus);
        first.forceActiveFocus(Qt.TabFocusReason);
        keyClick(Qt.Key_Down); tryCompare(setting, "activeFocus", true);
        keyClick(Qt.Key_Down); tryCompare(last, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(setting, "activeFocus", true);
        // Keep the previous action visible by hovering: Up must still skip it.
        mouseMove(first, 40, first.height / 2);
        tryCompare(button, "visible", true);
        keyClick(Qt.Key_Up); tryCompare(first, "activeFocus", true);
        keyClick(Qt.Key_Tab); tryCompare(button, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(first, "activeFocus", true);
        keyClick(Qt.Key_Tab); tryCompare(button, "activeFocus", true);
        keyClick(Qt.Key_Down); tryCompare(setting, "activeFocus", true);
        keyClick(Qt.Key_Down); tryCompare(last, "activeFocus", true);
        keyClick(Qt.Key_Tab); tryCompare(other, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(last, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(setting, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(first, "activeFocus", true);
    }
    function test_action_accessible_names_data() {
        return [{tag:"ignore", ignored:false, expected:"Ignore Claude"},
            {tag:"stop-ignoring", ignored:true, expected:"Stop ignoring Claude"}];
    }
    function test_action_accessible_names(data) {
        controller.state = "blocked";
        controller.blockerModel = [{record:{internalId:"claude", appId:"claude", appName:"Claude", ignored:data.ignored}}];
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        let row;
        tryVerify(() => { row = findChild(full, "blocker-claude"); return !!row; });
        row.forceActiveFocus(Qt.TabFocusReason);
        const button = findChild(row, "ignoreButton");
        compare(button.Accessible.name, data.expected);
    }
    function test_late_conflicts_precede_ignored_blockers() {
        controller.state = "late";
        controller.blockerModel = [{record:{internalId:"claude", appId:"claude", appName:"Claude", ignored:true}}];
        controller.conflicts = [{setting:"lock", seconds:20, kcm:"kcm_screenlocker"},
            {setting:"screen-off", seconds:30, kcm:"kcm_powerdevilprofilesconfig"}];
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        const flick = findChild(full, "statusScroll").contentItem;
        let blocker, lock, screen;
        tryVerify(() => {
            blocker = findChild(full, "blocker-claude"); lock = findChild(full, "setting-lock");
            screen = findChild(full, "setting-screen-off");
            return blocker && lock && screen && blocker.height > 0;
        });
        const top = row => row.mapToItem(flick.contentItem, 0, 0).y;
        tryVerify(() => top(lock) + lock.height <= top(screen) && top(screen) + screen.height <= top(blocker));
        lock.forceActiveFocus(Qt.TabFocusReason);
        keyClick(Qt.Key_Down); tryCompare(screen, "activeFocus", true);
        keyClick(Qt.Key_Down); tryCompare(blocker, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(screen, "activeFocus", true);
        keyClick(Qt.Key_Up); tryCompare(lock, "activeFocus", true);
    }
    function test_ignore_keyboard_and_row_activation() {
        const row = createTemporaryObject(rowComponent, test, {width:432, actionText:"Ignore", name:"App"});
        const rowSpy = createTemporaryObject(clickSpy, test, {target:row});
        const actionSpy = createTemporaryObject(clickSpy, test, {target:row, signalName:"actionTriggered"});
        const button = findChild(row, "ignoreButton");
        compare(button.visible, false);
        row.forceActiveFocus(Qt.TabFocusReason); tryCompare(button, "visible", true);
        keyClick(Qt.Key_Tab); verify(button.activeFocus);
        keyClick(Qt.Key_Return); compare(actionSpy.count, 1); compare(rowSpy.count, 0);
        keyClick(Qt.Key_Space); compare(actionSpy.count, 2); compare(rowSpy.count, 0);
        row.actionBusy = true; compare(button.enabled, false);
        row.actionBusy = false; row.forceActiveFocus(); keyClick(Qt.Key_Return); compare(rowSpy.count, 1);
    }
    function test_ignored_rows_ready_errors_and_unidentified() {
        controller.blockerModel = [{record:{internalId:"one",appId:"app",appName:"App",caption:"Title",ignored:true,since:1}},
            {record:{internalId:"unknown",appName:"Unidentified window",unidentified:true}}];
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client}); wait(50);
        const row = findChild(full,"blocker-one"), unknown = findChild(full,"blocker-unknown");
        verify(row.visible); compare(row.time,"Ignored"); compare(row.actionText,"Stop Ignoring");
        compare(findChild(row,"rowName").opacity,0.5); compare(unknown.actionText,"");
        row.actionTriggered(); compare(client.ignoredCalls[0].id,"app"); compare(client.ignoredCalls[0].ignored,false);
        client.ignorePending = {app:{ignored:false}}; compare(row.actionBusy,true);
        client.ignoreErrors = {app:"Couldn't stop ignoring App"};
        compare(row.caption,"Couldn't stop ignoring App"); verify(row.ignored);
        compare(findChild(row,"rowTooltip").text,"App\nCouldn't stop ignoring App");
    }
    function test_keyboard_activation_data() {
        return [
            {tag:"window", interactive:true, historyRow:false},
            {tag:"setting", interactive:true, historyRow:false},
            {tag:"unidentified", interactive:false, historyRow:false},
            {tag:"history", interactive:false, historyRow:true}
        ];
    }
    function test_keyboard_activation(data) {
        const row = createTemporaryObject(rowComponent, test, {interactive:data.interactive, historyRow:data.historyRow});
        const spy = createTemporaryObject(clickSpy, test, {target:row});
        row.forceActiveFocus(); verify(row.activeFocus);
        for (const event of [{key:Qt.Key_Return, modifiers:Qt.NoModifier},
            {key:Qt.Key_Enter, modifiers:Qt.KeypadModifier}, {key:Qt.Key_Space, modifiers:Qt.NoModifier}]) {
            const before = spy.count;
            keyClick(event.key, event.modifiers);
            compare(spy.count, before + (data.interactive ? 1 : 0));
        }
    }
    function test_focus_scroll_data() {
        return [{tag:"windows-arrows", state:"blocked", key:Qt.Key_Down, reverse:Qt.Key_Up},
            {tag:"windows-tab", state:"blocked", key:Qt.Key_Tab, reverse:Qt.Key_Backtab},
            {tag:"settings-arrows", state:"late", key:Qt.Key_Down, reverse:Qt.Key_Up},
            {tag:"settings-tab", state:"late", key:Qt.Key_Tab, reverse:Qt.Key_Backtab}];
    }
    function test_focus_scroll(data) {
        controller.state = data.state;
        if (data.state === "blocked") controller.blockerModel = Array.from({length:20}, (_, index) =>
            ({record:{internalId:"window-" + index, appName:"App " + index, caption:"Window", since:1}}));
        else controller.conflicts = Array.from({length:20}, (_, index) =>
            ({setting:"lock", seconds:20 + index, kcm:"kcm_powerdevilprofilesconfig"}));
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        wait(50);
        const scroll = findChild(full, "statusScroll"), flick = scroll.contentItem;
        const prefix = data.state === "blocked" ? "blocker-" : "setting-";
        const rows = objects(full).filter(item => item.objectName && item.objectName.startsWith(prefix))
            .sort((a,b) => a.mapToItem(flick.contentItem,0,0).y - b.mapToItem(flick.contentItem,0,0).y);
        compare(rows.length, 20);
        rows[0].forceActiveFocus();
        function visible(row) {
            const top = row.mapToItem(flick.contentItem, 0, 0).y;
            return top >= flick.contentY - 1 && top + row.height <= flick.contentY + flick.height + 1;
        }
        for (let index = 1; index < rows.length; ++index) {
            keyClick(data.key); tryCompare(rows[index], "activeFocus", true);
            tryVerify(() => visible(rows[index]));
        }
        verify(flick.contentY > 0);
        for (let index = rows.length - 2; index >= 0; --index) {
            keyClick(data.reverse); tryCompare(rows[index], "activeFocus", true);
            tryVerify(() => visible(rows[index]));
        }
    }
    function test_settings_page_and_deadline() {
        controller.state = "late";
        controller.conflicts = [{setting:"lock", seconds:20, kcm:"kcm_powerdevilprofilesconfig"},
            {setting:"screen-off", seconds:30, kcm:"kcm_powerdevilprofilesconfig"},
            {setting:"lock", seconds:60, kcm:"kcm_screenlocker"}];
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        wait(50);
        const rows = objects(full).filter(item => item.objectName && item.objectName.startsWith("setting-"));
        compare(rows.length, 3);
        for (const row of rows) {
            const screenlocker = row.time === "After 1 min";
            compare(row.caption, screenlocker ? "Screen Locking" : "Power Management");
            compare(row.iconName, screenlocker ? "preferences-desktop-user-password" : "preferences-system-power-management");
        }
        verify(rows.some(row => row.time === "After 20 s"));
    }
    function test_plain_rows_data() { return [{tag:"blocker", interactive:true, historyRow:false}, {tag:"history", interactive:false, historyRow:true}]; }
    function test_plain_rows(data) {
        const row = createTemporaryObject(rowComponent, test, {interactive:data.interactive, historyRow:data.historyRow});
        verify(row);
        compare(findChild(row, "rowName").textFormat, Text.PlainText);
        compare(findChild(row, "rowName").text, "<b>Firefox</b>");
        compare(findChild(row, "rowCaption").textFormat, Text.PlainText);
        compare(findChild(row, "rowCaption").text, row.caption);
        compare(findChild(row, "rowTooltip").textFormat, Text.PlainText);
        compare(findChild(row, "rowTooltip").text, row.name + "\n" + row.caption);
    }
    function test_caption_presentations_data() {
        const cases = [
            {tag:"empty", caption:"", expected:""},
            {tag:"whitespace", caption:" \t ", expected:""},
            {tag:"equal", caption:"Claude", expected:""},
            {tag:"trimmed-case", caption:" \tcLaUdE ", expected:""},
            {tag:"contains", caption:"Chat with Claude", expected:"Chat with Claude"}
        ];
        const rows = [];
        for (const historyRow of [false, true])
            for (const data of cases)
                rows.push(Object.assign({}, data, {tag:(historyRow ? "history-" : "blocker-") + data.tag, historyRow:historyRow}));
        return rows;
    }
    function test_caption_presentations(data) {
        const record = {appName:"Claude", caption:data.caption, internalId:"claude", since:1, start:1, end:3601};
        controller.state = "blocked";
        if (data.historyRow) controller.history = [record];
        else controller.blockerModel = [{record:record}];
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        verify(full);
        let row;
        tryVerify(() => {
            row = objects(full).find(item => item.name === "Claude" && item.historyRow === data.historyRow);
            return !!row;
        });
        compare(row.caption, data.expected);
        const caption = findChild(row, "rowCaption"), name = findChild(row, "rowName");
        compare(caption.text, data.expected);
        compare(caption.visible, !!data.expected);
        compare(findChild(row, "rowTooltip").text, "Claude" + (data.expected ? "\n" + data.expected : ""));
        if (!data.expected) {
            // Centre the sole name line even alongside two-line history times.
            tryVerify(() => Math.abs(name.mapToItem(row.contentItem, 0, name.height / 2).y - row.contentItem.height / 2) <= 1);
        }
    }
    function test_history_errors_and_plain_policies() {
        const full = createTemporaryObject(fullComponent, test, {controller:controller, client:client});
        verify(full); const status = findChild(full, "historyStatus");
        verify(status); compare(status.text, "None in the last 7 days");
        client.historyFailed = true; compare(status.text, "Couldn't read history"); verify(status.visible);
        client.clearHistoryFailed = true; compare(status.text, "Couldn't clear history"); verify(status.visible);
        client.clearHistoryFailed = false; client.historyFailed = false;
        let policies = 0;
        for (const object of objects(full))
            if (object.text === "<b>App</b> <b>Reason</b>") { compare(object.textFormat, Text.PlainText); ++policies; }
        verify(policies > 0);
    }
}
