pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: row

    required property gitHubActionsRun observation
    objectName: "githubActionRun-" + observation.id + "-" + observation.attempt
    Accessible.role: Accessible.Grouping
    Accessible.name: "%1 · %2 · %3".arg(observation.workflow).arg(observation.summary).arg(observation.resultLabel)

    GridLayout {
        width: parent.width
        columns: row.availableWidth >= 630 ? 3 : 1
        columnSpacing: Theme.gap
        rowSpacing: 8

        ColumnLayout {
            Layout.fillWidth: true
            spacing: 4

            Label {
                Layout.fillWidth: true
                text: row.observation.workflow
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                font.weight: Font.DemiBold
            }
            Label {
                Layout.fillWidth: true
                text: row.observation.summary
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.muted
            }
        }

        ColumnLayout {
            spacing: 4

            Label {
                text: row.observation.resultLabel
                textFormat: Text.PlainText
                color: row.resultColor()
                font.weight: Font.DemiBold
            }
            Label {
                text: row.observation.createdAt
                textFormat: Text.PlainText
                color: Theme.muted
            }
        }

        ActionButton {
            visible: row.observation.url.toString().length > 0
            objectName: "githubActionLink-" + row.observation.id + "-" + row.observation.attempt
            text: qsTr("Open on GitHub")
            Accessible.role: Accessible.Link
            Accessible.name: row.observation.linkLabel
            onClicked: Qt.openUrlExternally(row.observation.url)
        }
    }

    function resultColor(): color {
        switch (observation.result) {
        case "success":
            return Theme.statusReady;
        case "failure":
        case "timed_out":
        case "action_required":
            return Theme.danger;
        case "queued":
        case "in_progress":
        case "waiting":
        case "pending":
            return Theme.statusWaiting;
        default:
            return Theme.muted;
        }
    }
}
