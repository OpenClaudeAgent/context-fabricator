pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: page

    required property Preferences preferences
    required property HistoryPreferences history
    required property AppLanguage appLanguage
    required property EmbeddingProfiles embeddingProfiles
    signal modelLibraryRequested
    readonly property int gutter: width < 580 ? 16 : Theme.margin

    objectName: "settingsPage"
    padding: gutter
    contentWidth: availableWidth
    clip: true
    ScrollBar.vertical: ScrollBar {
        id: settingsScroll
    }

    function showAnswerHistorySettings(): void {
        if (page.contentHeight > 0)
            settingsScroll.position = Math.min(historySettings.y / page.contentHeight, 1 - settingsScroll.size);
        historySettings.forceActiveFocus();
    }

    Shortcut {
        sequences: [StandardKey.Save]
        enabled: page.visible && page.preferences.ready && !page.preferences.busy && !page.preferences.conflict
        onActivated: page.preferences.save() // qmllint disable compiler
    }

    ColumnLayout {
        objectName: "settingsColumn"
        width: Math.min(Theme.workspaceWidth, page.availableWidth)
        x: (page.availableWidth - width) / 2
        spacing: Theme.gap

        RowLayout {
            Layout.fillWidth: true
            PageTitle {
                Layout.fillWidth: true
                text: qsTr("Settings")
            }

            ActionButton {
                text: qsTr("Reload Settings")
                enabled: !page.preferences.busy
                onClicked: page.preferences.reload() // qmllint disable compiler
            }
        }

        SurfaceGroup {
            Layout.fillWidth: true
            title: qsTr("General")

            contentItem: RowLayout {
                Label {
                    Layout.fillWidth: true
                    text: qsTr("App Language")
                    wrapMode: Text.WordWrap
                }

                LocalizedComboBox {
                    Layout.preferredWidth: Math.min(184, page.availableWidth / 2)
                    objectName: "appLanguage"
                    labels: page.appLanguage.options
                    selectedIndex: page.appLanguage.selection
                    Accessible.name: qsTr("App Language")
                    onActivated: index => page.appLanguage.selection = index
                }
            }
        }

        SurfaceGroup {
            Layout.fillWidth: true
            title: qsTr("Models")

            ColumnLayout {
                anchors.fill: parent

                EmbeddingProfileSetting {
                    Layout.fillWidth: true
                    profiles: page.embeddingProfiles
                    onLibraryRequested: page.modelLibraryRequested()
                }

                SurfaceDivider {}

                RowLayout {
                    Layout.fillWidth: true

                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Local model library")
                        wrapMode: Text.WordWrap
                    }

                    ActionButton {
                        text: qsTr("Manage Models…")
                        objectName: "settingsManageModels"
                        onClicked: page.modelLibraryRequested()
                    }
                }
            }
        }

        SurfaceGroup {
            Layout.fillWidth: true
            title: qsTr("Answers")

            contentItem: RowLayout {
                enabled: page.preferences.ready && !page.preferences.busy

                Label {
                    Layout.fillWidth: true
                    text: qsTr("Answer language")
                    wrapMode: Text.WordWrap
                }

                ActionButton {
                    objectName: "answerLanguage"
                    text: page.preferences.languageName
                    Accessible.name: qsTr("Answer language")
                    onClicked: languages.open()
                }
            }
        }

        HistorySettingsGroup {
            id: historySettings

            Layout.fillWidth: true
            history: page.history
        }

        SurfaceGroup {
            Layout.fillWidth: true
            enabled: page.preferences.ready && !page.preferences.busy
            visible: page.preferences.hasArtifactLimit
            title: qsTr("Inference storage")

            ColumnLayout {
                anchors.fill: parent

                SettingsInput {
                    label: qsTr("Maximum artifact disk space")
                    fieldName: "inferenceArtifactLimit"
                    value: page.preferences.artifactLimit
                    unit: qsTr("GB")
                    onEdited: value => page.preferences.artifactLimit = value
                }
            }
        }

        SurfaceGroup {
            Layout.fillWidth: true
            enabled: page.preferences.ready && !page.preferences.busy
            title: qsTr("Diagnostics")

            ColumnLayout {
                anchors.fill: parent

                SettingsInput {
                    label: qsTr("Diagnostic retention")
                    fieldName: "diagnosticRetention"
                    value: page.preferences.retentionHours
                    unit: qsTr("hours")
                    onEdited: value => page.preferences.retentionHours = value
                }

                SurfaceDivider {
                    visible: page.preferences.hasDiagnosticLimit
                }

                SettingsInput {
                    label: qsTr("Maximum disk space")
                    fieldName: "diagnosticStorageLimit"
                    value: page.preferences.diagnosticLimit
                    unit: qsTr("MB")
                    visible: page.preferences.hasDiagnosticLimit
                    onEdited: value => page.preferences.diagnosticLimit = value
                }
            }
        }

        RowLayout {
            Layout.fillWidth: true

            Label {
                objectName: "settingsFeedback"
                Layout.fillWidth: true
                text: page.preferences.busy ? qsTr("Working") : page.preferences.feedback
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                Accessible.role: Accessible.StaticText
            }

            ActionButton {
                objectName: "saveSettings"
                text: qsTr("Save")
                highlighted: true
                enabled: page.preferences.ready && !page.preferences.busy && !page.preferences.conflict
                onClicked: page.preferences.save() // qmllint disable compiler
            }
        }
    }

    SurfaceDialog {
        id: languages

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(parent.width - 32, 340)
        height: Math.min(parent.height - 32, 360)
        modal: true
        title: qsTr("Answer language")

        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
        }

        onOpened: {
            search.clear();
            page.preferences.languageFilter = "";
            search.forceActiveFocus();
        }
        onClosed: page.preferences.languageFilter = ""

        ColumnLayout {
            anchors.fill: parent

            SurfaceInput {
                id: search

                objectName: "answerLanguageSearch"
                Layout.fillWidth: true
                placeholderText: qsTr("Search languages")
                Accessible.name: placeholderText
                onTextEdited: page.preferences.languageFilter = text
            }

            Label {
                Layout.fillWidth: true
                visible: choices.count === 0
                text: qsTr("No matching languages")
                wrapMode: Text.WordWrap
            }

            ListView {
                id: choices

                Layout.fillWidth: true
                Layout.fillHeight: true
                clip: true
                model: page.preferences.languageChoices

                delegate: ItemDelegate {
                    required property int index
                    required property string modelData

                    text: modelData
                    width: choices.width
                    highlighted: text === page.preferences.languageName
                    onClicked: {
                        page.preferences.chooseLanguage(index); // qmllint disable compiler
                        languages.close();
                    }
                }

                ScrollBar.vertical: ScrollBar {}
            }
        }
    }
}
