// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Layouts
import org.kde.kirigami as Kirigami
import org.kde.plasma.components as PC3

PC3.ItemDelegate {
    id: row
    property string name: ""
    property string caption: ""
    property string iconName: "application-x-executable"
    property string time: ""
    property string detail: ""
    property int iconSize: 32
    property bool interactive: true
    property bool historyRow: false
    property Flickable focusViewport: null
    function revealFocus() {
        if (!activeFocus || !focusViewport) return;
        const top = mapToItem(focusViewport.contentItem, 0, 0).y;
        const bottom = top + height;
        let position = focusViewport.contentY;
        if (top < position) position = top;
        else if (bottom > position + focusViewport.height) position = bottom - focusViewport.height;
        focusViewport.contentY = Math.max(0, Math.min(position,
            focusViewport.contentHeight - focusViewport.height));
    }
    onActiveFocusChanged: if (activeFocus) Qt.callLater(revealFocus)
    Keys.onReturnPressed: if (interactive) click()
    Keys.onEnterPressed: if (interactive) click()
    Keys.onSpacePressed: event => { event.accepted = !interactive; }
    Keys.onReleased: event => { event.accepted = !interactive && event.key === Qt.Key_Space; }
    hoverEnabled: interactive
    focusPolicy: interactive ? Qt.StrongFocus : Qt.NoFocus
    // Keep Plasma's themed fill/outline and keyboard focus styling.
    Binding { target: row.background; property: "visible"; value: row.interactive }
    Keys.onDownPressed: nextItemInFocusChain().forceActiveFocus(Qt.TabFocusReason)
    Keys.onUpPressed: nextItemInFocusChain(false).forceActiveFocus(Qt.BacktabFocusReason)
    Accessible.role: interactive ? Accessible.Button : Accessible.ListItem
    opacity: 1
    leftPadding: Kirigami.Units.smallSpacing
    rightPadding: Kirigami.Units.smallSpacing
    topPadding: Kirigami.Units.smallSpacing * 2
    bottomPadding: Kirigami.Units.smallSpacing * 2
    contentItem: RowLayout {
        spacing: Kirigami.Units.smallSpacing * 2
        Kirigami.Icon {
            source: row.iconName || "application-x-executable"
            Layout.preferredWidth: row.iconSize
            Layout.preferredHeight: row.iconSize
        }
        ColumnLayout {
            Layout.fillWidth: true
            Layout.alignment: row.caption.length || row.detail.length ? Qt.AlignTop : Qt.AlignVCenter
            spacing: 0
            PC3.Label { textFormat: Text.PlainText; id: nameLabel; objectName: "rowName"; text: row.name; elide: Text.ElideRight; Layout.fillWidth: true }
            PC3.Label { textFormat: Text.PlainText;
                id: captionLabel
                objectName: "rowCaption"
                visible: text.length > 0
                text: row.caption
                elide: Text.ElideRight
                font: Kirigami.Theme.smallFont
                opacity: 0.75
                Layout.fillWidth: true
            }
        }
        ColumnLayout {
            Layout.alignment: Qt.AlignRight | (row.caption.length || row.detail.length ? Qt.AlignTop : Qt.AlignVCenter)
            spacing: 0
            PC3.Label { textFormat: Text.PlainText;
                text: row.time
                font: row.historyRow ? Kirigami.Theme.defaultFont : Kirigami.Theme.smallFont
                opacity: row.historyRow ? 1 : 0.75
                Layout.alignment: Qt.AlignRight
                // Align the small time's baseline with the body-size app name.
                Layout.topMargin: !row.historyRow && (row.caption.length || row.detail.length)
                    ? nameLabel.baselineOffset - baselineOffset : 0
            }
            PC3.Label { textFormat: Text.PlainText; text: row.detail; visible: text.length > 0; font: Kirigami.Theme.smallFont; opacity: 0.75; Layout.alignment: Qt.AlignRight }
        }
    }
    PC3.ToolTip {
        objectName: "rowTooltip"
        textFormat: Text.PlainText
        // Use a separate hover area so static history records also expose elided text.
        visible: tooltipHover.hovered && (nameLabel.truncated || captionLabel.truncated)
        text: row.name + (row.caption ? "\n" + row.caption : "")
    }
    HoverHandler { id: tooltipHover; enabled: true }
}
