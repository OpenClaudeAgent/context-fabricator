import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: section

    required property gitHubCollection collection
    required property bool refreshEnabled
    signal refreshRequested

    title: qsTr("Branch Sync")

    ColumnLayout {
        width: section.availableWidth
        spacing: 7

        RowLayout {
            Layout.fillWidth: true

            Label {
                Layout.fillWidth: true
                text: section.collection.graphOnly ? qsTr("Code graph") : qsTr("Branch files")
                wrapMode: Text.WordWrap
            }
            ActionButton {
                text: qsTr("Refresh Now")
                objectName: "githubRefreshBranch"
                enabled: section.refreshEnabled
                onClicked: section.refreshRequested()
            }
        }

        RowLayout {
            Layout.fillWidth: true

            MetricRow {
                title: qsTr("Status")
                value: section.collection.status
                objectName: "githubCollectionStatus"
            }
            BusyIndicator {
                Layout.preferredWidth: 18
                Layout.preferredHeight: 18
                running: section.collection.running
                visible: running
                Accessible.ignored: true
            }
        }
        MetricRow {
            visible: section.collection.pendingFiles >= 0
            title: qsTr("Files prepared")
            value: section.collection.pendingFiles.toLocaleString(Qt.locale(), "f", 0)
        }
        MetricRow {
            visible: section.collection.pendingCommit.length > 0
            title: qsTr("Commit being synced")
            value: section.collection.pendingCommit.slice(0, 10)
            ToolTip.text: section.collection.pendingCommit
            ToolTip.visible: pendingCommitHover.hovered

            HoverHandler {
                id: pendingCommitHover
            }
        }
        MetricRow {
            visible: section.collection.commit.length > 0
            title: qsTr("Local commit")
            value: section.collection.commit.slice(0, 10)
            ToolTip.text: section.collection.commit
            ToolTip.visible: localCommitHover.hovered

            HoverHandler {
                id: localCommitHover
            }
        }
        MetricRow {
            visible: section.collection.publishedFiles >= 0
            title: qsTr("Files in local copy")
            value: section.collection.publishedFiles.toLocaleString(Qt.locale(), "f", 0)
        }
        MetricRow {
            visible: section.collection.lastChecked.length > 0
            title: qsTr("Last checked")
            value: section.collection.lastChecked
        }
        MetricRow {
            visible: section.collection.nextCheck.length > 0
            title: qsTr("Next check")
            value: section.collection.nextCheck
        }
        Label {
            Layout.fillWidth: true
            text: section.collection.error
            textFormat: Text.PlainText
            visible: text.length > 0
            wrapMode: Text.WordWrap
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("The first sync must finish before this branch is available in the app.")
            visible: section.collection.pendingCommit.length > 0 && section.collection.commit.length === 0 && section.collection.error.length === 0
            wrapMode: Text.WordWrap
        }
    }
}
