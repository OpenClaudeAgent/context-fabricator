import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

RowLayout {
    id: row

    required property string label
    required property string value
    required property string fieldName
    property string unit
    signal edited(value: string)
    signal committed(value: string)

    Layout.fillWidth: true

    Label {
        Layout.fillWidth: true
        wrapMode: Text.WordWrap
        text: row.label
    }

    SurfaceInput {
        objectName: row.fieldName
        text: row.value
        Layout.preferredWidth: 76
        horizontalAlignment: TextInput.AlignRight
        inputMethodHints: Qt.ImhFormattedNumbersOnly
        Accessible.name: row.label
        onTextEdited: row.edited(text)
        onEditingFinished: row.committed(text)
    }

    Label {
        visible: row.unit !== ""
        Layout.preferredWidth: 38
        text: row.unit
    }
}
