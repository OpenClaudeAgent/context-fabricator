pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property PipelineInspector pipeline
    property int page: 0
    property int questionRow: -1
    property string question: ""

    objectName: "pipelineInspector"
    title: qsTr("Inspect Pipeline")
    width: Math.min(1060, parent.width - 24)
    height: Math.min(740, parent.height - 24)
    x: (parent.width - width) / 2
    y: (parent.height - height) / 2
    modal: true
    closePolicy: pipeline.busy || pipeline.sources.busy ? Popup.NoAutoClose : Popup.CloseOnEscape

    function openFor(text: string) {
        question = text;
        page = 0;
        pipeline.open(); // qmllint disable compiler
        open();
    }

    function showBenchmarks() {
        if (pipeline.selectedSuite < 0)
            questionRow = -1;
        pipeline.selectDefaultBenchmark(); // qmllint disable compiler
        page = 1;
    }

    contentItem: ColumnLayout {
        spacing: 10

        RowLayout {
            Layout.fillWidth: true
            spacing: 6

            SegmentTab {
                text: qsTr("Inspect")
                checked: dialog.page === 0
                enabled: !dialog.pipeline.busy
                objectName: "pipelineInspectTab"
                onClicked: dialog.page = 0
            }
            SegmentTab {
                text: qsTr("Benchmarks")
                checked: dialog.page === 1
                enabled: !dialog.pipeline.busy
                objectName: "pipelineBenchmarkTab"
                onClicked: dialog.showBenchmarks()
            }
            SegmentTab {
                text: qsTr("Sources")
                checked: dialog.page === 2
                enabled: !dialog.pipeline.busy
                objectName: "pipelineSourcesTab"
                onClicked: dialog.page = 2
            }
            Item {
                Layout.fillWidth: true
            }
            ActionButton {
                text: qsTr("Done")
                objectName: "closePipelineInspector"
                enabled: !dialog.pipeline.busy && !dialog.pipeline.sources.busy
                onClicked: dialog.close()
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: dialog.page !== 2

            Label {
                text: dialog.page === 1 ? qsTr("Question Set") : qsTr("Search in")
                Layout.maximumWidth: dialog.width < 480 ? 98 : 170
                elide: Text.ElideRight
                color: Theme.muted
            }
            LocalizedComboBox {
                id: scope

                objectName: "pipelineQuestionSet"
                Layout.fillWidth: true
                Layout.minimumWidth: 95
                labels: [qsTr("All sources")].concat(dialog.pipeline.suites)
                selectedIndex: dialog.pipeline.selectedSuite + 1
                enabled: !dialog.pipeline.busy && !dialog.pipeline.sources.busy
                onActivated: {
                    dialog.questionRow = -1;
                    dialog.pipeline.selectSuite(currentIndex - 1); // qmllint disable compiler
                }
            }
            LocalizedComboBox {
                objectName: "pipelineRetrievalMode"
                Layout.minimumWidth: 88
                Layout.preferredWidth: dialog.width < 480 ? 96 : 140
                visible: dialog.pipeline.hybridAvailable
                labels: [qsTr("Hybrid"), qsTr("BM25")]
                selectedIndex: dialog.pipeline.hybrid ? 0 : 1
                enabled: !dialog.pipeline.busy
                Accessible.name: qsTr("Search mode")
                onActivated: dialog.pipeline.hybrid = currentIndex === 0
            }
        }

        StackLayout {
            Layout.minimumWidth: 0
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: dialog.page

            PipelineDetailPane {
                Layout.minimumWidth: 0
                pipeline: dialog.pipeline
                question: dialog.question
                selectedCase: dialog.questionRow
                onQuestionEdited: text => dialog.question = text
            }
            PipelineBenchmarkPane {
                Layout.minimumWidth: 0
                pipeline: dialog.pipeline
                rowIndex: dialog.questionRow
                onRowPicked: index => dialog.questionRow = index
                onSourcesRequested: dialog.page = 2
                onInspectRequested: function (index, text) {
                    dialog.questionRow = index;
                    dialog.question = text;
                    dialog.pipeline.inspectResult(index); // qmllint disable compiler
                    dialog.page = 0;
                }
            }
            PipelineSourcePane {
                Layout.minimumWidth: 0
                sources: dialog.pipeline.sources
                onImportFilesRequested: inputFiles.open()
                onImportSuiteRequested: suiteFile.open()
            }
        }

        RowLayout {
            Layout.fillWidth: true
            Layout.minimumHeight: 34

            Label {
                id: feedback
                Layout.fillWidth: true
                objectName: "pipelineFeedback"
                Layout.minimumWidth: 0
                text: dialog.page === 2 ? dialog.pipeline.sources.feedback : dialog.pipeline.feedback
                textFormat: Text.PlainText
                color: Theme.muted
                elide: Text.ElideRight
                Accessible.description: text
                ToolTip.visible: feedbackHover.hovered && feedback.truncated
                ToolTip.text: text
                HoverHandler {
                    id: feedbackHover
                }
            }
            ActionButton {
                visible: dialog.page !== 2 && dialog.pipeline.hybridProblem && dialog.pipeline.hybrid
                text: qsTr("Use BM25")
                onClicked: dialog.pipeline.hybrid = false
            }
            ProgressBar {
                visible: dialog.pipeline.busy && (dialog.page === 1 || dialog.pipeline.indexTotal > 0)
                Layout.preferredWidth: 130
                from: 0
                to: dialog.pipeline.indexTotal > 0 ? dialog.pipeline.indexTotal : Math.max(1, dialog.pipeline.totalCount)
                value: dialog.pipeline.indexTotal > 0 ? dialog.pipeline.indexed : dialog.pipeline.finishedCount
                Accessible.name: dialog.pipeline.indexTotal > 0 ? qsTr("Preparing benchmark embeddings…") : qsTr("Running benchmark…")
            }
            ActionButton {
                visible: dialog.pipeline.busy
                text: qsTr("Stop")
                objectName: "stopPipelineRequest"
                enabled: !dialog.pipeline.stopping
                onClicked: dialog.pipeline.stop() // qmllint disable compiler
            }
        }
    }

    FileDialog {
        id: inputFiles
        title: qsTr("Import Corpus")
        fileMode: FileDialog.OpenFiles
        onAccepted: dialog.pipeline.importFiles(selectedFiles) // qmllint disable compiler
    }
    FileDialog {
        id: suiteFile
        title: qsTr("Import Benchmark…")
        nameFilters: ["JSON (*.json)"]
        onAccepted: dialog.pipeline.importSuite(selectedFile) // qmllint disable compiler
    }
}
