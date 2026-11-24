pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: row

    required property ModelLibrary library
    required property libraryModel entry
    required property bool separator
    readonly property bool pending: library.pendingProfile === entry.profile
    signal discardRequested(libraryModel candidate)

    spacing: 4

    Rectangle {
        Layout.fillWidth: true
        Layout.preferredHeight: 1
        Layout.bottomMargin: 6
        visible: row.separator
        color: Theme.outline
    }

    GridLayout {
        Layout.fillWidth: true
        columns: width >= 420 ? 2 : 1
        columnSpacing: Theme.gap
        rowSpacing: 4

        ColumnLayout {
            Layout.fillWidth: true
            Layout.minimumWidth: 0
            spacing: 3

            Label {
                Layout.fillWidth: true
                text: row.entry.label
                textFormat: Text.PlainText
                elide: Text.ElideMiddle
                font.weight: Font.DemiBold
            }
            Label {
                Layout.fillWidth: true
                objectName: "model-status-" + row.entry.profile
                text: row.entry.status
                color: Theme.muted
                wrapMode: Text.Wrap
            }
        }

        RowLayout {
            Layout.alignment: Qt.AlignRight | Qt.AlignVCenter
            spacing: 6

            BusyIndicator {
                Layout.preferredWidth: 22
                Layout.preferredHeight: 22
                visible: running
                running: row.pending
                Accessible.name: qsTr("Updating model")
            }
            ActionButton {
                visible: !row.pending && text !== ""
                enabled: row.entry.canAct && row.library.pendingProfile === "" && row.library.catalogFeedback === ""
                text: row.entry.actionLabel
                Accessible.name: text + " " + row.entry.label
                objectName: "model-action-" + row.entry.profile
                onClicked: row.library.act(row.entry) // qmllint disable compiler
            }
            ActionButton {
                visible: !row.pending && row.entry.canDiscard
                enabled: row.library.pendingProfile === "" && row.library.catalogFeedback === ""
                text: qsTr("Discard…")
                Accessible.name: text + " " + row.entry.label
                objectName: "model-discard-" + row.entry.profile
                onClicked: row.discardRequested(row.entry)
            }
        }
    }

    ActionButton {
        id: sourceLink

        visible: row.entry.sourceUrl.toString() !== ""
        text: row.entry.source
        objectName: "model-source-" + row.entry.profile
        leftPadding: 0
        rightPadding: 0
        Layout.preferredHeight: 24
        contentItem: Label {
            text: sourceLink.text
            textFormat: Text.PlainText
            color: Theme.accent
            font.pixelSize: 12
            verticalAlignment: Text.AlignVCenter
            elide: Text.ElideRight
        }
        background: Rectangle {
            radius: Theme.controlRadius
            color: sourceLink.hovered ? Theme.hovered : "transparent"
            border.width: sourceLink.visualFocus ? 1 : 0
            border.color: Theme.accent
        }
        onClicked: Qt.openUrlExternally(row.entry.sourceUrl)
    }

    ProgressBar {
        Layout.fillWidth: true
        visible: row.entry.transferring
        objectName: "model-progress-" + row.entry.profile
        Accessible.name: indeterminate ? qsTr("Model installation progress") : qsTr("Download progress")
        indeterminate: !row.entry.determinate
        value: row.entry.progress
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: row.entry.feedback
        color: Theme.muted
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: row.library.feedbackProfile === row.entry.profile ? row.library.feedback : ""
        color: Theme.danger
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: row.entry.peakSpace
        color: Theme.muted
        font.pixelSize: 12
        wrapMode: Text.Wrap
    }
}
