pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: detail

    required property savedAnswer entry
    signal traceRequested(string traceId)

    padding: 1
    contentWidth: availableWidth
    clip: true

    ColumnLayout {
        width: detail.availableWidth
        spacing: 8

        TextArea {
            Layout.fillWidth: true
            objectName: "savedAnswerQuestion"
            padding: 0
            readOnly: true
            text: detail.entry.question
            textFormat: TextEdit.PlainText
            wrapMode: TextEdit.Wrap
            font.pixelSize: 16
            font.weight: Font.DemiBold
            background: null
        }
        Label {
            Layout.fillWidth: true
            text: detail.entry.completedAt
            color: Theme.muted
        }
        Repeater {
            model: detail.entry.scope

            delegate: Label {
                required property string modelData
                Layout.fillWidth: true
                text: modelData
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.muted
            }
        }
        AnswerResultPanel {
            Layout.fillWidth: true
            Layout.topMargin: 8
            result: detail.entry.result
            traceId: detail.entry.traceId
            onTraceRequested: function (traceId) {
                detail.traceRequested(traceId);
            }
        }
    }
}
