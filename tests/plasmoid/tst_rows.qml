pragma ComponentBehavior: Bound
import QtQuick
import QtTest
import "../../plasmoid/contents/ui" as Widget

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
    Component { id: clickSpy; SignalSpy { signalName: "clicked" } }
    QtObject {
        id: controller
        property string iconState: "ready"
        property string mainText: "Ready"
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
        client.activated = []; client.historyFailed = false; client.clearHistoryFailed = false;
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
