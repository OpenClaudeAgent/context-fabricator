import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: storage

    required property Diagnostics diagnostics
    spacing: Theme.gap

    Label {
        visible: !storage.diagnostics.ready
        text: qsTr("Storage Unavailable")
        color: Theme.muted
    }

    SurfaceGroup {
        Layout.fillWidth: true
        visible: storage.diagnostics.ready
        title: qsTr("Retention & Storage")

        ColumnLayout {
            anchors.fill: parent
            spacing: 10

            MetricRow {
                title: qsTr("Stored on disk")
                value: storage.diagnostics.storageUsed
            }
            MetricRow {
                title: qsTr("Maximum disk space")
                value: storage.diagnostics.storageBudget
            }
            ProgressBar {
                Layout.fillWidth: true
                value: storage.diagnostics.storageFraction
                Accessible.name: qsTr("Disk budget used")
            }
            MetricRow {
                title: qsTr("Retention")
                value: storage.diagnostics.storageRetention
            }
            MetricRow {
                visible: value !== ""
                title: qsTr("Oldest retained record")
                value: storage.diagnostics.storageOldest
            }
            ActionButton {
                Layout.alignment: Qt.AlignRight
                objectName: "purgeExpiredDiagnostics"
                text: qsTr("Purge expired")
                enabled: !storage.diagnostics.busy
                onClicked: storage.diagnostics.purgeExpired() // qmllint disable compiler
            }
        }
    }

    SurfaceGroup {
        Layout.fillWidth: true
        visible: storage.diagnostics.ready
        title: qsTr("Record Handling")

        ColumnLayout {
            anchors.fill: parent
            spacing: 10

            MetricRow {
                title: qsTr("Queued records")
                value: storage.diagnostics.queuedCount
            }
            MetricRow {
                title: qsTr("Dropped records")
                value: storage.diagnostics.droppedCount
            }
            MetricRow {
                title: qsTr("Purged records")
                value: storage.diagnostics.purgedCount
            }
            MetricRow {
                visible: value !== ""
                title: qsTr("Debug expires")
                value: storage.diagnostics.debugExpiry
            }
        }
    }
}
