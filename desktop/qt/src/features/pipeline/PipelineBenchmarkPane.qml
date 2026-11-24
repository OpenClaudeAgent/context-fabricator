pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: pane

    required property PipelineInspector pipeline
    required property int rowIndex
    readonly property int currentRow: rowIndex >= 0 && rowIndex < pipeline.rows.length ? rowIndex : 0
    property benchmarkResultRow emptyRow
    readonly property benchmarkResultRow selectedRow: pipeline.rows[currentRow] ?? emptyRow
    readonly property bool compact: width < 640
    signal rowPicked(int index)
    signal sourcesRequested
    signal inspectRequested(int index, string question)

    spacing: 8

    RowLayout {
        Layout.fillWidth: true

        Label {
            Layout.fillWidth: true
            text: qsTr("Benchmark Questions") + " · " + pane.pipeline.rows.length
            visible: !pane.compact
            elide: Text.ElideRight
            font.weight: Font.DemiBold
            color: Theme.text
        }
        Item {
            visible: pane.compact
            Layout.fillWidth: true
        }
        ActionButton {
            text: qsTr("Run Question")
            objectName: "runBenchmarkQuestion"
            highlighted: true
            enabled: pane.selectedRow.query !== "" && pane.pipeline.sourceReady && !pane.pipeline.busy
            onClicked: pane.pipeline.runQuestion(pane.currentRow) // qmllint disable compiler
        }
        ActionButton {
            text: qsTr("Run All")
            objectName: "runBenchmarkSuite"
            enabled: pane.pipeline.sourceReady && !pane.pipeline.busy && pane.pipeline.rows.length > 0
            onClicked: pane.pipeline.runAll() // qmllint disable compiler
        }
    }

    RowLayout {
        Layout.fillWidth: true
        Layout.minimumHeight: 36
        Label {
            visible: pane.pipeline.sourceReady
            Layout.fillWidth: true
            text: "✓  " + qsTr("Ready")
            color: Theme.statusReady
            Accessible.name: qsTr("Ready")
        }

        Label {
            Layout.fillWidth: true
            visible: !pane.pipeline.sourceReady
            text: pane.pipeline.sources.pendingCorpus !== "" ? qsTr("Loading…") : qsTr("Activate the source for this question set.")
            color: Theme.muted
            wrapMode: Text.Wrap
        }
        ActionButton {
            visible: !pane.pipeline.sourceReady
            text: pane.pipeline.selectedCorpus === "" ? qsTr("Sources…") : pane.pipeline.selectedCorpusSaved ? qsTr("Activate") : qsTr("Download Corpus")
            enabled: !pane.pipeline.sources.busy
            // qmllint disable compiler
            onClicked: {
                if (pane.pipeline.selectedCorpus === "")
                    pane.sourcesRequested();
                else
                    pane.pipeline.sources.setActive(pane.pipeline.selectedCorpus, true);
            }
            // qmllint enable compiler
        }
    }

    GridLayout {
        Layout.fillWidth: true
        Layout.fillHeight: true
        columns: pane.compact ? 1 : 3
        columnSpacing: 10
        rowSpacing: 8

        ListView {
            id: cases

            objectName: "pipelineBenchmarkList"
            Layout.fillHeight: !pane.compact
            Layout.fillWidth: pane.compact
            Layout.preferredHeight: pane.compact ? 128 : -1
            Layout.preferredWidth: pane.width * 0.48
            Layout.minimumWidth: 140
            model: pane.pipeline.rows
            clip: true
            spacing: 4
            ScrollBar.vertical: ScrollBar {}

            delegate: ItemDelegate {
                id: entry

                required property benchmarkResultRow modelData
                required property int index
                objectName: "benchmarkQuestion-" + index
                width: cases.width
                highlighted: index === pane.currentRow
                Accessible.name: modelData.query
                onClicked: pane.rowPicked(index)
                verticalPadding: 7
                contentItem: ColumnLayout {
                    spacing: 3
                    Label {
                        Layout.fillWidth: true
                        text: entry.modelData.query
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        maximumLineCount: 2
                        elide: Text.ElideRight
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 18
                        Label {
                            Layout.fillWidth: true
                            Layout.preferredHeight: 16
                            text: entry.modelData.source
                            textFormat: Text.PlainText
                            color: Theme.muted
                            elide: Text.ElideRight
                        }
                        Label {
                            id: outcome
                            objectName: "benchmarkResultStatus-" + entry.index
                            Layout.preferredHeight: 16
                            Layout.alignment: Qt.AlignRight
                            Layout.maximumWidth: entry.width * 0.56
                            elide: Text.ElideRight
                            opacity: entry.modelData.complete ? 1 : 0
                            Accessible.ignored: !entry.modelData.complete
                            color: entry.modelData.error !== "" ? Theme.danger : entry.modelData.outcome === "answered" ? Theme.statusReady : Theme.statusWaiting
                            Accessible.name: text
                            text: entry.modelData.error !== "" ? qsTr("Error") : entry.modelData.outcome === "answered" ? qsTr("Published") : qsTr("Insufficient evidence")
                            leftPadding: 17
                            Label {
                                anchors.left: parent.left
                                anchors.verticalCenter: parent.verticalCenter
                                text: entry.modelData.error !== "" ? "×" : entry.modelData.outcome === "answered" ? "✓" : "!"
                                color: outcome.color
                                font.bold: true
                                Accessible.ignored: true
                            }
                        }
                    }
                }
            }
        }

        Rectangle {
            visible: !pane.compact
            Layout.fillHeight: true
            Layout.preferredWidth: 1
            color: Theme.outline
        }

        ScrollView {
            id: panelScroll

            Layout.fillHeight: true
            Layout.fillWidth: true
            clip: true
            contentWidth: availableWidth

            ColumnLayout {
                width: panelScroll.availableWidth
                spacing: 10

                Label {
                    Layout.fillWidth: true
                    text: pane.selectedRow.query || qsTr("Select a question")
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.weight: Font.DemiBold
                }
                RowLayout {
                    Layout.fillWidth: true
                    visible: pane.selectedRow.reference.toString() !== "" || pane.selectedRow.source !== ""

                    Label {
                        Layout.fillWidth: true
                        text: pane.selectedRow.source
                        textFormat: Text.PlainText
                        color: Theme.muted
                        elide: Text.ElideMiddle
                    }
                    ActionButton {
                        visible: pane.selectedRow.reference.toString() !== ""
                        text: qsTr("Reference source")
                        Accessible.role: Accessible.Link
                        onClicked: Qt.openUrlExternally(pane.selectedRow.reference)
                    }
                }
                SurfaceGroup {
                    Layout.fillWidth: true
                    visible: !!pane.selectedRow.expected
                    title: qsTr("Expected Answer")
                    contentItem: Label {
                        text: pane.selectedRow.expected ?? ""
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                }
                SurfaceGroup {
                    Layout.fillWidth: true
                    visible: !!pane.selectedRow.complete
                    title: pane.selectedRow.error ? qsTr("Error") : qsTr("Model Answer")
                    contentItem: Label {
                        text: pane.selectedRow.error || pane.selectedRow.answer || qsTr("Insufficient evidence")
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                }
                ActionButton {
                    Layout.alignment: Qt.AlignRight
                    text: qsTr("Inspect Result")
                    objectName: "inspectBenchmarkResult"
                    visible: !!pane.selectedRow.complete && !pane.selectedRow.error
                    enabled: !pane.pipeline.busy
                    onClicked: pane.inspectRequested(pane.currentRow, pane.selectedRow.query)
                }
            }
        }
    }
}
