pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: form

    required property GitHubConnector github
    property bool accountManagement: false
    property bool repositoryScoped: false
    signal disconnectRequested

    spacing: 10

    StatusChip {
        Layout.maximumWidth: parent.width
        ready: form.github.credential === GitHubConnector.Connected
        text: form.github.credential === GitHubConnector.ReconnectRequired ? qsTr("Reconnect GitHub") : (form.github.login || qsTr("Not connected"))
    }

    Flow {
        Layout.fillWidth: true
        Layout.preferredHeight: implicitHeight
        layoutDirection: Qt.RightToLeft
        spacing: 8

        ActionButton {
            text: qsTr("Create Classic Token")
            highlighted: true
            onClicked: form.github.openClassicTokenPage(false) // qmllint disable compiler
        }

        ActionButton {
            text: qsTr("Fine-Grained Token")
            onClicked: tokenMenu.open()

            Menu {
                id: tokenMenu

                MenuItem {
                    text: qsTr("Repository contents")
                    onTriggered: form.github.openTokenPage(false, form.repositoryScoped) // qmllint disable compiler
                }

                MenuItem {
                    text: qsTr("Repository contents and Actions")
                    onTriggered: form.github.openTokenPage(true, form.repositoryScoped) // qmllint disable compiler
                }
            }
        }

        ActionButton {
            text: qsTr("Configure SSO")
            onClicked: form.github.openClassicTokenPage(true) // qmllint disable compiler
        }
    }

    Label {
        Layout.fillWidth: true
        text: qsTr("For private repositories, select the repo scope on GitHub.")
        wrapMode: Text.WordWrap
        color: Theme.muted
    }

    Label {
        Layout.fillWidth: true
        text: qsTr("If your organization uses SAML, use Configure SSO to authorize the classic token.")
        wrapMode: Text.WordWrap
        color: Theme.muted
    }

    Label {
        Layout.fillWidth: true
        text: qsTr("For a fine-grained token, select the repository and its owner on GitHub.")
        wrapMode: Text.WordWrap
        color: Theme.muted
    }

    Label {
        Layout.fillWidth: true
        visible: form.github.credential === GitHubConnector.Connected
        text: qsTr("Replacing the token can affect access to repositories under other owners.")
        wrapMode: Text.WordWrap
        color: Theme.muted
    }

    SurfaceInput {
        id: token

        Layout.fillWidth: true
        objectName: "githubToken"
        Accessible.name: qsTr("Personal access token")
        placeholderText: qsTr("Personal access token")
        echoMode: TextInput.Password
        inputMethodHints: Qt.ImhSensitiveData | Qt.ImhNoPredictiveText | Qt.ImhNoAutoUppercase
        onAccepted: activate.click()
    }

    Flow {
        Layout.fillWidth: true
        Layout.preferredHeight: implicitHeight
        spacing: 8

        ActionButton {
            id: activate

            objectName: "githubAuthorize"
            text: form.github.credential === GitHubConnector.Connected ? qsTr("Replace Token") : qsTr("Connect")
            highlighted: true
            enabled: token.length > 0
            onClicked: {
                const value = token.text;
                token.clear();
                form.github.authorize(value); // qmllint disable compiler
            }
        }

        ActionButton {
            text: qsTr("Disconnect…")
            visible: form.accountManagement && form.github.credential !== GitHubConnector.Disconnected
            onClicked: form.disconnectRequested()
        }
    }

    onVisibleChanged: {
        if (!visible)
            token.clear();
    }
}
