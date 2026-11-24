import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: page

    required property Overview overview
    readonly property int gutter: width < 580 ? 16 : Theme.margin
    signal answersRequested

    padding: gutter
    contentWidth: availableWidth
    clip: true

    ColumnLayout {
        width: Math.min(Theme.workspaceWidth, page.availableWidth)
        x: (page.availableWidth - width) / 2
        spacing: Theme.gap

        GridLayout {
            Layout.fillWidth: true
            columns: heading.implicitWidth + actions.implicitWidth + columnSpacing <= width ? 2 : 1
            columnSpacing: 8
            rowSpacing: 8

            PageTitle {
                id: heading

                Layout.fillWidth: true
                text: qsTr("Overview")
            }

            RowLayout {
                id: actions

                Layout.alignment: Qt.AlignRight
                spacing: 8

                ActionButton {
                    text: qsTr("Refresh")
                    enabled: !page.overview.busy
                    onClicked: page.overview.refresh() // qmllint disable compiler
                }
                ActionButton {
                    text: qsTr("Ask a question")
                    objectName: "askQuestionFromOverview"
                    highlighted: true
                    enabled: page.overview.ready
                    onClicked: page.answersRequested()
                }
            }
        }

        SurfaceGroup {
            Layout.fillWidth: true
            title: qsTr("Corpus")

            ColumnLayout {
                anchors.fill: parent
                spacing: 8

                MetricRow {
                    title: qsTr("Searchable documents")
                    value: page.overview.ready ? page.overview.indexedDocuments.toLocaleString(Qt.locale(), "f", 0) : "—"
                }
                SurfaceDivider {
                    visible: corpusCollected.visible
                }
                MetricRow {
                    id: corpusCollected

                    title: qsTr("Collected documents")
                    value: page.overview.collectedDocuments
                    visible: page.overview.ready && value !== ""
                }
                SurfaceDivider {}
                MetricRow {
                    title: qsTr("Connected sources")
                    value: page.overview.ready ? page.overview.connectedSources.toLocaleString(Qt.locale(), "f", 0) : "—"
                }
            }
        }

        GridLayout {
            Layout.fillWidth: true
            columns: width >= 580 ? 2 : 1
            uniformCellWidths: true
            columnSpacing: Theme.gap
            rowSpacing: Theme.gap

            SurfaceGroup {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignTop
                title: qsTr("Local service")

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 8

                    RowLayout {
                        Layout.fillWidth: true

                        Label {
                            Layout.fillWidth: true
                            text: qsTr("Backend")
                            color: Theme.muted
                        }
                        StatusChip {
                            Layout.maximumWidth: 215
                            objectName: "backendState"
                            text: page.overview.serviceState
                            ready: page.overview.ready
                        }
                    }
                    SurfaceDivider {
                        visible: embeddingIndex.visible || localInference.visible
                    }
                    MetricRow {
                        id: embeddingIndex

                        title: qsTr("Embedding index")
                        value: page.overview.embeddingProgress || "—"
                        visible: page.overview.ready && page.overview.embeddingState !== ""
                    }
                    SurfaceDivider {
                        visible: embeddingIndex.visible && embeddingState.visible
                    }
                    MetricRow {
                        id: embeddingState

                        title: qsTr("Embedding state")
                        value: page.overview.embeddingState
                        visible: page.overview.ready && value !== ""
                    }
                    MetricRow {
                        id: localInference

                        title: qsTr("Local inference")
                        value: qsTr("Awaiting configuration")
                        visible: page.overview.ready && page.overview.embeddingState === ""
                    }
                    SurfaceDivider {
                        visible: restart.visible
                    }
                    ActionButton {
                        id: restart

                        Layout.alignment: Qt.AlignRight
                        objectName: "restartBackend"
                        text: qsTr("Restart backend")
                        visible: page.overview.managed
                        enabled: page.overview.canRestart
                        onClicked: page.overview.restart() // qmllint disable compiler
                    }
                }
            }

            SurfaceGroup {
                Layout.fillWidth: true
                Layout.alignment: Qt.AlignTop
                title: qsTr("Outbound traffic")

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 8

                    MetricRow {
                        title: qsTr("Outbound attempts")
                        value: page.overview.ready ? page.overview.outboundAttempts.toLocaleString(Qt.locale(), "f", 0) : "—"
                    }
                    SurfaceDivider {
                        visible: pending.visible
                    }
                    MetricRow {
                        id: pending

                        title: qsTr("Pending outcomes")
                        value: page.overview.pendingOutcomes
                        visible: page.overview.ready && value !== ""
                    }
                    SurfaceDivider {
                        visible: uncertain.visible
                    }
                    MetricRow {
                        id: uncertain

                        title: qsTr("Uncertain outcomes")
                        value: page.overview.uncertainOutcomes
                        visible: page.overview.ready && value !== ""
                    }
                }
            }
        }
    }
}
