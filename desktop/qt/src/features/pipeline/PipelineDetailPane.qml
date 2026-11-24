pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: pane

    required property PipelineInspector pipeline
    required property string question
    required property int selectedCase
    property int stage: 2
    readonly property bool wide: width >= 740
    readonly property list<string> stageLabels: [qsTr("Corpus"), qsTr("Search Index"), qsTr("Retrieval"), qsTr("Embeddings"), qsTr("Ranking"), qsTr("Model Input"), qsTr("Model Output"), qsTr("Citation Verification")]
    signal questionEdited(string text)

    spacing: 10

    RowLayout {
        Layout.fillWidth: true

        SurfaceInput {
            id: query
            Layout.fillWidth: true
            objectName: "pipelineQuestion"
            text: pane.question
            placeholderText: qsTr("Question about the local corpus")
            enabled: !pane.pipeline.busy
            onTextEdited: pane.questionEdited(text)
            onAccepted: {
                if (run.enabled)
                    pane.pipeline.ask(text, pane.selectedCase); // qmllint disable compiler
            }
        }
        ActionButton {
            id: run
            objectName: "runPipelineQuestion"
            text: qsTr("Run")
            highlighted: true
            enabled: query.text.trim().length > 0 && !pane.pipeline.busy && !pane.pipeline.sources.busy && (pane.pipeline.selectedSuite === -1 || pane.pipeline.sourceReady)
            onClicked: pane.pipeline.ask(query.text, pane.selectedCase) // qmllint disable compiler
        }
    }

    RowLayout {
        Layout.fillWidth: true
        Layout.fillHeight: true

        ColumnLayout {
            visible: pane.wide
            Layout.alignment: Qt.AlignTop
            Layout.preferredWidth: 188
            Layout.maximumWidth: 188
            spacing: 3

            Repeater {
                model: pane.stageLabels
                delegate: ItemDelegate {
                    id: step
                    required property int index
                    required property string modelData
                    Layout.fillWidth: true
                    objectName: "pipelineStage-" + index
                    text: modelData
                    highlighted: pane.stage === index
                    implicitHeight: 34
                    onClicked: pane.stage = index

                    contentItem: Label {
                        text: step.text
                        font.weight: step.highlighted ? Font.DemiBold : Font.Medium
                        color: Theme.text
                        verticalAlignment: Text.AlignVCenter
                        elide: Text.ElideRight
                    }

                    background: Rectangle {
                        radius: Theme.controlRadius
                        color: step.highlighted ? Theme.selection : step.hovered ? Theme.hovered : "transparent"
                        border.width: step.visualFocus ? 2 : 0
                        border.color: Theme.accent
                    }
                }
            }
        }

        Rectangle {
            color: Theme.outline
            visible: pane.wide
            Layout.fillHeight: true
            Layout.preferredWidth: 1
        }

        ColumnLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            spacing: 8

            RowLayout {
                Layout.fillWidth: true
                Layout.minimumHeight: 36

                LocalizedComboBox {
                    visible: !pane.wide
                    Layout.fillWidth: true
                    objectName: "pipelineStage"
                    labels: pane.stageLabels
                    selectedIndex: pane.stage
                    Accessible.name: qsTr("Inspect Pipeline")
                    onActivated: pane.stage = currentIndex
                }
                Label {
                    visible: pane.wide
                    Layout.fillWidth: true
                    text: pane.stageLabels[pane.stage]
                    font.weight: Font.DemiBold
                    font.pixelSize: 16
                    Accessible.role: Accessible.Heading
                }
                Label {
                    visible: pane.wide
                    Layout.maximumWidth: 248
                    opacity: pane.pipeline.busy ? 0 : 1
                    text: pane.pipeline.traceId
                    textFormat: Text.PlainText
                    elide: Text.ElideMiddle
                    font.family: Theme.fixedFont
                    color: Theme.muted
                }
            }

            Item {
                Layout.fillWidth: true
                Layout.fillHeight: true
                objectName: "pipelineStageFrame"

                ScrollView {
                    id: panelScroll

                    anchors.fill: parent
                    visible: pane.pipeline.result.present && !pane.pipeline.busy
                    contentWidth: availableWidth
                    clip: true

                    ColumnLayout {
                        width: panelScroll.availableWidth
                        spacing: 12

                        Label {
                            Layout.fillWidth: true
                            visible: pane.pipeline.inspectedQuestion !== pane.question && pane.pipeline.inspectedQuestion !== ""
                            text: pane.pipeline.inspectedQuestion
                            textFormat: Text.PlainText
                            font.weight: Font.DemiBold
                            wrapMode: Text.Wrap
                        }
                        TraceEvidencePanel {
                            Layout.fillWidth: true
                            visible: pane.stage === 2 && pane.pipeline.details.hasReview
                            details: pane.pipeline.details
                        }
                        TraceModelSources {
                            Layout.fillWidth: true
                            visible: pane.stage === 5
                            sources: pane.pipeline.details.sources
                            collapsible: true
                            disclosureKey: pane.pipeline.traceId
                        }
                        SurfaceGroup {
                            id: prompt

                            Layout.fillWidth: true
                            visible: pane.stage === 5 && pane.pipeline.completionInput !== ""
                            contentItem: ColumnLayout {
                                RowLayout {
                                    Layout.fillWidth: true
                                    ItemDelegate {
                                        id: disclosure
                                        Layout.fillWidth: true
                                        objectName: "pipelinePromptDisclosure"
                                        text: (checked ? "▾  " : "▸  ") + qsTr("Exact completion input")
                                        Accessible.name: qsTr("Exact completion input")
                                        checkable: true
                                        Connections {
                                            target: pane.pipeline
                                            function onChanged() {
                                                if (pane.pipeline.busy)
                                                    disclosure.checked = false;
                                            }
                                        }
                                    }
                                    ActionButton {
                                        text: qsTr("Copy prompt")
                                        onClicked: pane.pipeline.copyCompletionInput() // qmllint disable compiler
                                    }
                                }
                                TextArea {
                                    Layout.fillWidth: true
                                    visible: disclosure.checked
                                    objectName: "pipelineExactPrompt"
                                    text: pane.pipeline.completionInput
                                    readOnly: true
                                    selectByMouse: true
                                    wrapMode: TextEdit.Wrap
                                    textFormat: TextEdit.PlainText
                                    font.family: Theme.fixedFont
                                }
                            }
                        }
                        AnswerResultPanel {
                            Layout.fillWidth: true
                            visible: pane.stage === 7
                            result: pane.pipeline.result
                            traceId: ""
                            citationsTitle: qsTr("Verified Citations")
                        }
                        TextArea {
                            Layout.fillWidth: true
                            objectName: "pipelineStageContents"
                            visible: pane.stage !== 5 && !(pane.stage === 2 && pane.pipeline.details.hasReview)
                            readOnly: true
                            selectByMouse: true
                            wrapMode: TextEdit.Wrap
                            textFormat: TextEdit.PlainText
                            font.family: Theme.fixedFont
                            text: {
                                if (!pane.pipeline.result.present)
                                    return "";
                                return pane.pipeline.stageText(pane.stage); // qmllint disable compiler
                            }
                            placeholderText: qsTr("No data recorded for this stage.")
                        }
                    }
                }

                ColumnLayout {
                    anchors.centerIn: parent
                    width: Math.min(parent.width, 340)
                    visible: !panelScroll.visible
                    spacing: 12

                    BusyIndicator {
                        Layout.alignment: Qt.AlignHCenter
                        Layout.preferredWidth: 28
                        Layout.preferredHeight: 28
                        palette.dark: Theme.accent
                        running: pane.pipeline.busy || pane.pipeline.sources.loadingOverview
                        visible: running
                        objectName: "pipelineCenteredLoader"
                    }
                    Label {
                        Layout.fillWidth: true
                        text: pane.pipeline.busy ? (pane.pipeline.indexTotal > 0 ? qsTr("Preparing benchmark embeddings…") : qsTr("Running local Answers…")) : pane.pipeline.sources.loadingOverview ? qsTr("Loading sources…") : pane.pipeline.feedback || qsTr("Run a question")
                        color: Theme.muted
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        textFormat: Text.PlainText
                    }
                }
            }
        }
    }
}
