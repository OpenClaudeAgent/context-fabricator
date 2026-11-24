pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: form

    required property GitHubConnector github
    property bool accountManagement: false
    signal disconnectRequested

    spacing: 10

    StatusChip {
        Layout.maximumWidth: parent.width
        ready: form.github.credential === GitHubConnector.Connected
        text: form.github.credential === GitHubConnector.ReconnectRequired ? qsTr("Reconnect GitHub") : (form.github.login || qsTr("Not connected"))
    }

    ActionButton {
        text: qsTr("Create Token")
        onClicked: tokenMenu.open()

        Menu {
            id: tokenMenu

            MenuItem {
                text: qsTr("Repository contents")
                onTriggered: form.github.openTokenPage(false) // qmllint disable compiler
            }

            MenuItem {
                text: qsTr("Repository contents and Actions")
                onTriggered: form.github.openTokenPage(true) // qmllint disable compiler
            }
        }
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
