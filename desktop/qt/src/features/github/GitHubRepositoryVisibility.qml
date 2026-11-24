import QtQuick

Item {
    id: glyph

    required property bool privateRepository
    readonly property string description: privateRepository ? qsTr("Private repository") : qsTr("Public repository")

    implicitWidth: 20
    implicitHeight: 20
    Accessible.ignored: true

    Item {
        anchors.centerIn: parent
        width: 16
        height: 16
        visible: glyph.privateRepository

        Rectangle {
            x: 4
            y: 1
            width: 8
            height: 10
            radius: 4
            color: "transparent"
            border.color: Theme.muted
        }
        Rectangle {
            x: 2
            y: 6
            width: 12
            height: 9
            radius: 2
            color: Theme.raised
            border.color: Theme.muted
        }
    }

    Item {
        anchors.centerIn: parent
        width: 16
        height: 16
        visible: !glyph.privateRepository

        Rectangle {
            x: 2
            y: 1
            width: 12
            height: 14
            radius: 2
            color: "transparent"
            border.color: Theme.muted
        }
        Rectangle {
            x: 5
            y: 2
            width: 1
            height: 12
            color: Theme.muted
        }
        Rectangle {
            x: 7
            y: 5
            width: 4
            height: 1
            color: Theme.muted
        }
    }
}
