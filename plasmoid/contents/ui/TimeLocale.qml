// SPDX-FileCopyrightText: 2026 Matthew Stanton
// SPDX-License-Identifier: GPL-3.0-or-later
pragma ComponentBehavior: Bound
import QtQuick
import org.kde.plasma.plasma5support as Plasma5Support

Item {
    id: root
    property string pattern: Qt.locale().timeFormat(Locale.ShortFormat)
    // Qt's CLDR data uses a 12-hour es_CL short time, whereas this desktop's
    // POSIX LC_TIME uses 24 hours. Honor the session's LC_TIME hour convention.
    Plasma5Support.DataSource {
        id: localeSource
        engine: "executable"
        onNewData: (source, data) => {
            disconnectSource(source);
            if (data["exit code"] !== 0) return;
            const match = String(data.stdout).match(/^t_fmt="([^"]+)"/m);
            if (!match) return;
            const shortPattern = Qt.locale().timeFormat(Locale.ShortFormat).replace(/[:.]ss/g, "");
            if (/%[HkRT]/.test(match[1])) root.pattern = shortPattern.replace(/[hH]+/, "HH").replace(/\s*[Aa][Pp]\s*/g, "").trim();
            else if (/%[Ilr]/.test(match[1])) root.pattern = shortPattern.replace(/[hH]+/, "h")
                + (/[Aa][Pp]/.test(shortPattern) ? "" : " AP");
        }
        Component.onCompleted: connectSource("locale -k LC_TIME")
    }
}
