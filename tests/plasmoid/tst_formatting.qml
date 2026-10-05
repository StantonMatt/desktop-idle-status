import QtQuick
import QtTest
import "../../plasmoid/contents/ui/Formatting.js" as Format
import "../../plasmoid/contents/ui" as Widget
import org.kde.plasma.workspace.dbus as DBus

TestCase {
    id: test
    name: "Formatting"
    Widget.ServiceClient { id: client }
    function test_ignore_keeps_row_order_and_filters_tooltip_names() {
        const rows = [{internalId:"a",appName:"Claude",since:1,ignored:true},
            {internalId:"b",appName:"Google Chrome",since:2,ignored:false}];
        compare(Format.blockers(rows)[0].internalId,"a");
        compare(Format.distinctNames(Format.activeBlockers(rows)).join(","),"Google Chrome");
        rows[1].ignored=true; compare(Format.activeBlockers(rows).length,0);
    }
    function test_blocked_tooltip_data() {
        const ignored = {appName:"Claude", ignored:true};
        return [
            {tag:"mixed", rows:[ignored, {appName:"Google Chrome"}], expected:"Blocked by Google Chrome"},
            {tag:"only-ignored", rows:[ignored], expected:"Blocked by Claude"},
            {tag:"multiple-ignored", rows:[ignored, {appName:"Chrome", ignored:true}], expected:"Blocked by Chrome and Claude"},
            {tag:"empty", rows:[], expected:"Blocked by an unidentified window"},
            {tag:"missing", rows:undefined, expected:"Blocked by an unidentified window"},
            {tag:"unnamed", rows:[{unidentified:true}], expected:"Blocked by an unidentified window"},
            {tag:"duplicates", rows:[ignored, {appName:"Firefox"}, {appName:"Firefox"}, {appName:"Chrome"}], expected:"Blocked by Chrome and Firefox"}
        ];
    }
    function test_blocked_tooltip(data) {
        function namesText(names, notification) {
            compare(notification, false);
            verify(names.length > 0);
            return names.join(" and ");
        }
        compare(Format.blockedTooltip(data.rows, namesText), data.expected);
    }
    function test_blocked_header_with_ignored_inhibitors() {
        compare(Format.statusTitle("blocked"), "Screensaver won't start");
        compare(Format.statusTitle("ready"), "Screensaver will start");
    }
    function test_caption() {
        compare(Format.caption({appName:"Firefox",caption:"YouTube — Firefox"}),"YouTube — Firefox");
        compare(Format.caption({appName:"Firefox",caption:"YouTube - Firefox"}),"YouTube - Firefox");
        compare(Format.caption({appName:"Firefox",caption:"YouTube — Haruna"}),"YouTube — Haruna");
        compare(Format.caption({appId:"steam_app_367520",caption:""}),"");
    }
    function test_caption_redundancy_data() {
        return [
            {tag:"missing", row:{appName:"Claude"}, expected:""},
            {tag:"empty", row:{appName:"Claude", caption:""}, expected:""},
            {tag:"whitespace", row:{appName:"Claude", caption:" \t\n "}, expected:""},
            {tag:"equal", row:{appName:"Claude", caption:"Claude"}, expected:""},
            {tag:"trimmed-case", row:{appName:" Claude ", caption:" \tcLaUdE\n"}, expected:""},
            {tag:"app-id", row:{appId:"claude", caption:" CLAUDE "}, expected:""},
            {tag:"contains", row:{appName:"Claude", caption:"Chat with Claude"}, expected:"Chat with Claude"},
            {tag:"prefix", row:{appName:"Claude", caption:"Claude conversation"}, expected:"Claude conversation"},
            {tag:"preserve-spacing", row:{appName:"Claude", caption:" Claude conversation "}, expected:" Claude conversation "}
        ];
    }
    function test_caption_redundancy(data) { compare(Format.caption(data.row), data.expected); }
    function test_caption_service_normalization() {
        // The raw title was Report — Firefox — Firefox; the service removed
        // exactly one suffix. Preserve the remaining title in every presentation.
        compare(Format.caption({appName:"Firefox", caption:"Report — Firefox"}), "Report — Firefox");
        compare(Format.caption({appName:"Firefox", caption:"Report — Firefox — Firefox"}), "Report — Firefox — Firefox");
    }
    function test_history_retention() {
        const now = 2000000, cutoff = now - 7 * 86400;
        const rows = [{start:cutoff-3600, end:cutoff}, {start:now-3600, end:now}];
        compare(Format.history(rows, now).length, 2); // Retain by end, not start.
        compare(Format.history(rows, now+1).length, 1);
        compare(Format.history(rows, now+7*86400+1).length, 0);
    }
    function test_names() {
        compare(Format.distinctNames([{appName:"Firefox"},{appName:"Haruna"},{appName:"Firefox"}]),["Firefox","Haruna"]);
        compare(Format.appName({appId:"steam_app_367520"}),"steam_app_367520");
    }
    function test_return_notification() {
        // Service ordering differs from both alphabetical and oldest-first order.
        const windows = [{appName:"Haruna", iconName:"haruna", caption:"Movie", seconds:19800},
            {appName:"Firefox", iconName:"firefox", caption:"YouTube", seconds:600},
            {appName:"Haruna", iconName:"haruna", caption:"Other movie", seconds:300}];
        compare(Format.distinctNames(windows), ["Haruna", "Firefox"]);
        compare(Format.notificationIcon(windows, "widget"), "haruna");
        compare(Format.notificationIcon([{appName:"Unidentified window", iconName:"preferences-system-windows", seconds:600}], "widget"), "widget");
        compare(Format.notificationIcon([{appName:"Firefox", seconds:600}], "widget"), "widget");
        compare(Format.notificationIcon([], "widget"), "widget");
    }
    function test_duration() {
        compare(Format.duration(2700),"45\u00a0min");
        compare(Format.duration(4500),"1\u00a0h\u00a015\u00a0min");
        compare(Format.duration(18000),"5\u00a0h");
        compare(Format.duration(3599),"1\u00a0h");
        compare(Format.duration(89),"1\u00a0min");
        compare(Format.duration(90),"2\u00a0min");
    }
    function test_settings_deadlines() {
        function plural(singular, plural, count) { return (count === 1 ? singular : plural).replace("%1", count); }
        const actions = {lock:"Screen locks", "screen-off":"Screen turns off", suspend:"Computer sleeps", dim:"Screen dims"};
        for (const seconds of [1, 20, 29, 30, 59, 60, 89, 90]) {
            const subminute = seconds < 60, count = subminute ? seconds : Format.minutes(seconds);
            compare(Format.settingAfter(seconds), "After " + count + (subminute ? " s" : " min"));
            for (const setting of Object.keys(actions))
                compare(Format.settingTooltip({setting:setting, seconds:seconds}, plural),
                    actions[setting] + " after " + count + (subminute ? " second" : " minute") + (count === 1 ? "" : "s"));
        }
    }
    function test_settings_pages() {
        compare(Format.settingsPageName("kcm_screenlocker"), "Screen Locking");
        compare(Format.settingsPageIcon("kcm_screenlocker"), "preferences-desktop-user-password");
        compare(Format.settingsPageName("kcm_powerdevilprofilesconfig"), "Power Management");
        compare(Format.settingsPageIcon("kcm_powerdevilprofilesconfig"), "preferences-system-power-management");
    }
    function test_minutes() {
        compare(Format.minutes(89), 1);
        compare(Format.minutes(90), 2);
        compare(Format.minutes(3599), 60);
        compare(Format.minutes(0), 0);
    }
    function test_unavailable_text() {
        for (const code of ["bridge-missing", "bridge-version-mismatch", "bridge-error", "future-code", undefined])
            compare(Format.unavailableText(code), "Can't read idle inhibitors from KWin");
        compare(Format.unavailableText("initializing"), "Checking screensaver status");
        compare(Format.unavailableText("policyagent-unavailable"), "Can't read screensaver activity from PowerDevil");
    }
    function test_ordering() {
        compare(Format.blockers([{since:2,appName:"A"},{since:1,appName:"C"},{since:1,appName:"B"}]).map(x=>x.appName),["B","C","A"]);
        compare(Format.conflicts([{seconds:600,setting:"lock"},{seconds:300,setting:"dim"},{seconds:300,setting:"suspend"},{seconds:300,setting:"screen-off"},{seconds:300,setting:"lock"}]).map(x=>x.setting),["lock","screen-off","suspend","dim","lock"]);
        compare(Format.history([{start:1},{start:3},{start:2}]).map(x=>x.start),[3,2,1]);
        compare(Format.policies([{appName:"Firefox",what:"idle"},{appName:"Firefox",what:"sleep"}])[0].what,"idle:sleep");
        compare(Format.policies([{appName:"Haruna"},{appName:"Elisa"}]).map(x=>x.appName),["Elisa","Haruna"]);
    }
    function test_blocker_caption_order() {
        // Hiding a redundant caption must not change the window-title tie break.
        compare(Format.blockers([
            {since:1, appName:"Claude", caption:"Claude", internalId:"a"},
            {since:1, appName:"Claude", caption:"", internalId:"z"}
        ]).map(row => row.internalId), ["z", "a"]);
    }
    function test_days() {
        const now=new Date(2026,9,4,12).getTime()/1000;
        compare(Format.dayDistance(new Date(2026,9,4,1).getTime()/1000,now),0);
        compare(Format.dayDistance(new Date(2026,9,3,23,58).getTime()/1000,now),1);
        compare(Format.dayDistance(new Date(2026,8,27).getTime()/1000,now),7);
        compare(Format.time(new Date(2026,9,4,21,14).getTime()/1000,Qt.locale("es_CL"),"HH:mm"),"21:14");
    }
    function test_dbus_values() {
        compare(client.unwrap(new DBus.bool(false)),false);
        compare(client.unwrap(new DBus.string("ready")),"ready");
        compare(client.unwrap({State:new DBus.string("ready"),Blockers:[{since:new DBus.int64(1234),caption:new DBus.string("YouTube")}]}),{State:"ready",Blockers:[{since:1234,caption:"YouTube"}]});
    }
    function test_return_dbus_values() {
        compare(client.unwrap([{appName:new DBus.string("Haruna"),seconds:new DBus.uint32(19800)}]),[{appName:"Haruna",seconds:19800}]);
    }
    function test_markup() { compare(Format.escapeMarkup("A & <B>"),"A &amp; &lt;B&gt;"); }
}
