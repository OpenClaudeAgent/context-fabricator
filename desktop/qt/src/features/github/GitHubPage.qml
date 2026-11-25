pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Item {
    id: page

    required property GitHubConnector github
    required property GitHubActionsActivity githubActions
    required property GitHubGraphBrowser githubGraph
    signal traceRequested(string traceId, string spanId)
    signal backRequested
    readonly property bool connected: github.credential === GitHubConnector.Connected
    readonly property string filter: configuredFilter.text.trim().toLowerCase()
    readonly property bool hasMatchingRepository: github.repositoryList.names.some(page.matchesFilter)

    function matchesFilter(repository: string): bool {
        return repository.toLowerCase().includes(page.filter);
    }

    function focusBack(): void {
        back.forceActiveFocus();
    }

    Shortcut {
        sequences: [StandardKey.Find]
        enabled: page.visible && configuredFilter.visible && !page.github.draftOpen && !picker.visible && !account.visible && !usage.visible
        onActivated: {
            configuredFilter.forceActiveFocus();
            configuredFilter.selectAll();
        }
    }

    ScrollView {
        id: scroll

        anchors.fill: parent
        padding: width < 580 ? 16 : Theme.margin
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            objectName: "githubWorkspaceColumn"
            width: Math.min(Theme.workspaceWidth, scroll.availableWidth)
            x: (scroll.availableWidth - width) / 2
            spacing: Theme.gap

            ActionButton {
                id: back

                objectName: "githubBack"
                text: qsTr("Back to Connectors")
                onClicked: page.backRequested()
            }

            RowLayout {
                Layout.fillWidth: true

                PageTitle {
                    Layout.fillWidth: true
                    text: qsTr("GitHub")
                }

                ActionButton {
                    text: qsTr("Reload")
                    enabled: !page.github.busy
                    onClicked: page.github.reload() // qmllint disable compiler
                }
            }

            Flow {
                Layout.fillWidth: true
                Layout.preferredHeight: implicitHeight
                visible: page.github.ready
                spacing: 8

                StatusChip {
                    ready: page.connected
                    text: page.github.credential === GitHubConnector.ReconnectRequired ? qsTr("Reconnect GitHub") : (page.github.login || qsTr("Not connected"))
                }
                ActionButton {
                    objectName: "githubAccount"
                    visible: page.connected || page.github.repositoryList.count > 0
                    text: page.connected ? qsTr("Account…") : qsTr("Connect to GitHub…")
                    enabled: !page.github.busy
                    onClicked: account.open()
                }
                ActionButton {
                    objectName: "githubRequestsStorage"
                    text: qsTr("Requests & Storage…")
                    enabled: !page.github.busy
                    onClicked: usage.open()
                }
            }

            Label {
                Layout.fillWidth: true
                visible: page.github.busy && !page.github.ready
                text: qsTr("Loading GitHub…")
                color: Theme.muted
                wrapMode: Text.WordWrap
            }

            Label {
                Layout.fillWidth: true
                text: page.github.feedback
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                visible: text.length > 0 && !page.github.draftOpen && !account.visible && !picker.visible
            }

            Label {
                Layout.fillWidth: true
                text: page.github.collectionFeedback
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                visible: text.length > 0 && !page.github.draftOpen
            }

            SurfaceGroup {
                Layout.fillWidth: true
                visible: page.github.ready
                title: page.connected || page.github.repositoryList.count > 0 ? qsTr("Repositories") : qsTr("Connect to GitHub")

                ColumnLayout {
                    width: parent.width

                    Label {
                        Layout.fillWidth: true
                        visible: page.github.repositoryList.count === 0
                        text: page.connected ? qsTr("Add a repository to choose its branch and what to collect.") : qsTr("Choose the repositories Context Fabricator can read.")
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                    }

                    ActionButton {
                        objectName: "githubConnect"
                        visible: !page.connected && page.github.repositoryList.count === 0
                        text: qsTr("Connect to GitHub…")
                        highlighted: true
                        enabled: !page.github.busy
                        onClicked: account.open()
                    }

                    SurfaceInput {
                        id: configuredFilter

                        Layout.fillWidth: true
                        objectName: "githubConfiguredFilter"
                        visible: page.github.repositoryList.count > 0
                        Accessible.name: placeholderText
                        placeholderText: qsTr("Filter configured repositories")
                    }

                    Label {
                        Layout.fillWidth: true
                        visible: configuredFilter.visible && !page.hasMatchingRepository
                        text: qsTr("No repositories match this filter.")
                        objectName: "githubNoConfiguredMatches"
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                    }

                    Repeater {
                        model: page.github.repositoryList

                        GitHubConfiguredRepositoryRow {
                            id: repositoryRow

                            required property int index

                            Layout.fillWidth: true
                            visible: page.matchesFilter(repositoryRow.entry.name)
                            enabled: !page.github.busy
                            onClicked: page.github.editRepository(index) // qmllint disable compiler
                        }
                    }

                    ActionButton {
                        objectName: "githubAddRepository"
                        visible: page.connected
                        enabled: !page.github.busy
                        text: qsTr("Add Repository…")
                        highlighted: page.github.repositoryList.count === 0
                        onClicked: picker.open()
                    }
                }
            }
        }
    }

    SurfaceDialog {
        id: account

        property bool repositoryScoped: false

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(500, parent.width - 24)
        modal: true
        closePolicy: page.github.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
        title: qsTr("GitHub Account")
        onClosed: repositoryScoped = false

        contentItem: ColumnLayout {
            spacing: Theme.gap

            GitHubAccountForm {
                Layout.fillWidth: true
                enabled: !page.github.busy
                github: page.github
                accountManagement: true
                repositoryScoped: account.repositoryScoped
                onDisconnectRequested: disconnect.open()
            }

            Label {
                Layout.fillWidth: true
                visible: text.length > 0
                text: page.github.feedback
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
            }
        }

        footer: DialogActions {
            ActionButton {
                objectName: "githubAccountDone"
                text: qsTr("Done")
                enabled: !page.github.busy
                onClicked: account.close()
            }
        }
    }

    ConfirmationDialog {
        id: disconnect

        title: qsTr("Disconnect GitHub?")
        message: qsTr("Your saved repository settings will be available when you reconnect.")

        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
            ActionButton {
                text: qsTr("Disconnect")
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        onAccepted: page.github.disconnectAccount() // qmllint disable compiler
    }

    GitHubRepositoryPicker {
        id: picker

        github: page.github
    }

    GitHubRepositoryDialog {
        github: page.github
        githubActions: page.githubActions
        githubGraph: page.githubGraph
        onManageTokenRequested: {
            account.repositoryScoped = true;
            account.open();
        }
        onTraceRequested: (traceId, spanId) => page.traceRequested(traceId, spanId)
    }

    GitHubUsageDialog {
        id: usage

        github: page.github
    }
}
