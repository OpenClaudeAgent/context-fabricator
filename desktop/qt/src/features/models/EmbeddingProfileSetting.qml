pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: setting

    required property EmbeddingProfiles profiles
    signal libraryRequested

    GridLayout {
        Layout.fillWidth: true
        columns: width < 380 ? 1 : 2

        Label {
            Layout.fillWidth: true
            text: qsTr("Embedding model")
            wrapMode: Text.Wrap
        }

        RowLayout {
            Layout.alignment: Qt.AlignRight
            spacing: 8

            BusyIndicator {
                Layout.preferredWidth: 20
                Layout.preferredHeight: 20
                visible: running
                running: setting.profiles.busy
                Accessible.name: qsTr("Loading embedding models…")
            }

            LocalizedComboBox {
                id: picker

                Layout.preferredWidth: Math.min(260, Math.max(0, setting.width - 34))
                objectName: "settingsEmbeddingModel"
                visible: labels.length > 0
                enabled: setting.profiles.ready && !setting.profiles.busy
                labels: setting.profiles.labels
                selectedIndex: setting.profiles.selectedIndex
                displayText: selectedIndex < 0 ? qsTr("Select an embedding model") : currentText
                Accessible.name: qsTr("Embedding model")
                onActivated: function (index) {
                    setting.profiles.select(index); // qmllint disable compiler
                    picker.currentIndex = Qt.binding(function (): int {
                        return picker.selectedIndex;
                    });
                }
            }

            ActionButton {
                visible: setting.profiles.loaded && setting.profiles.labels.length === 0
                text: qsTr("No embedding model installed")
                onClicked: setting.libraryRequested()
            }

            ActionButton {
                visible: !setting.profiles.loaded && setting.profiles.feedback !== ""
                enabled: setting.profiles.ready && !setting.profiles.busy
                text: qsTr("Retry")
                onClicked: setting.profiles.refresh() // qmllint disable compiler
            }
        }
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: setting.profiles.feedback
        color: Theme.muted
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
    }
}
