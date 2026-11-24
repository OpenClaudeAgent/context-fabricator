import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

StackLayout {
    id: page

    required property GitHubConnector github
    required property GitHubActionsActivity githubActions
    required property GitHubGraphBrowser githubGraph
    signal traceRequested(string traceId, string spanId)
    readonly property bool managing: currentIndex === 1
    readonly property bool managementVisible: visible && managing

    currentIndex: 0
    onManagementVisibleChanged: github.setCollectionVisible(managementVisible) // qmllint disable compiler

    ScrollView {
        id: directory

        padding: width < 580 ? 16 : Theme.margin
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            objectName: "connectorDirectoryColumn"
            width: Math.min(Theme.workspaceWidth, directory.availableWidth)
            x: (directory.availableWidth - width) / 2
            spacing: Theme.gap

            PageTitle {
                Layout.fillWidth: true
                text: qsTr("Connectors")
            }

            ItemDelegate {
                id: manage

                Layout.fillWidth: true
                objectName: "githubManage"
                text: qsTr("GitHub")
                Accessible.description: description.text
                horizontalPadding: 14
                verticalPadding: 12
                hoverEnabled: true
                onClicked: {
                    page.currentIndex = 1;
                    management.focusBack();
                }

                contentItem: RowLayout {
                    spacing: 14

                    Rectangle {
                        Layout.preferredWidth: 36
                        Layout.preferredHeight: 36
                        radius: Theme.controlRadius
                        color: Theme.raised

                        Label {
                            anchors.centerIn: parent
                            text: "</>"
                            textFormat: Text.PlainText
                            color: Theme.text
                            font.family: Theme.fixedFont
                            font.weight: Font.DemiBold
                            Accessible.ignored: true
                        }
                    }

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 3

                        Label {
                            Layout.fillWidth: true
                            text: manage.text
                            color: Theme.text
                            font.weight: Font.DemiBold
                        }
                        Label {
                            id: description

                            Layout.fillWidth: true
                            text: qsTr("Repository access")
                            color: Theme.muted
                            wrapMode: Text.WordWrap
                        }
                    }

                    Label {
                        text: "›"
                        color: Theme.muted
                        font.pixelSize: 22
                        Accessible.ignored: true
                    }
                }

                background: Rectangle {
                    radius: Theme.radius
                    color: manage.hovered || manage.down ? Theme.hovered : Theme.surface
                    border.color: manage.visualFocus ? Theme.accent : Theme.outline
                    border.width: manage.visualFocus ? 2 : 1
                }
            }
        }
    }

    GitHubPage {
        id: management

        github: page.github
        githubActions: page.githubActions
        githubGraph: page.githubGraph
        onBackRequested: {
            page.currentIndex = 0;
            manage.forceActiveFocus();
        }
        onTraceRequested: (traceId, spanId) => page.traceRequested(traceId, spanId)
    }
}
