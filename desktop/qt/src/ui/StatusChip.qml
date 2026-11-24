import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Control {
    id: chip

    required property string text
    required property bool ready
    readonly property color indicator: ready ? Theme.statusReady : Theme.statusWaiting

    padding: 7
    horizontalPadding: 10
    implicitWidth: implicitContentWidth + leftPadding + rightPadding
    implicitHeight: implicitContentHeight + topPadding + bottomPadding
    Accessible.role: Accessible.StaticText
    Accessible.name: text

    background: Rectangle {
        color: Theme.field
        border.color: Theme.outline
        radius: height / 2
    }

    contentItem: RowLayout {
        spacing: 7

        Rectangle {
            Layout.preferredWidth: 7
            Layout.preferredHeight: 7
            radius: width / 2
            color: chip.indicator
        }
        Label {
            Layout.fillWidth: true
            text: chip.text
            color: chip.indicator
            textFormat: Text.PlainText
            elide: Text.ElideRight
            font.pixelSize: 12
            font.weight: Font.DemiBold
        }
    }
}
