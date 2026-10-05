// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.extras as PlasmaExtras
import org.kde.plasma.components as PC3
import QtQuick.Controls as QQC2
import org.kde.kcmutils as KCMUtils
import "Formatting.js" as Format

PlasmaExtras.Representation {
    id: full
    required property var controller
    required property var client
    collapseMarginsHint: true
    Layout.minimumWidth: Kirigami.Units.gridUnit * 24
    Layout.minimumHeight: Kirigami.Units.gridUnit * 24
    Layout.preferredWidth: Kirigami.Units.gridUnit * 24
    Layout.preferredHeight: Kirigami.Units.gridUnit * 24

    function showHistory() {
        const flick = scroll.contentItem;
        Qt.callLater(function() {
            flick.contentY = Math.max(0, Math.min(historyHeading.y,
                flick.contentHeight - flick.height));
        });
    }
    PC3.BusyIndicator { anchors.centerIn: parent; visible: full.client.loading; running: visible }
    PlasmaExtras.PlaceholderMessage {
        id: servicePlaceholder
        type: PlasmaExtras.PlaceholderMessage.Actionable
        anchors.centerIn: parent
        width: parent.width - Kirigami.Units.gridUnit * 2
        visible: !full.client.loading && !full.client.available
        iconName: Qt.resolvedUrl("../icons/unknown-32.svg")
        text: qsTr("Screensaver status unknown")
        explanation: full.client.serviceStartFailed ? qsTr("Couldn't start the Desktop Idle Status service")
            : qsTr("The Desktop Idle Status service isn't running")
        PC3.Button {
            text: qsTr("Start Service")
            icon.name: "media-playback-start"
            visible: full.client.startingService
            enabled: false
            Layout.alignment: Qt.AlignHCenter
            Layout.topMargin: Kirigami.Units.gridUnit
        }
        helpfulAction: QQC2.Action {
            objectName: "startServiceAction"
            text: qsTr("Start Service")
            icon.name: "media-playback-start"
            enabled: !full.client.startingService
            onTriggered: full.client.startService()
        }
    }
    PC3.ScrollView {
        id: scroll
        objectName: "statusScroll"
        anchors.fill: parent
        visible: !full.client.loading && full.client.available
        contentWidth: availableWidth
        QQC2.ScrollBar.horizontal.policy: QQC2.ScrollBar.AlwaysOff
        QQC2.ScrollBar.vertical.policy: QQC2.ScrollBar.AsNeeded
        ColumnLayout {
            width: scroll.availableWidth
            spacing: 0
            RowLayout {
                Layout.fillWidth: true
                Layout.margins: Kirigami.Units.gridUnit
                spacing: Kirigami.Units.gridUnit
                StatusIcon { state: full.controller.iconState; size: 32; Layout.alignment: Qt.AlignTop }
                ColumnLayout {
                    Layout.fillWidth: true
                    spacing: Kirigami.Units.smallSpacing
                    Kirigami.Heading { textFormat: Text.PlainText; level: 3; text: full.controller.mainText; wrapMode: Text.WordWrap; Layout.fillWidth: true }
                    PC3.Label {
                        textFormat: Text.PlainText
                        text: full.controller.statusSubtext
                        visible: text.length > 0
                        opacity: 0.75
                        wrapMode: Text.WordWrap
                        Layout.fillWidth: true
                    }
                    PC3.Button {
                        objectName: "startScreensaverButton"
                        text: qsTr("Start")
                        icon.name: "media-playback-start"
                        visible: full.controller.state === "screensaver-off"
                        enabled: !full.client.startingScreensaver
                        onClicked: full.client.startScreensaver()
                    }
                }
            }
            ListView {
                id: blockers
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.gridUnit + 32 + Kirigami.Units.gridUnit - Kirigami.Units.smallSpacing
                Layout.rightMargin: Kirigami.Units.gridUnit - Kirigami.Units.smallSpacing
                Layout.preferredHeight: contentHeight
                interactive: false
                visible: full.controller.state === "blocked"
                model: full.controller.blockerModel
                delegate: StatusRow {
                    focusViewport: scroll.contentItem as Flickable
                    required property var record
                    objectName: "blocker-" + record.internalId
                    width: blockers.width
                    name: record.unidentified ? qsTr("Unidentified window") : Format.appName(record)
                    caption: record.unidentified ? qsTr("KWin plugin isn't loaded") : Format.caption(record)
                    iconName: record.unidentified ? "preferences-system-windows" : record.iconName || "application-x-executable"
                    time: record.since ? full.controller.since(Number(record.since)) : ""
                    interactive: !record.unidentified
                    onClicked: if (interactive) full.client.activateWindow(record.internalId)
                }
                remove: Transition { NumberAnimation { property: "opacity"; to: 0; duration: Kirigami.Units.shortDuration } }
                displaced: Transition { NumberAnimation { properties: "x,y"; duration: Kirigami.Units.shortDuration } }
            }
            ColumnLayout {
                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.gridUnit + 32 + Kirigami.Units.gridUnit - Kirigami.Units.smallSpacing
                Layout.rightMargin: Kirigami.Units.gridUnit - Kirigami.Units.smallSpacing
                spacing: 0
                visible: full.controller.state === "late"
                Repeater {
                    model: full.controller.conflicts
                    StatusRow {
                        focusViewport: scroll.contentItem as Flickable
                        required property var modelData
                        Layout.fillWidth: true
                        objectName: "setting-" + modelData.setting
                        name: full.controller.settingText(modelData.setting)
                        caption: Format.settingsPageName(modelData.kcm)
                        iconName: Format.settingsPageIcon(modelData.kcm)
                        time: Format.settingAfter(modelData.seconds)
                        onClicked: {
                            KCMUtils.KCMLauncher.openSystemSettings(modelData.kcm);
                            full.controller.expanded = false;
                        }
                    }
                }
            }
            PlasmaExtras.ListSectionHeader { id: historyHeading; text: qsTr("Blocked While Away"); Layout.fillWidth: true; Layout.leftMargin: Kirigami.Units.gridUnit; Layout.rightMargin: Kirigami.Units.gridUnit }
            PC3.Label {
                textFormat: Text.PlainText
                objectName: "historyStatus"
                text: full.client.clearHistoryFailed ? qsTr("Couldn't clear history")
                    : full.client.historyFailed ? qsTr("Couldn't read history") : qsTr("None in the last 7 days")
                visible: full.client.clearHistoryFailed || full.client.historyFailed || full.controller.history.length === 0
                opacity: 0.75
                Layout.leftMargin: Kirigami.Units.gridUnit
                Layout.topMargin: Kirigami.Units.smallSpacing * 2
                Layout.bottomMargin: Kirigami.Units.smallSpacing * 2
            }
            Repeater {
                model: full.controller.history
                StatusRow {
                    required property var modelData
                    Layout.fillWidth: true
                    Layout.leftMargin: Kirigami.Units.gridUnit - Kirigami.Units.smallSpacing
                    Layout.rightMargin: Kirigami.Units.gridUnit - Kirigami.Units.smallSpacing
                    name: Format.appName(modelData)
                    caption: Format.caption(modelData)
                    iconName: modelData.iconName || "application-x-executable"
                    iconSize: 22
                    interactive: false
                    historyRow: true
                    time: Format.time(Number(modelData.start), full.controller.timeLocale, full.controller.timePattern) + "–" + Format.time(Number(modelData.end), full.controller.timeLocale, full.controller.timePattern)
                    detail: full.controller.day(Number(modelData.start)) + " · " + Format.duration(Number(modelData.end) - Number(modelData.start))
                }
            }
            PlasmaExtras.ListSectionHeader {
                text: qsTr("Sleep and Screen Locking")
                Layout.leftMargin: Kirigami.Units.gridUnit
                Layout.rightMargin: Kirigami.Units.gridUnit
                visible: full.controller.policies.length > 0
                Layout.fillWidth: true
            }
            Repeater {
                model: full.controller.policies
                RowLayout {
                    required property var modelData
                    Layout.fillWidth: true
                    Layout.margins: Kirigami.Units.smallSpacing
                    Layout.leftMargin: Kirigami.Units.gridUnit
                    Layout.rightMargin: Kirigami.Units.gridUnit
                    spacing: Kirigami.Units.smallSpacing
                    Kirigami.Icon {
                        source: parent.modelData.iconName || "application-x-executable"
                        Layout.preferredWidth: 16
                        Layout.preferredHeight: 16
                        Layout.alignment: Qt.AlignTop
                    }
                    PC3.Label {
                        textFormat: Text.PlainText
                        text: full.controller.policyText(parent.modelData)
                        font: Kirigami.Theme.smallFont
                        wrapMode: Text.WordWrap
                        maximumLineCount: 4
                        elide: Text.ElideRight
                        Layout.fillWidth: true
                    }
                }
            }
            Item { Layout.preferredHeight: Kirigami.Units.gridUnit }
        }
    }
}
