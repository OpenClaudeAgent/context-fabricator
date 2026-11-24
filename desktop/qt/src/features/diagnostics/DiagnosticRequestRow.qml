pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

RowLayout {
    id: row

    required property traceRecord record
    required property string question
    readonly property string description: question || record.traceId
    signal traceRequested(string traceId)

    spacing: 10

    GridLayout {
        Layout.fillWidth: true
        columns: row.width < 525 ? 1 : 3
        columnSpacing: 14
        rowSpacing: 4

        Label {
            text: row.record.errorLevel ? qsTr("Failed") : qsTr("Completed")
            color: row.record.errorLevel ? Theme.danger : Theme.statusReady
            font.weight: Font.DemiBold
        }
        Label {
            id: dateLabel

            text: row.record.observedAt
            color: Theme.muted
            font.pixelSize: 12
        }
        Label {
            Layout.fillWidth: true
            text: row.question || row.record.traceId.slice(0, 8)
            textFormat: Text.PlainText
            elide: Text.ElideRight
            color: Theme.muted
            font.family: row.question === "" ? Theme.fixedFont : dateLabel.font.family
            font.pixelSize: 12
            Accessible.name: row.description
            ToolTip.visible: hovered.hovered
            ToolTip.delay: 650
            ToolTip.text: row.description

            HoverHandler {
                id: hovered
            }
        }
    }
    ActionButton {
        objectName: "viewAnswerRequestTrace"
        text: qsTr("View Trace")
        Accessible.name: qsTr("View Trace") + " " + row.record.traceId
        onClicked: row.traceRequested(row.record.traceId)
    }
}
