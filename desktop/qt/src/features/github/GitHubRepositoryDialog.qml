import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: editor

    required property GitHubConnector github
    required property GitHubActionsActivity githubActions
    required property GitHubGraphBrowser githubGraph
    signal traceRequested(string traceId, string spanId)

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(550, parent.width - 24)
    height: Math.min(550, parent.height - 24)
    modal: true
    visible: github.draftOpen
    closePolicy: github.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
    title: github.editing ? qsTr("Repository Settings") : qsTr("Configure Repository")
    objectName: "githubRepositoryDialog"

    function loadForm(): void {
        repository.text = github.draftRepository;
        branch.text = github.draftBranch;
        branchFilter.clear();
        interval.value = github.draftInterval;
        files.checked = (github.draftFeatures & GitHubConnector.BranchFiles) !== 0;
        history.checked = (github.draftFeatures & GitHubConnector.CommitHistory) !== 0;
        actions.checked = (github.draftFeatures & GitHubConnector.Actions) !== 0;
        graph.checked = (github.draftFeatures & GitHubConnector.CodeGraph) !== 0;
    }

    function featureFlags(): int {
        return (files.checked ? GitHubConnector.BranchFiles : 0) | (history.checked ? GitHubConnector.CommitHistory : 0) | (actions.checked ? GitHubConnector.Actions : 0) | (graph.checked ? GitHubConnector.CodeGraph : 0);
    }

    onOpened: loadForm()
    onClosed: {
        githubGraph.close(); // qmllint disable compiler
        github.closeDraft(); // qmllint disable compiler
    }

    ScrollView {
        id: scroll

        anchors.fill: parent
        contentWidth: availableWidth

        ColumnLayout {
            width: scroll.availableWidth
            spacing: 8

            Label {
                Layout.fillWidth: true
                text: editor.github.feedback
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                visible: text.length > 0
            }

            Label {
                Layout.fillWidth: true
                text: editor.github.collectionFeedback
                textFormat: Text.PlainText
                wrapMode: Text.WordWrap
                visible: text.length > 0
            }

            RowLayout {
                visible: editor.github.draftNeedsReview
                enabled: !editor.github.busy

                ActionButton {
                    text: qsTr("Reload")
                    onClicked: editor.github.reload() // qmllint disable compiler
                }

                ActionButton {
                    text: qsTr("Review My Edits")
                    enabled: editor.github.draftCanReview
                    onClicked: {
                        editor.github.reviewDraft(repository.text, branch.text, interval.value, editor.featureFlags()); // qmllint disable compiler
                        editor.loadForm();
                    }
                }
            }

            Label {
                text: qsTr("Repository")
            }

            SurfaceInput {
                id: repository

                Layout.fillWidth: true
                objectName: "githubRepository"
                Accessible.name: qsTr("Repository")
                placeholderText: "https://github.com/owner/repository"
                readOnly: editor.github.editing || editor.github.busy
            }

            Label {
                text: qsTr("Branch")
            }

            SurfaceInput {
                id: branch

                Layout.fillWidth: true
                objectName: "githubBranch"
                Accessible.name: qsTr("Branch")
                placeholderText: qsTr("Default branch")
                readOnly: editor.github.busy
            }

            SurfaceInput {
                id: branchFilter

                Layout.fillWidth: true
                visible: editor.github.editing && editor.github.branchesLoaded
                enabled: !editor.github.busy && !editor.github.draftNeedsReview && editor.github.credential === GitHubConnector.Connected
                Accessible.name: qsTr("Search branches")
                placeholderText: qsTr("Search branches")
                onTextEdited: editor.github.filterBranches(text) // qmllint disable compiler
            }

            RowLayout {
                Layout.fillWidth: true
                visible: editor.github.editing
                enabled: !editor.github.busy && !editor.github.draftNeedsReview && editor.github.credential === GitHubConnector.Connected

                ComboBox {
                    Layout.fillWidth: true
                    visible: count > 0
                    Accessible.name: qsTr("Branch")
                    model: editor.github.matchingBranches
                    onActivated: index => branch.text = editor.github.matchingBranches[index]
                }

                Label {
                    Layout.fillWidth: true
                    visible: editor.github.branchesLoaded && editor.github.matchingBranches.length === 0
                    text: editor.github.branches.length === 0 ? qsTr("No branches found") : qsTr("No matching branches")
                    wrapMode: Text.WordWrap
                }

                ActionButton {
                    visible: !editor.github.branchesLoaded || editor.github.moreBranches
                    text: editor.github.moreBranches ? qsTr("Load More Branches") : qsTr("Browse Branches")
                    onClicked: editor.github.loadMoreBranches() // qmllint disable compiler
                }
            }

            Label {
                text: qsTr("Refresh interval")
            }

            SpinBox {
                id: interval

                Accessible.name: qsTr("Refresh interval")
                from: editor.github.minimumInterval
                to: editor.github.maximumInterval
                editable: true
                enabled: !editor.github.busy
            }

            GridLayout {
                Layout.fillWidth: true
                columns: scroll.availableWidth >= 390 ? 2 : 1
                columnSpacing: 14
                rowSpacing: 4
                enabled: !editor.github.busy

                CheckBox {
                    id: files
                    text: qsTr("Branch files")
                }
                CheckBox {
                    id: history
                    text: qsTr("Commit history")
                }
                CheckBox {
                    id: actions
                    objectName: "githubActionsFeature"
                    text: qsTr("GitHub Actions")
                }
                CheckBox {
                    id: graph
                    text: qsTr("Code graph")
                }
            }

            ActionButton {
                visible: editor.github.editing
                enabled: !editor.github.busy && !editor.github.draftNeedsReview && editor.github.credential === GitHubConnector.Connected
                text: qsTr("Check Access")
                onClicked: editor.github.verifyAccess() // qmllint disable compiler
            }

            Label {
                visible: editor.github.editing
                text: editor.github.draftVerified ? qsTr("Access verified") : qsTr("Access unverified")
            }

            GitHubCollectionSection {
                Layout.fillWidth: true
                collection: editor.github.draftCollection
                visible: collection.available || editor.github.canRefreshBranch
                refreshEnabled: editor.github.canRefreshBranch && !editor.github.busy
                onRefreshRequested: editor.github.refreshBranch() // qmllint disable compiler
            }

            ActionButton {
                objectName: "githubBrowseGraph"
                visible: editor.github.canBrowseGraph
                enabled: !editor.github.busy
                text: qsTr("Browse Code Graph…")
                onClicked: editor.github.browseGraph() // qmllint disable compiler
            }

            SurfaceGroup {
                Layout.fillWidth: true
                visible: editor.githubActions.active
                title: qsTr("GitHub Actions")

                ColumnLayout {
                    width: parent.width
                    spacing: 8

                    MetricRow {
                        objectName: "githubActionsNextCheck"
                        visible: value.length > 0
                        title: qsTr("Next check")
                        value: editor.githubActions.nextCheck
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: text.length > 0 && !editor.githubActions.visible
                        text: editor.githubActions.feedback
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                    }
                    Flow {
                        Layout.fillWidth: true
                        Layout.preferredHeight: implicitHeight
                        spacing: 8

                        ActionButton {
                            objectName: "githubViewActions"
                            text: qsTr("View Workflow Runs…")
                            enabled: !editor.github.busy
                            onClicked: editor.githubActions.show() // qmllint disable compiler
                        }
                        ActionButton {
                            objectName: "githubRefreshRepositoryActions"
                            text: qsTr("Refresh Now")
                            enabled: editor.githubActions.availability === GitHubActionsActivity.Ready && !editor.githubActions.busy && !editor.github.busy
                            onClicked: editor.githubActions.refresh() // qmllint disable compiler
                        }
                    }
                    GitHubCollectionTraceLinks {
                        Layout.fillWidth: true
                        Layout.preferredHeight: implicitHeight
                        feature: "actions"
                        links: editor.github.actionsTraces
                        onTraceRequested: (traceId, spanId) => editor.traceRequested(traceId, spanId)
                    }
                }
            }

            SurfaceGroup {
                Layout.fillWidth: true
                visible: editor.github.hasCommitHistory
                title: qsTr("Commit History Collection")

                ColumnLayout {
                    width: parent.width
                    spacing: 8

                    ActionButton {
                        objectName: "githubRefreshHistory"
                        text: qsTr("Refresh History")
                        enabled: editor.github.canRefreshHistory && !editor.github.busy && !editor.githubActions.busy
                        onClicked: editor.github.refreshHistory() // qmllint disable compiler
                    }
                    GitHubCollectionTraceLinks {
                        Layout.fillWidth: true
                        Layout.preferredHeight: implicitHeight
                        feature: "history"
                        links: editor.github.historyTraces
                        onTraceRequested: (traceId, spanId) => editor.traceRequested(traceId, spanId)
                    }
                }
            }

            SurfaceGroup {
                Layout.fillWidth: true
                visible: editor.github.editing
                title: qsTr("Local Repository Data")

                ColumnLayout {
                    anchors.fill: parent
                    spacing: 9

                    Label {
                        Layout.fillWidth: true
                        visible: editor.github.draftDataPaused
                        objectName: "githubDataPaused"
                        text: qsTr("Automatic collection paused")
                        color: Theme.statusWaiting
                        wrapMode: Text.WordWrap
                        Accessible.role: Accessible.Heading
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: editor.github.draftDataPaused
                        text: qsTr("Local data was deleted. Refresh an enabled source to resume collection.")
                        color: Theme.muted
                        wrapMode: Text.WordWrap
                    }
                    Label {
                        Layout.fillWidth: true
                        text: editor.github.draftDataFeedback
                        textFormat: Text.PlainText
                        visible: text.length > 0
                        wrapMode: Text.WordWrap
                    }
                    ActionButton {
                        visible: editor.github.draftDataFeedback.length > 0
                        enabled: !editor.github.busy
                        text: qsTr("Reload")
                        onClicked: editor.github.loadDraftData() // qmllint disable compiler
                    }
                    Flow {
                        Layout.fillWidth: true
                        Layout.preferredHeight: implicitHeight
                        enabled: !editor.github.busy && !editor.github.draftNeedsReview
                        spacing: 8

                        ActionButton {
                            objectName: "githubPurgeData"
                            text: qsTr("Delete Local Data…")
                            onClicked: purge.open()
                        }
                        ActionButton {
                            text: qsTr("Remove Repository")
                            onClicked: removal.open()
                        }
                    }
                }
            }
        }
    }

    footer: DialogActions {
        ActionButton {
            text: qsTr("Cancel")
            enabled: !editor.github.busy
            onClicked: editor.github.closeDraft() // qmllint disable compiler
        }

        ActionButton {
            text: qsTr("Save")
            highlighted: true
            objectName: "githubSaveRepository"
            enabled: !editor.github.busy && !editor.github.draftNeedsReview && repository.text.trim().length > 0 && editor.github.credential === GitHubConnector.Connected
            onClicked: editor.github.saveRepository(repository.text, branch.text, interval.value, editor.featureFlags()) // qmllint disable compiler
        }
    }

    GitHubGraphDialog {
        browser: editor.githubGraph
        refreshEnabled: editor.github.canRefreshBranch
        refreshing: editor.github.busy
        refreshFeedback: editor.github.feedback
        onRefreshRequested: editor.github.refreshGraphSource(editor.githubGraph.branch) // qmllint disable compiler
    }

    GitHubActionsDialog {
        github: editor.github
        activity: editor.githubActions
    }

    ConfirmationDialog {
        id: purge

        maximumWidth: 450
        title: qsTr("Delete local repository data?")
        objectName: "githubPurgeDialog"
        message: qsTr("This deletes all retained local code, history, Actions and graph data for this repository, including its other branches. Settings remain. Collection pauses until you explicitly refresh an enabled, verified feature.")

        footer: DialogActions {
            defaultButton: cancelPurge

            ActionButton {
                id: cancelPurge

                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
            ActionButton {
                objectName: "githubConfirmPurge"
                text: qsTr("Delete Local Data")
                destructive: true
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        onAccepted: editor.github.purgeRepositoryData() // qmllint disable compiler
    }

    ConfirmationDialog {
        id: removal

        title: qsTr("Remove this repository?")
        message: qsTr("This removes the repository settings from Context Fabricator.")

        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.RejectRole
            }
            ActionButton {
                text: qsTr("Remove Repository")
                destructive: true
                DialogButtonBox.buttonRole: DialogButtonBox.AcceptRole
            }
        }

        onAccepted: editor.github.removeRepository() // qmllint disable compiler
    }
}
