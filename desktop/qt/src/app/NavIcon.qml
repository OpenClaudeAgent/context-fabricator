pragma ComponentBehavior: Bound

import QtQuick

Item {
    id: icon

    enum Glyph {
        Dashboard,
        Connectors,
        Diagnostics,
        Settings,
        Answers,
        Fabric
    }

    Row {
        anchors.centerIn: parent
        spacing: 1
        visible: icon.glyph === NavIcon.Diagnostics

        Repeater {
            model: [4, 8, 12, 16, 10, 6, 3]
            delegate: Rectangle {
                required property int modelData
                y: (16 - modelData) / 2
                width: 1.5
                height: modelData
                radius: 0.75
                color: icon.ink
            }
        }
    }

    required property int glyph
    required property color ink
    implicitWidth: 20
    implicitHeight: 20
    Accessible.ignored: true

    Grid {
        anchors.centerIn: parent
        columns: 2
        spacing: 2
        visible: icon.glyph === NavIcon.Dashboard

        Repeater {
            model: 4
            delegate: Rectangle {
                required property int index
                width: 6
                height: 6
                radius: 1.5
                color: index === 0 ? icon.ink : "transparent"
                border.color: icon.ink
            }
        }
    }

    Item {
        anchors.centerIn: parent
        width: 16
        height: 15
        visible: icon.glyph === NavIcon.Answers

        Rectangle {
            width: 14
            height: 10
            radius: 3
            color: "transparent"
            border.color: icon.ink
        }
        Rectangle {
            x: 4
            y: 9
            width: 1
            height: 5
            rotation: 38
            color: icon.ink
        }
    }

    Item {
        anchors.centerIn: parent
        width: 18
        height: 18
        visible: icon.glyph === NavIcon.Connectors

        Rectangle {
            x: 2
            y: 2
            width: 10
            height: 7
            rotation: -38
            radius: 3
            color: "transparent"
            border.color: icon.ink
        }
        Rectangle {
            x: 6
            y: 9
            width: 10
            height: 7
            rotation: -38
            radius: 3
            color: "transparent"
            border.color: icon.ink
        }
        Rectangle {
            anchors.centerIn: parent
            width: 8
            height: 1.5
            rotation: 45
            color: icon.ink
        }
    }

    Column {
        anchors.centerIn: parent
        spacing: 4
        visible: icon.glyph === NavIcon.Settings

        Repeater {
            model: 3
            delegate: Rectangle {
                id: slider

                required property int index
                width: 15
                height: 1
                color: icon.ink

                Rectangle {
                    x: slider.index === 1 ? 4 : 9
                    y: -2
                    width: 3
                    height: 5
                    radius: 1
                    color: icon.ink
                }
            }
        }
    }

    Row {
        anchors.centerIn: parent
        spacing: 2
        rotation: 35
        visible: icon.glyph === NavIcon.Fabric

        Repeater {
            model: 3
            delegate: Rectangle {
                required property int index
                y: index === 1 ? 0 : 3
                width: 3
                height: index === 1 ? 15 : 9
                radius: 1.5
                color: icon.ink
            }
        }
    }
}
