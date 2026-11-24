import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

RowLayout {
    id: row

    required property string title
    required property string value
    property bool emphasis: false

    Layout.fillWidth: true
    spacing: 12

    Label {
        Layout.fillWidth: true
        text: row.title
        color: Theme.muted
        wrapMode: Text.WordWrap
    }

    Label {
        Layout.maximumWidth: 240
        text: row.value
        textFormat: Text.PlainText
        color: Theme.text
        horizontalAlignment: Text.AlignRight
        wrapMode: Text.Wrap
        font.pixelSize: row.emphasis ? 27 : 14
        font.weight: row.emphasis ? Font.DemiBold : Font.Medium
    }
}
