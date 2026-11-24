import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ItemDelegate {
    id: row

    required property gitHubRepositoryEntry entry

    objectName: "githubConfigured/" + entry.name
    text: entry.name
    Accessible.name: text
    Accessible.description: visibility.description + ", " + qsTr("Branch") + " " + entry.branch + (entry.collectionStatus.length > 0 ? ", " + entry.collectionStatus : "") + (entry.accessWarning.length > 0 ? ", " + entry.accessWarning : "")
    horizontalPadding: 12
    verticalPadding: 10
    hoverEnabled: true

    contentItem: RowLayout {
        spacing: 12

        Rectangle {
            Layout.preferredWidth: 36
            Layout.preferredHeight: 36
            radius: Theme.controlRadius
            color: Theme.raised

            GitHubRepositoryVisibility {
                id: visibility

                anchors.centerIn: parent
                objectName: "githubConfiguredVisibility/" + row.entry.name
                privateRepository: row.entry.privateRepository
            }
        }

        GridLayout {
            id: metadata

            Layout.fillWidth: true
            columns: row.width < 460 ? 1 : 2
            columnSpacing: 12
            rowSpacing: 4

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                Label {
                    Layout.fillWidth: true
                    text: row.text
                    textFormat: Text.PlainText
                    wrapMode: Text.WrapAnywhere
                    maximumLineCount: 2
                    elide: Text.ElideRight
                    font.weight: Font.Medium
                    Accessible.ignored: true
                }
                Label {
                    Layout.fillWidth: true
                    objectName: "githubConfiguredBranch/" + row.entry.name
                    text: row.entry.branch
                    textFormat: Text.PlainText
                    font.pixelSize: 12
                    elide: Text.ElideMiddle
                    color: Theme.muted
                    Accessible.ignored: true
                }
                Label {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: row.entry.collectionStatus
                    textFormat: Text.PlainText
                    font.pixelSize: 12
                    wrapMode: Text.WordWrap
                    color: Theme.muted
                    Accessible.ignored: true
                }
            }

            Label {
                Layout.fillWidth: metadata.columns === 1
                Layout.maximumWidth: metadata.columns === 1 ? -1 : 180
                Layout.alignment: metadata.columns === 1 ? Qt.AlignLeft : Qt.AlignRight | Qt.AlignVCenter
                objectName: "githubConfiguredWarning/" + row.entry.name
                visible: text.length > 0
                text: row.entry.accessWarning
                textFormat: Text.PlainText
                font.pixelSize: 12
                wrapMode: Text.WordWrap
                color: Theme.statusWaiting
                Accessible.ignored: true
            }
        }

        Label {
            text: "›"
            color: Theme.muted
            font.pixelSize: 20
            Accessible.ignored: true
        }
    }

    background: Rectangle {
        radius: Theme.controlRadius
        color: row.hovered || row.down ? Theme.hovered : "transparent"
        border.color: row.visualFocus ? Theme.accent : "transparent"
        border.width: row.visualFocus ? 2 : 0
    }
}
