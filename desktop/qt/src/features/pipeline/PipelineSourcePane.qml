pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: pane

    required property BenchmarkSources sources
    signal importFilesRequested
    signal importSuiteRequested
    property bool showActions: false
    property bool showConnected: false
    readonly property bool loadingList: sources.loadingOverview || (sources.busy && sources.pendingCorpus === "")
    readonly property bool compact: width < 760

    spacing: 8

    GridLayout {
        Layout.minimumWidth: 0
        Layout.fillWidth: true
        columns: pane.compact ? 1 : 2
        columnSpacing: 10
        rowSpacing: 6

        Flow {
            Layout.minimumWidth: 0
            Layout.fillWidth: true
            spacing: 4

            SegmentTab {
                text: qsTr("Benchmark Corpora")
                checked: !pane.showActions
                onClicked: pane.showActions = false
            }
            SegmentTab {
                objectName: "pipelineActionsSourceTab"
                text: qsTr("GitHub Actions")
                checked: pane.showActions && !pane.showConnected
                onClicked: {
                    pane.showActions = true;
                    pane.showConnected = false;
                }
            }
            SegmentTab {
                objectName: "pipelineConnectedSourcesTab"
                text: qsTr("Connected Repositories")
                checked: pane.showActions && pane.showConnected
                onClicked: {
                    pane.showActions = true;
                    pane.showConnected = true;
                }
            }
        }
        Flow {
            Layout.minimumWidth: 0
            Layout.fillWidth: true
            Layout.preferredWidth: pane.compact ? 0 : 470
            layoutDirection: Qt.RightToLeft
            spacing: 5

            ActionButton {
                visible: !pane.showActions
                text: qsTr("Download & Activate All")
                enabled: pane.sources.canActivateCorpora
                onClicked: pane.sources.activateAllCorpora() // qmllint disable compiler
            }
            ActionButton {
                visible: !pane.showActions
                text: qsTr("Import Corpus…")
                enabled: !pane.sources.busy
                onClicked: pane.importFilesRequested()
            }
            ActionButton {
                visible: !pane.showActions
                text: qsTr("Import Benchmark…")
                enabled: !pane.sources.busy
                onClicked: pane.importSuiteRequested()
            }
            ActionButton {
                visible: pane.showActions && !pane.showConnected
                text: qsTr("Download All Actions")
                objectName: "pipelineDownloadAllActions"
                enabled: !pane.sources.busy
                onClicked: pane.sources.downloadAllActions() // qmllint disable compiler
            }
            ActionButton {
                visible: pane.showActions && pane.showConnected
                text: qsTr("Refresh")
                enabled: !pane.sources.busy
                onClicked: pane.sources.loadConnectedSources() // qmllint disable compiler
            }
        }
    }

    Item {
        Layout.fillWidth: true
        Layout.fillHeight: true
        objectName: "pipelineSourcesFrame"

        ScrollView {
            id: panelScroll

            anchors.fill: parent
            visible: !pane.loadingList
            contentWidth: availableWidth
            clip: true

            ColumnLayout {
                width: panelScroll.availableWidth
                spacing: 10

                SurfaceGroup {
                    Layout.fillWidth: true
                    visible: !pane.showActions || !pane.showConnected
                    title: pane.showActions ? qsTr("Public Downloads") : qsTr("Benchmark Corpora")

                    ColumnLayout {
                        anchors.fill: parent

                        Repeater {
                            model: pane.sources.corpora
                            delegate: ColumnLayout {
                                id: corpus
                                required property int index
                                required property benchmarkCorpusRow modelData
                                Layout.fillWidth: true
                                SurfaceDivider {
                                    visible: corpus.index > 0
                                }
                                RowLayout {
                                    id: corpusControls
                                    Layout.fillWidth: true
                                    Layout.minimumHeight: 44
                                    readonly property string runCountText: !pane.showActions || !corpus.modelData.hasActions ? "" : corpus.modelData.runCount === 0 ? qsTr("No runs found") : corpus.modelData.runCount === 1 ? qsTr("1 run") : qsTr("%1 runs").arg(corpus.modelData.runCount)

                                    ColumnLayout {
                                        Layout.fillWidth: true
                                        spacing: 3

                                        Label {
                                            Layout.fillWidth: true
                                            text: corpus.modelData.repository
                                            textFormat: Text.PlainText
                                            elide: Text.ElideMiddle
                                        }
                                        Label {
                                            text: corpus.modelData.revision + (pane.compact && corpusControls.runCountText !== "" ? " · " + corpusControls.runCountText : "")
                                            Layout.fillWidth: true
                                            elide: Text.ElideRight
                                            textFormat: Text.PlainText
                                            font.family: Theme.fixedFont
                                            color: Theme.muted
                                        }
                                    }
                                    Label {
                                        visible: pane.showActions && !pane.compact
                                        Layout.preferredWidth: 100
                                        horizontalAlignment: Text.AlignRight
                                        elide: Text.ElideRight
                                        text: corpusControls.runCountText
                                        color: Theme.muted
                                    }
                                    Item {
                                        Layout.preferredWidth: 22
                                        Layout.preferredHeight: 22

                                        BusyIndicator {
                                            anchors.fill: parent
                                            visible: running
                                            palette.dark: Theme.accent
                                            running: pane.sources.pendingCorpus === corpus.modelData.key
                                        }
                                    }
                                    ActionButton {
                                        visible: pane.showActions || !corpus.modelData.saved
                                        objectName: (pane.showActions ? "downloadPipelineActions-" : "downloadPipelineCorpus-") + corpus.modelData.key
                                        text: pane.showActions && corpus.modelData.hasActions ? qsTr("Refresh") : qsTr("Download")
                                        enabled: !pane.sources.busy
                                        // qmllint disable compiler
                                        onClicked: {
                                            if (pane.showActions)
                                                pane.sources.downloadActions(corpus.modelData.key);
                                            else
                                                pane.sources.setActive(corpus.modelData.key, true);
                                        }
                                        // qmllint enable compiler
                                    }
                                    Switch {
                                        visible: !pane.showActions && corpus.modelData.saved
                                        objectName: "activatePipelineCorpus-" + corpus.modelData.key
                                        checked: corpus.modelData.active
                                        enabled: !pane.sources.busy
                                        Accessible.name: corpus.modelData.repository
                                        onToggled: pane.sources.setActive(corpus.modelData.key, checked) // qmllint disable compiler
                                    }
                                }
                            }
                        }
                    }
                }

                SurfaceGroup {
                    Layout.fillWidth: true
                    visible: pane.showActions
                    title: qsTr("Workflow Runs")

                    ColumnLayout {
                        anchors.fill: parent

                        ActionButton {
                            Layout.alignment: Qt.AlignRight
                            visible: !pane.showConnected
                            text: qsTr("Load Recorded Captures")
                            objectName: "loadPipelineCaptures"
                            enabled: !pane.sources.busy && !pane.sources.loadedCaptures
                            onClicked: pane.sources.loadRecordedCaptures() // qmllint disable compiler
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: pane.showConnected ? pane.sources.connectedWorkflows.length === 0 : pane.sources.workflows.length === 0
                            text: pane.showConnected ? (pane.sources.connectedLoaded ? qsTr("No workflow runs have been collected.") : qsTr("Refresh to load workflow runs.")) : qsTr("No workflow runs loaded.")
                            color: Theme.muted
                            horizontalAlignment: Text.AlignHCenter
                            wrapMode: Text.Wrap
                            Layout.minimumHeight: 44
                        }
                        Repeater {
                            model: pane.showConnected ? pane.sources.connectedWorkflows : pane.sources.workflows
                            delegate: RowLayout {
                                id: workflow
                                required property benchmarkWorkflowRow modelData
                                Layout.fillWidth: true
                                ColumnLayout {
                                    Layout.fillWidth: true
                                    Label {
                                        Layout.fillWidth: true
                                        text: workflow.modelData.title
                                        textFormat: Text.PlainText
                                        elide: Text.ElideMiddle
                                    }
                                    Label {
                                        Layout.fillWidth: true
                                        text: workflow.modelData.subtitle
                                        textFormat: Text.PlainText
                                        color: Theme.muted
                                        elide: Text.ElideRight
                                    }
                                }
                                Switch {
                                    checked: workflow.modelData.active
                                    enabled: !pane.sources.busy
                                    Accessible.name: workflow.modelData.title
                                    onToggled: pane.sources.setWorkflowActive(workflow.modelData.key, checked) // qmllint disable compiler
                                }
                            }
                        }
                    }
                }
            }
        }
        ColumnLayout {
            anchors.centerIn: parent
            visible: pane.loadingList

            BusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                palette.dark: Theme.accent
                running: pane.loadingList
                objectName: "pipelineSourcesLoader"
            }
            Label {
                text: qsTr("Loading sources…")
                color: Theme.muted
                Layout.alignment: Qt.AlignHCenter
            }
        }
    }
}
