pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: page

    required property Answers answers
    required property AnswerHistory history
    readonly property int gutter: width < 580 ? 16 : Theme.margin
    signal traceRequested(string traceId)
    signal historySettingsRequested
    signal modelLibraryRequested
    signal inferenceSetupRequested
    signal pipelineRequested

    padding: gutter
    contentWidth: availableWidth
    clip: true
    onVisibleChanged: {
        if (visible)
            answers.refreshModels(); // qmllint disable compiler
    }

    ColumnLayout {
        x: (page.availableWidth - width) / 2
        width: Math.min(Theme.workspaceWidth, page.availableWidth)
        spacing: Theme.gap

        RowLayout {
            Layout.fillWidth: true

            PageTitle {
                Layout.fillWidth: true
                text: qsTr("Answers")
            }
            ActionButton {
                text: qsTr("Inspect Pipeline")
                objectName: "openPipelineInspector"
                enabled: page.answers.ready && !page.answers.busy
                onClicked: page.pipelineRequested()
            }
            ActionButton {
                text: qsTr("History…")
                objectName: "answerHistory"
                enabled: page.answers.ready
                onClicked: historyDialog.open()
            }
            ActionButton {
                text: qsTr("Refresh")
                enabled: page.answers.ready && !page.answers.modelsBusy
                onClicked: page.answers.refreshModels() // qmllint disable compiler
            }
        }

        SurfaceGroup {
            Layout.fillWidth: true
            visible: page.answers.ready && !page.answers.modelsBusy && page.answers.activeModelIndex < 0 && page.answers.modelFeedback === ""
            contentItem: RowLayout {
                spacing: 8

                Label {
                    Layout.fillWidth: true
                    text: qsTr("Set up a local model to start asking questions.")
                    wrapMode: Text.Wrap
                }
                ActionButton {
                    text: qsTr("Choose a Model…")
                    highlighted: true
                    objectName: "setUpAnswers"
                    onClicked: page.inferenceSetupRequested()
                }
            }
        }

        AnswerComposer {
            Layout.fillWidth: true
            answers: page.answers
            onModelLibraryRequested: page.modelLibraryRequested()
        }

        RowLayout {
            Layout.fillWidth: true
            visible: page.answers.feedback !== ""
            spacing: 8

            Label {
                Layout.fillWidth: true
                objectName: "answerFeedback"
                text: page.answers.feedback
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.muted
            }
            ActionButton {
                visible: page.answers.traceId !== "" && !page.answers.busy
                text: qsTr("View Trace")
                onClicked: page.traceRequested(page.answers.traceId)
            }
        }

        AnswerResultPanel {
            Layout.fillWidth: true
            visible: page.answers.result.present
            result: page.answers.result
            traceId: page.answers.traceId
            onTraceRequested: function (id) {
                page.traceRequested(id);
            }
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.topMargin: Theme.gap
            visible: page.answers.activeModelIndex >= 0 && !page.answers.busy && !page.answers.result.present && page.answers.feedback === ""
            spacing: 6

            Label {
                Layout.fillWidth: true
                text: qsTr("Ask about your corpus")
                horizontalAlignment: Text.AlignHCenter
                color: Theme.text
                font.weight: Font.DemiBold
                Accessible.role: Accessible.Heading
            }
            Label {
                Layout.fillWidth: true
                text: qsTr("Answers appear here with citations from your local sources.")
                horizontalAlignment: Text.AlignHCenter
                color: Theme.muted
                wrapMode: Text.Wrap
            }
        }
    }

    AnswerHistoryDialog {
        id: historyDialog

        history: page.history
        answers: page.answers
        onHistorySettingsRequested: page.historySettingsRequested()
        onTraceRequested: function (traceId) {
            page.traceRequested(traceId);
        }
    }
}
