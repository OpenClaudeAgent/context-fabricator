pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: pane

    required property TraceAnswer answer
    required property TraceInspector inspector
    readonly property bool withheld: answer.result.abstained || (!answer.result.present && inspector.recordedAbstention)
    readonly property bool unavailable: !answer.result.present && !withheld && answer.question === ""
    signal spansRequested

    contentWidth: availableWidth
    clip: true

    ColumnLayout {
        width: pane.availableWidth
        spacing: Theme.gap

        ColumnLayout {
            Layout.fillWidth: true
            visible: pane.withheld && !pane.answer.loading
            spacing: 4

            Label {
                Layout.fillWidth: true
                text: qsTr("Insufficient evidence")
                color: Theme.text
                font.pixelSize: 16
                font.weight: Font.DemiBold
                Accessible.role: Accessible.Heading
            }
            Label {
                Layout.fillWidth: true
                text: pane.answer.details.hasReview ? pane.answer.result.text : pane.inspector.abstentionDetail
                color: Theme.muted
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: pane.answer.question !== ""
            spacing: 4

            Label {
                Layout.fillWidth: true
                text: qsTr("Question")
                font.weight: Font.DemiBold
                Accessible.role: Accessible.Heading
            }
            TextArea {
                Layout.fillWidth: true
                text: pane.answer.question
                textFormat: TextEdit.PlainText
                wrapMode: TextEdit.Wrap
                readOnly: true
                padding: 0
                background: null
                objectName: "traceAnswerQuestion"
            }
        }

        AnswerResultPanel {
            Layout.fillWidth: true
            visible: pane.answer.result.present && !pane.withheld
            result: pane.answer.result
            traceId: ""
            citationsTitle: pane.answer.live ? qsTr("Verified Citations") : qsTr("Sources")
        }

        TraceModelSources {
            Layout.fillWidth: true
            visible: pane.answer.live && !pane.withheld
            sources: pane.answer.details.sources
        }

        TraceEvidencePanel {
            Layout.fillWidth: true
            visible: pane.withheld && pane.answer.details.hasReview
            details: pane.answer.details
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: pane.withheld && !pane.answer.details.hasReview && !pane.answer.loading
            spacing: 8

            Label {
                Layout.fillWidth: true
                visible: pane.answer.question === ""
                text: qsTr("The question was not retained for this trace.")
                color: Theme.muted
                wrapMode: Text.Wrap
            }
            TraceDetailsGroup {
                Layout.fillWidth: true
                title: qsTr("Recorded retrieval")
                fields: pane.inspector.recordedRetrieval
            }
            Label {
                Layout.fillWidth: true
                text: qsTr("Candidate details were not retained for this trace.")
                color: Theme.muted
                wrapMode: Text.Wrap
            }
        }

        Label {
            Layout.fillWidth: true
            visible: pane.answer.loading
            text: qsTr("Loading saved answer…")
            color: Theme.muted
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: !pane.answer.loading && !pane.answer.result.present && !pane.withheld && pane.answer.question !== ""
            text: qsTr("The question is still in this session. The answer is no longer available.")
            color: Theme.muted
            wrapMode: Text.Wrap
        }
        ActionButton {
            visible: !pane.answer.loading && (pane.withheld || (!pane.answer.result.present && !pane.unavailable))
            text: qsTr("Show recorded steps")
            onClicked: pane.spansRequested()
        }

        ColumnLayout {
            Layout.fillWidth: true
            visible: pane.unavailable && !pane.answer.loading
            spacing: 8

            Label {
                Layout.fillWidth: true
                text: qsTr("Saved Answer Unavailable")
                color: Theme.text
                font.weight: Font.DemiBold
                Accessible.role: Accessible.Heading
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                text: pane.answer.feedback || qsTr("No saved answer is available for this trace. It may have expired or been removed.")
                textFormat: Text.PlainText
                color: Theme.muted
                wrapMode: Text.Wrap
            }
            ActionButton {
                text: qsTr("Retry")
                onClicked: pane.answer.reload() // qmllint disable compiler
            }
        }
    }
}
