import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property GitHubConnector github
    readonly property gitHubUsage usage: github.usage
    readonly property int confirmedLimit: usage.storageLimit
    readonly property int confirmedRate: usage.requestedRate

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(570, parent.width - 24)
    height: Math.min(675, parent.height - 24)
    modal: true
    title: qsTr("GitHub Requests & Storage")
    closePolicy: github.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
    objectName: "githubUsageDialog"

    onConfirmedLimitChanged: storageLimit.value = confirmedLimit
    onConfirmedRateChanged: rateLimit.value = confirmedRate
    onOpened: {
        rateLimit.value = confirmedRate;
        storageLimit.value = confirmedLimit;
        github.loadStorage(); // qmllint disable compiler
    }

    ScrollView {
        id: scroll

        anchors.fill: parent
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            width: scroll.availableWidth
            spacing: Theme.gap

            Label {
                Layout.fillWidth: true
                text: dialog.github.feedback
                textFormat: Text.PlainText
                visible: text.length > 0
                wrapMode: Text.WordWrap
            }

            SurfaceGroup {
                Layout.fillWidth: true
                visible: dialog.usage.hasTraffic
                title: qsTr("Local Rate Limit")

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 8

                    MetricRow {
                        title: qsTr("Effective limit")
                        value: "%1 %2".arg(dialog.usage.effectiveRate).arg(qsTr("requests/min"))
                    }
                    MetricRow {
                        title: qsTr("Requests in parallel")
                        value: dialog.usage.concurrency
                    }

                    RowLayout {
                        Layout.fillWidth: true

                        Label {
                            Layout.fillWidth: true
                            text: qsTr("Requested limit")
                            color: Theme.muted
                            wrapMode: Text.WordWrap
                        }
                        SpinBox {
                            id: rateLimit

                            objectName: "githubRequestRate"
                            Accessible.name: qsTr("Maximum requests per minute")
                            from: 1
                            to: Math.max(1, dialog.usage.maximumRate)
                            editable: true
                            enabled: !dialog.github.busy
                        }
                    }

                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Choose 1–%1 requests per minute for all repositories.").arg(dialog.usage.maximumRate)
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                        font.pixelSize: 12
                    }

                    ActionButton {
                        Layout.alignment: Qt.AlignRight
                        objectName: "githubApplyRate"
                        text: qsTr("Apply Limit")
                        enabled: !dialog.github.busy && rateLimit.value !== dialog.confirmedRate
                        onClicked: dialog.github.setRequestLimit(rateLimit.value) // qmllint disable compiler
                    }
                }
            }

            SurfaceGroup {
                Layout.fillWidth: true
                title: qsTr("GitHub Storage")

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 9

                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Storage usage unavailable.")
                        visible: dialog.confirmedLimit === 0 && !dialog.github.busy
                        wrapMode: Text.WordWrap
                    }
                    BusyIndicator {
                        Layout.alignment: Qt.AlignHCenter
                        implicitWidth: 26
                        implicitHeight: 26
                        running: dialog.confirmedLimit === 0 && dialog.github.busy
                        visible: running
                    }
                    MetricRow {
                        visible: dialog.confirmedLimit > 0
                        objectName: "githubStorageBytes"
                        title: qsTr("Retained payload")
                        value: dialog.usage.retainedPayload
                    }
                    RowLayout {
                        Layout.fillWidth: true
                        visible: dialog.confirmedLimit > 0

                        Label {
                            Layout.fillWidth: true
                            text: qsTr("Storage limit")
                            color: Theme.muted
                            wrapMode: Text.WordWrap
                        }
                        SpinBox {
                            id: storageLimit

                            objectName: "githubStorageLimit"
                            Accessible.name: qsTr("Storage limit in decimal GB")
                            from: 1
                            to: 1000
                            editable: true
                            enabled: !dialog.github.busy
                        }
                        Label {
                            text: qsTr("GB")
                            color: Theme.muted
                        }
                    }

                    Label {
                        Layout.fillWidth: true
                        visible: dialog.confirmedLimit > 0
                        text: qsTr("Choose 1–1,000 decimal GB. Logical payload excludes SQLite indexes, free pages, WAL and filesystem overhead. Lowering the limit does not delete data.")
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                        font.pixelSize: 12
                    }

                    ActionButton {
                        Layout.alignment: Qt.AlignRight
                        objectName: "githubApplyStorage"
                        text: qsTr("Apply Storage Limit")
                        visible: dialog.confirmedLimit > 0
                        enabled: !dialog.github.busy && storageLimit.value !== dialog.confirmedLimit
                        onClicked: dialog.github.setStorageLimit(storageLimit.value) // qmllint disable compiler
                    }
                }
            }

            SurfaceGroup {
                Layout.fillWidth: true
                visible: dialog.usage.hasTraffic
                title: qsTr("Request Activity")

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 6

                    MetricRow {
                        title: qsTr("Attempts")
                        value: dialog.usage.attempts
                    }
                    MetricRow {
                        title: qsTr("Failed")
                        value: dialog.usage.failures
                    }
                    MetricRow {
                        title: qsTr("Queued")
                        value: dialog.usage.queued
                    }
                    MetricRow {
                        title: qsTr("In flight")
                        value: dialog.usage.inFlight
                    }
                    MetricRow {
                        title: qsTr("Uncertain")
                        value: dialog.usage.uncertain
                    }
                    MetricRow {
                        visible: value.length > 0
                        title: qsTr("Cooldown ends")
                        value: dialog.usage.cooldown
                    }
                }
            }
        }
    }

    footer: DialogActions {
        ActionButton {
            text: qsTr("Refresh Storage")
            enabled: !dialog.github.busy
            onClicked: {
                storageLimit.value = dialog.confirmedLimit;
                dialog.github.loadStorage(); // qmllint disable compiler
            }
        }
        ActionButton {
            text: qsTr("Done")
            highlighted: true
            enabled: !dialog.github.busy
            DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
        }
    }
}
