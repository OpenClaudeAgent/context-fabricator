pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

ColumnLayout {
    id: section

    required property LocalRuntime runtime
    required property ModelLibrary library
    visible: runtime.hasRuntime || runtime.feedback !== ""
    spacing: 6

    LibrarySection {
        Layout.fillWidth: true
        title: qsTr("Local Runtime")
        visible: section.runtime.hasRuntime

        contentItem: ColumnLayout {
            spacing: 6

            RowLayout {
                Layout.fillWidth: true

                Label {
                    Layout.fillWidth: true
                    objectName: "runtimeEngineName"
                    text: section.runtime.engine
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                BusyIndicator {
                    Layout.preferredWidth: 20
                    Layout.preferredHeight: 20
                    visible: running
                    running: section.runtime.busy
                }
                Label {
                    objectName: "runtimeEngineBuild"
                    text: section.runtime.status
                    textFormat: Text.PlainText
                    color: Theme.muted
                    visible: text !== ""
                }
            }

            Flow {
                Layout.fillWidth: true
                Layout.preferredHeight: implicitHeight
                visible: section.runtime.canDownload || section.runtime.canImport
                spacing: 6
                layoutDirection: Qt.RightToLeft

                ActionButton {
                    visible: section.runtime.canDownload
                    text: qsTr("Download Runtime")
                    highlighted: true
                    onClicked: section.runtime.download() // qmllint disable compiler
                }
                ActionButton {
                    visible: section.runtime.canImport
                    text: qsTr("Choose ZIP…")
                    onClicked: archive.open()
                }
            }

            Label {
                Layout.fillWidth: true
                visible: section.runtime.canDownload
                text: qsTr("Source: GitHub Releases")
                color: Theme.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
            }

            Label {
                Layout.fillWidth: true
                visible: section.runtime.canImport && text !== ""
                text: section.library.runtimeImportEstimate
                color: Theme.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
            }

            Label {
                Layout.fillWidth: true
                visible: section.runtime.canDownload && text !== ""
                text: section.library.runtimeDownloadEstimate
                color: Theme.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
            }

            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: section.runtime.archiveSummary
                color: Theme.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
            }
        }
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: section.runtime.feedback
        color: Theme.muted
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
    }

    FileDialog {
        id: archive

        title: qsTr("Choose the trusted runtime release ZIP.")
        fileMode: FileDialog.OpenFile
        nameFilters: [qsTr("ZIP archives (*.zip)")]
        onAccepted: section.runtime.install(selectedFile) // qmllint disable compiler
    }
}
