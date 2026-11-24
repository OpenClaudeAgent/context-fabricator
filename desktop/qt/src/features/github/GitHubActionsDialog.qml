pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property GitHubConnector github
    required property GitHubActionsActivity activity
    readonly property bool canCheck: activity.availability === GitHubActionsActivity.AccessRequired && github.credential === GitHubConnector.Connected

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(880, parent.width - 24)
    height: Math.min(activity.hasSnapshot ? (width >= 700 ? 470 : 620) : 285, parent.height - 24)
    modal: true
    visible: activity.visible
    closePolicy: Popup.CloseOnEscape
    title: qsTr("GitHub Actions")
    objectName: "githubActionsDialog"

    onOpened: attemptsView.positionViewAtBeginning()
    onClosed: activity.close() // qmllint disable compiler

    ColumnLayout {
        anchors.fill: parent
        spacing: Theme.gap

        Label {
            Layout.fillWidth: true
            text: "%1 · %2".arg(dialog.activity.repository).arg(dialog.activity.branch)
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: Theme.muted
        }
        Label {
            Layout.fillWidth: true
            visible: text.length > 0
            text: dialog.activity.feedback || (dialog.canCheck ? dialog.github.feedback : "")
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
        }
        Label {
            Layout.fillWidth: true
            visible: dialog.activity.busy && !dialog.activity.hasSnapshot
            text: qsTr("Loading Actions…")
            wrapMode: Text.WordWrap
        }
        Label {
            Layout.fillWidth: true
            visible: text.length > 0 && !dialog.activity.busy
            text: dialog.availabilityMessage()
            color: Theme.muted
            wrapMode: Text.WordWrap
        }

        Label {
            visible: dialog.activity.hasSnapshot
            text: qsTr("Workflows")
            font.weight: Font.DemiBold
            Accessible.role: Accessible.Heading
        }
        LocalizedComboBox {
            Layout.fillWidth: true
            Layout.maximumWidth: 440
            visible: dialog.activity.hasSnapshot
            labels: dialog.activity.workflows
            selectedIndex: dialog.activity.workflowIndex
            objectName: "githubActionsWorkflow"
            Accessible.name: qsTr("Workflows")
            onActivated: function (index) {
                dialog.activity.selectWorkflow(index); // qmllint disable compiler
                attemptsView.positionViewAtBeginning();
            }
        }

        RowLayout {
            Layout.fillWidth: true
            visible: dialog.activity.hasSnapshot

            Label {
                Layout.fillWidth: true
                text: qsTr("Recent Runs")
                font.weight: Font.DemiBold
                Accessible.role: Accessible.Heading
            }
            Label {
                text: qsTr("%1 observed").arg(attemptsView.count)
                color: Theme.muted
            }
        }
        Label {
            Layout.fillWidth: true
            visible: dialog.activity.hasSnapshot && attemptsView.count === 0
            text: qsTr("There are no observed runs for this selection.")
            color: Theme.muted
            wrapMode: Text.WordWrap
        }
        ListView {
            id: attemptsView

            Layout.fillWidth: true
            Layout.fillHeight: true
            clip: true
            spacing: 8
            model: dialog.activity.runs
            ScrollBar.vertical: ScrollBar {
                id: attemptsScroll
            }

            delegate: GitHubActionsRunRow {
                required property gitHubActionsRun modelData

                width: attemptsView.width - attemptsScroll.width
                observation: modelData
            }
        }
    }

    function availabilityMessage(): string {
        switch (activity.availability) {
        case GitHubActionsActivity.Ready:
            return activity.hasSnapshot ? "" : qsTr("Workflow runs will appear here after collection.");
        case GitHubActionsActivity.Disabled:
            return qsTr("Actions collection is off");
        case GitHubActionsActivity.AccessRequired:
            return github.draftVerified ? qsTr("GitHub requires Contents and Actions read access for this repository.") : qsTr("Repository access needs to be checked with the current token.");
        case GitHubActionsActivity.ReconnectRequired:
            return qsTr("Reconnect GitHub to collect Actions");
        default:
            return qsTr("Actions Unavailable");
        }
    }

    footer: DialogActions {
        ActionButton {
            objectName: "githubActionsBack"
            text: qsTr("Back")
            onClicked: dialog.activity.close() // qmllint disable compiler
        }
        ActionButton {
            objectName: "githubActionsPrimary"
            enabled: !dialog.activity.busy && !dialog.github.busy && (dialog.activity.availability !== GitHubActionsActivity.AccessRequired || dialog.canCheck)
            text: dialog.actionLabel()
            highlighted: dialog.activity.availability !== GitHubActionsActivity.Unknown
            onClicked: {
                switch (dialog.activity.availability) {
                case GitHubActionsActivity.Ready:
                    dialog.activity.refresh(); // qmllint disable compiler
                    break;
                case GitHubActionsActivity.AccessRequired:
                    dialog.github.verifyAccess(); // qmllint disable compiler
                    break;
                case GitHubActionsActivity.ReconnectRequired:
                    reconnect.open();
                    break;
                default:
                    dialog.activity.reload(); // qmllint disable compiler
                }
            }
        }
    }

    function actionLabel(): string {
        switch (activity.availability) {
        case GitHubActionsActivity.Ready:
            return qsTr("Refresh Now");
        case GitHubActionsActivity.AccessRequired:
            return qsTr("Check Access");
        case GitHubActionsActivity.ReconnectRequired:
            return qsTr("Reconnect GitHub…");
        default:
            return qsTr("Reload");
        }
    }

    SurfaceDialog {
        id: reconnect

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(440, parent.width - 24)
        implicitHeight: implicitHeaderHeight + implicitFooterHeight + topPadding + bottomPadding + connection.implicitHeight + Theme.gap
        modal: true
        title: qsTr("GitHub Account")
        closePolicy: dialog.github.busy ? Popup.NoAutoClose : Popup.CloseOnEscape
        objectName: "githubActionsReconnectDialog"

        Connections {
            target: dialog.activity

            function onChanged(): void {
                if (reconnect.visible && dialog.activity.availability !== GitHubActionsActivity.ReconnectRequired && dialog.activity.availability !== GitHubActionsActivity.Unknown)
                    reconnect.close();
            }
        }

        ColumnLayout {
            id: connection

            width: parent.width
            spacing: Theme.gap

            Label {
                Layout.fillWidth: true
                visible: text.length > 0
                text: dialog.github.feedback
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
            }

            GitHubAccountForm {
                Layout.fillWidth: true
                github: dialog.github
                enabled: !dialog.github.busy
            }
        }

        footer: DialogActions {
            ActionButton {
                text: qsTr("Back")
                enabled: !dialog.github.busy
                onClicked: reconnect.close()
            }
        }
    }
}
