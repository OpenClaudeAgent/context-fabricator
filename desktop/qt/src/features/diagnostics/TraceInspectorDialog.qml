pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    enum Pane {
        Answer,
        Spans,
        Logs
    }

    required property TraceInspector inspector
    required property TraceAnswer answer
    readonly property bool wide: width >= 700
    property bool showRecordedAnswerWhenReady: false

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(870, parent.width - 24)
    height: Math.min(610, parent.height - 24)
    modal: true
    visible: inspector.visible
    closePolicy: Popup.CloseOnEscape
    title: answer.available ? qsTr("Answer trace") : qsTr("Trace")
    objectName: "traceInspectorDialog"

    onOpened: selectInitialPane()
    onClosed: inspector.close() // qmllint disable compiler

    function selectInitialPane(): void {
        const root = inspector.opensRoot;
        const showAnswer = root && answer.available && (answer.live || inspector.recordedAbstention);
        tabs.currentIndex = showAnswer ? TraceInspectorDialog.Answer : TraceInspectorDialog.Spans;
        showRecordedAnswerWhenReady = root && !showAnswer;
    }

    Connections {
        target: dialog.inspector

        function onDestinationChanged(): void {
            if (dialog.inspector.visible)
                dialog.selectInitialPane();
        }

        function onRecordsChanged(): void {
            if (dialog.showRecordedAnswerWhenReady && dialog.answer.available && dialog.inspector.recordedAbstention) {
                tabs.currentIndex = TraceInspectorDialog.Answer;
                dialog.showRecordedAnswerWhenReady = false;
            }
        }
    }
    Connections {
        target: dialog.answer

        function onChanged(): void {
            if (!dialog.answer.available && tabs.currentIndex === TraceInspectorDialog.Answer)
                tabs.currentIndex = TraceInspectorDialog.Spans;
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        Flow {
            Layout.fillWidth: true
            Layout.preferredHeight: implicitHeight
            visible: dialog.inspector.canGoBack || dialog.inspector.hasMore
            spacing: 8

            ActionButton {
                visible: dialog.inspector.canGoBack
                text: qsTr("Back")
                onClicked: {
                    tabs.currentIndex = TraceInspectorDialog.Spans;
                    dialog.inspector.goBack(); // qmllint disable compiler
                }
            }
            ActionButton {
                visible: dialog.inspector.hasMore
                enabled: !dialog.inspector.busy
                text: qsTr("Load More")
                onClicked: dialog.inspector.loadMore() // qmllint disable compiler
            }
        }

        Label {
            Layout.fillWidth: true
            visible: tabs.currentIndex !== TraceInspectorDialog.Answer && text.length > 0
            text: dialog.inspector.busy ? qsTr("Loading trace…") : dialog.inspector.feedback
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }

        GridLayout {
            Layout.fillWidth: true
            columns: dialog.wide ? 2 : 1
            columnSpacing: Theme.gap
            rowSpacing: Theme.gap

            TabBar {
                id: tabs

                readonly property real stripWidth: Math.min(228, dialog.availableWidth)
                readonly property real tabWidth: stripWidth / (dialog.answer.available ? 3 : 2)

                Layout.preferredWidth: stripWidth
                Accessible.name: qsTr("Trace records")
                currentIndex: TraceInspectorDialog.Spans
                spacing: 0
                background: Rectangle {
                    color: Theme.raised
                    radius: Theme.controlRadius
                }

                SegmentTab {
                    text: qsTr("Answer")
                    visible: dialog.answer.available
                    enabled: dialog.answer.available
                    width: dialog.answer.available ? tabs.tabWidth : 0
                    objectName: "traceAnswerTab"
                    onClicked: dialog.showRecordedAnswerWhenReady = false
                }
                SegmentTab {
                    text: qsTr("Spans")
                    width: tabs.tabWidth
                    onClicked: dialog.showRecordedAnswerWhenReady = false
                }
                SegmentTab {
                    text: qsTr("Logs")
                    width: tabs.tabWidth
                    onClicked: dialog.showRecordedAnswerWhenReady = false
                }
            }

            RowLayout {
                Layout.fillWidth: true
                spacing: 8

                SurfaceInput {
                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    text: dialog.inspector.traceId
                    readOnly: true
                    selectByMouse: true
                    horizontalAlignment: dialog.wide ? TextInput.AlignRight : TextInput.AlignLeft
                    Accessible.name: qsTr("Trace ID")
                    objectName: "traceInspectorId"
                }
                ActionButton {
                    text: qsTr("Copy Trace ID")
                    onClicked: dialog.inspector.copyTraceId() // qmllint disable compiler
                }
            }
        }

        Label {
            Layout.fillWidth: true
            visible: tabs.currentIndex !== TraceInspectorDialog.Answer && text !== ""
            text: dialog.answer.question
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            maximumLineCount: 2
            elide: Text.ElideRight
            color: Theme.muted
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            TraceAnswerPane {
                answer: dialog.answer
                inspector: dialog.inspector
                onSpansRequested: tabs.currentIndex = TraceInspectorDialog.Spans
            }

            ColumnLayout {
                Label {
                    Layout.fillWidth: true
                    visible: dialog.inspector.selectedIndex < 0 && !dialog.inspector.busy
                    text: qsTr("No Spans in the Loaded Records")
                    wrapMode: Text.WordWrap
                }

                GridLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    visible: dialog.inspector.selectedIndex >= 0
                    columns: dialog.wide ? 2 : 1
                    columnSpacing: Theme.gap
                    rowSpacing: 8

                    TraceOutlinePane {
                        inspector: dialog.inspector
                        visible: dialog.wide
                        Layout.preferredWidth: 260
                        Layout.fillHeight: true
                    }

                    LocalizedComboBox {
                        Layout.fillWidth: true
                        visible: !dialog.wide
                        labels: dialog.inspector.spanTitles
                        selectedIndex: dialog.inspector.selectedIndex
                        Accessible.name: qsTr("Span hierarchy")
                        onActivated: function (index) {
                            dialog.inspector.selectSpan(index); // qmllint disable compiler
                        }
                    }

                    TraceSpanDetail {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        span: dialog.inspector.selected
                        logs: dialog.inspector.spanLogs
                        onWorkTraceRequested: dialog.inspector.followWorkTrace() // qmllint disable compiler
                    }
                }
            }

            ColumnLayout {
                Label {
                    Layout.fillWidth: true
                    visible: traceLogs.count === 0 && !dialog.inspector.busy
                    text: qsTr("No Logs in the Loaded Records")
                    wrapMode: Text.WordWrap
                }
                ListView {
                    id: traceLogs

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    spacing: 8
                    clip: true
                    model: dialog.inspector.logs
                    ScrollBar.vertical: ScrollBar {
                        id: logsScroll
                    }
                    delegate: TraceLogRow {
                        required property traceRecord modelData

                        width: traceLogs.width - logsScroll.width
                        showSpanId: true
                        record: modelData
                    }
                }
            }
        }
    }

    footer: DialogActions {
        ActionButton {
            text: qsTr("Refresh Trace")
            enabled: !dialog.inspector.busy
            onClicked: {
                dialog.inspector.reload(); // qmllint disable compiler
                dialog.answer.reload(); // qmllint disable compiler
            }
        }
        ActionButton {
            text: qsTr("Close")
            objectName: "traceInspectorClose"
            onClicked: dialog.inspector.close() // qmllint disable compiler
        }
    }
}
