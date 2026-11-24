pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: excerpt

    required property traceAnswerExcerpt record
    spacing: 4

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: excerpt.record.title
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        font.weight: Font.DemiBold
    }
    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: excerpt.record.location
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: Theme.muted
    }
    TextArea {
        objectName: "traceExcerptText"
        Layout.fillWidth: true
        visible: text !== ""
        text: excerpt.record.text
        textFormat: TextEdit.PlainText
        wrapMode: TextEdit.Wrap
        readOnly: true
        padding: 8
        font.family: Theme.fixedFont
        background: Rectangle {
            color: Theme.raised
            radius: Theme.controlRadius
        }
    }
    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: excerpt.record.status
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
        color: Theme.muted
    }
}
