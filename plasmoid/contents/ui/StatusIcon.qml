import QtQuick
import org.kde.kirigami as Kirigami
Kirigami.Icon {
    property string state: "unknown"
    property int size: 22
    source: Qt.resolvedUrl("../icons/" + state + "-" + size + ".svg")
    implicitWidth: size
    implicitHeight: size
}
