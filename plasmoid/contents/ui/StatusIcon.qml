// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
import QtQuick
import org.kde.kirigami as Kirigami
Kirigami.Icon {
    property string state: "unknown"
    property int size: 22
    source: Qt.resolvedUrl("../icons/" + state + "-" + size + ".svg")
    implicitWidth: size
    implicitHeight: size
}
