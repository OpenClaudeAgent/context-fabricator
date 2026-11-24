import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: group

    required property HistoryPreferences history
    readonly property list<int> presetDays: [1, 7, 30, 90, 365]
    readonly property int matchingPreset: presetDays.indexOf(history.retentionDays)
    readonly property bool editable: history.ready && !history.busy && !history.conflict
    property bool showingCustom: false

    title: qsTr("Answer history")

    ColumnLayout {
        anchors.fill: parent

        RowLayout {
            Layout.fillWidth: true
            visible: group.history.ready

            Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: qsTr("Save answer history")
            }

            Switch {
                objectName: "historyEnabled"
                Accessible.name: qsTr("Save answer history")
                Accessible.checkable: true
                Accessible.checked: checked
                checkable: false
                checked: group.history.enabled
                enabled: group.editable
                onClicked: {
                    if (checked) {
                        confirmation.disabling = true;
                        confirmation.open();
                    } else {
                        group.history.setEnabled(true); // qmllint disable compiler
                    }
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: group.history.ready

            Label {
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                text: qsTr("Keep answers for")
            }

            LocalizedComboBox {
                Accessible.name: qsTr("Keep answers for")
                enabled: group.editable
                labels: [qsTr("1 day"), qsTr("7 days"), qsTr("30 days"), qsTr("90 days"), qsTr("1 year"), qsTr("Custom…")]
                selectedIndex: group.showingCustom || group.matchingPreset < 0 ? group.presetDays.length : group.matchingPreset

                onActivated: index => {
                    group.showingCustom = index === group.presetDays.length;
                    if (!group.showingCustom)
                        group.history.setRetentionDays(group.presetDays[index]); // qmllint disable compiler
                }
            }
        }

        SettingsInput {
            label: qsTr("Custom duration")
            fieldName: "historyRetentionDays"
            value: group.history.retentionDays.toString()
            unit: qsTr("days")
            visible: group.history.ready && (group.showingCustom || group.matchingPreset < 0)
            enabled: group.editable
            onCommitted: value => {
                group.history.setCustomRetention(value); // qmllint disable compiler
            }
        }

        RowLayout {
            Layout.fillWidth: true

            Label {
                objectName: "historyFeedback"
                Layout.fillWidth: true
                wrapMode: Text.WordWrap
                textFormat: Text.PlainText
                text: group.history.busy ? qsTr("Working") : group.history.feedback
            }

            ActionButton {
                visible: group.history.conflict || !group.history.ready
                enabled: !group.history.busy
                text: qsTr("Reload history settings")
                onClicked: group.history.reload() // qmllint disable compiler
            }

            ActionButton {
                objectName: "clearHistory"
                visible: group.history.ready
                enabled: !group.history.busy
                text: qsTr("Clear History…")
                onClicked: {
                    confirmation.disabling = false;
                    confirmation.open();
                }
            }
        }
    }

    ConfirmationDialog {
        id: confirmation

        property bool disabling: false

        title: disabling ? qsTr("Disable and clear answer history?") : qsTr("Clear all saved answers?")
        message: disabling ? qsTr("This deletes saved questions and answers and turns off history.") : qsTr("This clears saved answers and recent session questions, including history for the current request.")

        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }

            ActionButton {
                text: confirmation.disabling ? qsTr("Disable and Clear") : qsTr("Clear History")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        onAccepted: {
            if (disabling) {
                group.history.setEnabled(false); // qmllint disable compiler
            } else {
                group.history.clear(); // qmllint disable compiler
            }
        }
    }
}
