pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: row

    required property traceRecord record
    property bool showSpanId: false
    objectName: "trace-log-" + record.name

    ColumnLayout {
        width: parent.width
        spacing: 4

        Label {
            Layout.fillWidth: true
            text: row.record.title
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            text: "%1 · %2".arg(row.record.level).arg(row.record.timestamp)
            textFormat: Text.PlainText
            wrapMode: Text.WordWrap
            color: Theme.muted
        }
        Label {
            Layout.fillWidth: true
            visible: row.showSpanId && row.record.spanId.length > 0
            text: row.record.spanId
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.muted
            Accessible.name: qsTr("Span ID") + " " + text
        }
    }
}
