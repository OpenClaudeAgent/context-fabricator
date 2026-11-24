pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property AnswerHistory history
    required property Answers answers
    readonly property bool wide: width >= 590
    readonly property bool showsSelection: history.selectedId !== ""
    property bool opensSettings: false
    property string destinationTraceId
    signal historySettingsRequested
    signal traceRequested(string traceId)

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(640, parent.width - 24)
    height: Math.min(wide ? 430 : 530, parent.height - 24)
    modal: true
    closePolicy: Popup.CloseOnEscape
    title: qsTr("Answer History")
    objectName: "answerHistoryDialog"

    onAboutToShow: {
        tabs.currentIndex = answers.sessionQuestions.length > 0 && !history.savingEnabled ? 0 : 1;
        history.open(); // qmllint disable compiler
    }
    onClosed: {
        history.close(); // qmllint disable compiler
        if (opensSettings) {
            opensSettings = false;
            historySettingsRequested();
        } else if (destinationTraceId !== "") {
            const traceId = destinationTraceId;
            destinationTraceId = "";
            traceRequested(traceId);
        }
    }

    header: Item {
        implicitHeight: heading.implicitHeight + Theme.dialogMargin + 4

        RowLayout {
            id: heading

            y: Theme.dialogMargin
            x: Theme.dialogMargin
            width: parent.width - 2 * Theme.dialogMargin

            Label {
                Layout.fillWidth: true
                text: dialog.title
                font.pixelSize: 20
                font.weight: Font.DemiBold
                color: Theme.text
                Accessible.role: Accessible.Heading
            }
            ActionButton {
                text: qsTr("Done")
                onClicked: dialog.close()
            }
        }
    }

    ColumnLayout {
        anchors.fill: parent
        spacing: 10

        TabBar {
            id: tabs

            Layout.preferredWidth: Math.min(250, dialog.availableWidth)
            Accessible.name: qsTr("History type")
            objectName: "answerHistoryType"
            spacing: 2
            background: Rectangle {
                color: Theme.raised
                radius: Theme.controlRadius
            }
            SegmentTab {
                text: qsTr("This Session")
            }
            SegmentTab {
                text: qsTr("Saved")
            }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: tabs.currentIndex

            ColumnLayout {
                spacing: 10

                Label {
                    Layout.fillWidth: true
                    visible: recent.count > 0
                    text: qsTr("Recent questions from this app session. They disappear when you quit.")
                    color: Theme.muted
                    wrapMode: Text.Wrap
                }
                ListView {
                    id: recent

                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    visible: count > 0
                    clip: true
                    model: dialog.answers.sessionQuestions
                    ScrollBar.vertical: ScrollBar {}
                    delegate: HistoryRow {
                        required property sessionQuestion modelData
                        width: recent.width
                        question: modelData.text
                        subtitle: modelData.outcome
                        onClicked: dialog.openTrace(modelData.traceId)
                    }
                }
                Label {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    visible: recent.count === 0
                    text: qsTr("No Questions This Session")
                    horizontalAlignment: Text.AlignHCenter
                    verticalAlignment: Text.AlignVCenter
                    wrapMode: Text.Wrap
                }
                RowLayout {
                    Layout.fillWidth: true
                    visible: !dialog.history.loading && !dialog.history.savingEnabled

                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Saving history is off.")
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                    ActionButton {
                        text: qsTr("History Settings…")
                        onClicked: dialog.openSettings()
                    }
                }
            }

            ColumnLayout {
                spacing: 10

                Label {
                    Layout.fillWidth: true
                    visible: saved.count > 0 && (dialog.wide || !dialog.showsSelection)
                    text: qsTr("Saved locally. Viewing an entry does not ask the model again.")
                    color: Theme.muted
                    wrapMode: Text.Wrap
                }
                RowLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    visible: saved.count > 0 || dialog.showsSelection
                    spacing: 10

                    ListView {
                        id: saved

                        Layout.fillWidth: !dialog.wide
                        Layout.preferredWidth: dialog.wide ? 216 : dialog.availableWidth
                        Layout.fillHeight: true
                        visible: dialog.wide || !dialog.showsSelection
                        objectName: "savedAnswerList"
                        clip: true
                        model: dialog.history
                        ScrollBar.vertical: ScrollBar {}
                        delegate: HistoryRow {
                            required property int index
                            required property historySummary entry
                            width: saved.width
                            question: entry.question
                            subtitle: entry.completedAt
                            highlighted: entry.key === dialog.history.selectedId
                            objectName: "savedAnswer-" + entry.key
                            onClicked: dialog.history.select(index) // qmllint disable compiler
                        }
                        footer: Item {
                            width: saved.width
                            height: more.visible ? more.implicitHeight + 8 : 0

                            ActionButton {
                                id: more

                                anchors.centerIn: parent
                                visible: dialog.history.hasMore
                                enabled: !dialog.history.loading
                                text: qsTr("Load More")
                                onClicked: dialog.history.loadMore() // qmllint disable compiler
                            }
                        }
                    }
                    Rectangle {
                        Layout.preferredWidth: 1
                        Layout.fillHeight: true
                        visible: dialog.wide
                        color: Theme.outline
                    }
                    ColumnLayout {
                        Layout.fillWidth: true
                        Layout.fillHeight: true
                        visible: dialog.wide || dialog.showsSelection

                        ActionButton {
                            visible: !dialog.wide && dialog.showsSelection
                            text: qsTr("Back")
                            onClicked: dialog.history.deselect() // qmllint disable compiler
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: !dialog.history.savedAnswer.present
                            text: dialog.history.loadingDetail ? qsTr("Loading saved answer…") : dialog.history.feedback || qsTr("Select an answer to view it.")
                            textFormat: Text.PlainText
                            color: Theme.muted
                            wrapMode: Text.Wrap
                        }
                        SavedAnswerDetail {
                            Layout.fillWidth: true
                            Layout.fillHeight: true
                            visible: dialog.history.savedAnswer.present
                            entry: dialog.history.savedAnswer
                            onTraceRequested: function (traceId) {
                                dialog.openTrace(traceId);
                            }
                        }
                    }
                }
                ColumnLayout {
                    Layout.fillWidth: true
                    Layout.fillHeight: true
                    visible: saved.count === 0 && !dialog.showsSelection
                    spacing: 8

                    Item {
                        Layout.fillHeight: true
                    }
                    Label {
                        Layout.fillWidth: true
                        text: {
                            if (dialog.history.loading)
                                return qsTr("Loading history…");
                            if (dialog.history.feedback !== "")
                                return dialog.history.feedback;
                            return dialog.history.savingEnabled ? qsTr("No saved answers.") : qsTr("Saving History Is Off");
                        }
                        textFormat: Text.PlainText
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        font.weight: Font.DemiBold
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: !dialog.history.loading && dialog.history.feedback === ""
                        text: dialog.history.savingEnabled ? qsTr("New completed questions will appear here.") : qsTr("Enable answer history to keep questions and answers after quitting.")
                        horizontalAlignment: Text.AlignHCenter
                        wrapMode: Text.Wrap
                        color: Theme.muted
                    }
                    ActionButton {
                        Layout.alignment: Qt.AlignHCenter
                        visible: !dialog.history.loading && !dialog.history.savingEnabled
                        text: qsTr("History Settings…")
                        onClicked: dialog.openSettings()
                    }
                    ActionButton {
                        Layout.alignment: Qt.AlignHCenter
                        visible: !dialog.history.loading && dialog.history.savingEnabled && dialog.history.feedback !== ""
                        text: qsTr("Retry")
                        onClicked: dialog.history.refresh() // qmllint disable compiler
                    }
                    Item {
                        Layout.fillHeight: true
                    }
                }
            }
        }
    }

    function openSettings(): void {
        opensSettings = true;
        close();
    }

    function openTrace(traceId: string): void {
        destinationTraceId = traceId;
        close();
    }

    component HistoryRow: ItemDelegate {
        id: row

        required property string question
        required property string subtitle
        padding: 8
        Accessible.name: question
        Accessible.description: subtitle

        contentItem: ColumnLayout {
            spacing: 4

            Label {
                Layout.fillWidth: true
                text: row.question
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                maximumLineCount: 2
                elide: Text.ElideRight
            }
            Label {
                Layout.fillWidth: true
                text: row.subtitle
                color: Theme.muted
                font.pixelSize: 12
            }
        }
        background: Rectangle {
            color: row.highlighted ? Theme.selection : row.hovered ? Theme.hovered : "transparent"
            radius: Theme.controlRadius
            border.color: row.visualFocus ? Theme.accent : "transparent"
        }
    }
}
