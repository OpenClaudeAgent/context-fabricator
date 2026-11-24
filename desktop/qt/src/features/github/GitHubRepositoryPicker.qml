pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: picker

    required property GitHubConnector github
    property bool enteringUrl: false
    property string selectedRepository
    readonly property int selectedIndex: github.candidateList.names.indexOf(selectedRepository)
    readonly property bool canContinue: !github.busy && (enteringUrl ? repositoryUrl.text.trim().length > 0 : selectedIndex >= 0)

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(540, parent.width - 24)
    height: Math.min(parent.height - 24, enteringUrl ? implicitHeight : 450)
    modal: true
    closePolicy: github.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
    title: qsTr("Add Repository")

    function continueSelection(): void {
        if (!canContinue)
            return;
        if (enteringUrl) {
            github.addRepository(repositoryUrl.text); // qmllint disable compiler
        } else {
            github.chooseCandidate(selectedIndex); // qmllint disable compiler
        }
        if (github.draftOpen)
            close();
    }

    function moveSelection(offset: int): void {
        if (github.busy || candidates.count === 0)
            return;
        const next = selectedIndex < 0 ? (offset > 0 ? 0 : candidates.count - 1) : Math.max(0, Math.min(selectedIndex + offset, candidates.count - 1));
        selectedRepository = github.candidateList.names[next];
        candidates.positionViewAtIndex(next, ListView.Contain);
    }

    onOpened: {
        enteringUrl = false;
        selectedRepository = "";
        repositoryUrl.clear();
        search.clear();
        github.filterCandidates(""); // qmllint disable compiler
        github.discover(); // qmllint disable compiler
        search.forceActiveFocus();
    }

    contentItem: ColumnLayout {
        spacing: 8

        SurfaceInput {
            id: search

            Layout.fillWidth: true
            objectName: "githubCandidateSearch"
            visible: !picker.enteringUrl
            Accessible.name: placeholderText
            placeholderText: qsTr("Search loaded repositories")
            onTextEdited: picker.github.filterCandidates(text) // qmllint disable compiler
            onAccepted: picker.continueSelection()
            Keys.onDownPressed: picker.moveSelection(1)
            Keys.onUpPressed: picker.moveSelection(-1)
        }

        Rectangle {
            Layout.fillWidth: true
            Layout.fillHeight: true
            Layout.preferredHeight: 206
            visible: !picker.enteringUrl
            color: Theme.field
            border.color: Theme.outline
            radius: Theme.controlRadius

            ListView {
                id: candidates

                anchors.fill: parent
                anchors.margins: 1
                clip: true
                model: picker.github.candidateList
                currentIndex: picker.selectedIndex
                keyNavigationEnabled: false
                Keys.onDownPressed: picker.moveSelection(1)
                Keys.onUpPressed: picker.moveSelection(-1)
                Keys.onReturnPressed: picker.continueSelection()

                delegate: ItemDelegate {
                    id: candidateRow

                    required property gitHubRepositoryEntry entry

                    objectName: "githubCandidate/" + entry.name
                    width: candidates.width
                    text: entry.name
                    Accessible.name: text
                    Accessible.description: visibility.description
                    enabled: !picker.github.busy
                    highlighted: picker.selectedRepository === entry.name
                    onClicked: picker.selectedRepository = entry.name

                    contentItem: RowLayout {
                        spacing: 8

                        GitHubRepositoryVisibility {
                            id: visibility

                            objectName: "githubCandidateVisibility/" + candidateRow.entry.name
                            privateRepository: candidateRow.entry.privateRepository
                        }
                        Label {
                            Layout.fillWidth: true
                            text: candidateRow.text
                            textFormat: Text.PlainText
                            elide: Text.ElideRight
                            color: candidateRow.highlighted ? Theme.accent : Theme.text
                            Accessible.ignored: true
                        }
                    }
                }

                ScrollBar.vertical: ScrollBar {}
            }

            Label {
                anchors.centerIn: parent
                width: parent.width - 16
                visible: candidates.count === 0
                objectName: "githubNoCandidates"
                text: picker.github.busy ? qsTr("Loading repositories…") : (search.text.trim().length > 0 ? qsTr("No repositories match this filter.") : qsTr("No repositories in the loaded list."))
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.WordWrap
                color: Theme.muted
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: !picker.enteringUrl && (picker.github.moreCandidates || candidates.count === 0)

            ActionButton {
                visible: candidates.count === 0
                text: qsTr("Reload Repositories")
                enabled: !picker.github.busy
                onClicked: picker.github.discover() // qmllint disable compiler
            }

            Item {
                Layout.fillWidth: true
            }

            ActionButton {
                objectName: "githubMoreCandidates"
                visible: picker.github.moreCandidates
                text: qsTr("Load More")
                enabled: !picker.github.busy
                onClicked: picker.github.loadMoreCandidates() // qmllint disable compiler
            }
        }

        SurfaceInput {
            id: repositoryUrl

            Layout.fillWidth: true
            objectName: "githubCandidateUrl"
            visible: picker.enteringUrl
            Accessible.name: placeholderText
            placeholderText: qsTr("Repository URL or owner/name")
            onAccepted: picker.continueSelection()
        }

        Label {
            Layout.fillWidth: true
            visible: picker.enteringUrl
            text: qsTr("Use a GitHub repository URL or enter owner/name.")
            color: Theme.muted
            wrapMode: Text.WordWrap
        }

        ActionButton {
            objectName: "githubCandidateUrlMode"
            text: picker.enteringUrl ? qsTr("Browse Repositories") : qsTr("Use a Repository URL…")
            onClicked: {
                picker.enteringUrl = !picker.enteringUrl;
                if (picker.enteringUrl)
                    repositoryUrl.forceActiveFocus();
                else
                    search.forceActiveFocus();
            }
        }

        Label {
            Layout.fillWidth: true
            visible: text.length > 0
            text: picker.github.feedback
            textFormat: Text.PlainText
            wrapMode: Text.WordWrap
        }
    }

    footer: DialogActions {
        ActionButton {
            text: qsTr("Cancel")
            enabled: !picker.github.busy
            onClicked: picker.close()
        }

        ActionButton {
            objectName: "githubContinueRepository"
            text: qsTr("Continue")
            highlighted: true
            enabled: picker.canContinue
            onClicked: picker.continueSelection()
        }
    }
}
