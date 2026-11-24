pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ItemDelegate {
    id: row

    required property bool compact
    required property string title
    required property string observedAt
    property string detail
    property string problem
    property string value
    property string classification
    property color classificationColor: Theme.muted
    property string status
    property bool reserveStatus: false
    property bool canOpen: false

    padding: 8
    horizontalPadding: 12
    focusPolicy: canOpen ? Qt.StrongFocus : Qt.NoFocus
    hoverEnabled: canOpen
    Accessible.role: canOpen ? Accessible.Button : Accessible.StaticText
    Accessible.name: [title, observedAt, detail, problem, value, classification, status].filter(function (part: string): bool {
        return part !== "";
    }).join(" · ")
    ToolTip.visible: canOpen && hovered
    ToolTip.text: qsTr("View Trace")
    ToolTip.delay: 650

    background: Item {
        Rectangle {
            anchors.fill: parent
            color: row.hovered ? Theme.hovered : "transparent"
            border.color: Theme.accent
            border.width: row.visualFocus ? 2 : 0
            radius: Theme.controlRadius
        }
        Rectangle {
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.bottom: parent.bottom
            height: 1
            color: Theme.outline
        }
    }

    contentItem: GridLayout {
        columns: row.compact ? 1 : 3
        rowSpacing: 5
        columnSpacing: 16

        Label {
            Layout.preferredWidth: row.compact ? -1 : 144
            text: row.observedAt
            elide: Text.ElideRight
            color: Theme.muted
            font.pixelSize: 12
            Accessible.ignored: true
        }
        ColumnLayout {
            Layout.fillWidth: true
            spacing: 2

            Label {
                Layout.fillWidth: true
                text: row.title
                textFormat: Text.PlainText
                color: Theme.text
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
                Accessible.ignored: true
            }
            Label {
                Layout.fillWidth: true
                visible: text !== "" && text !== row.title
                text: row.detail
                textFormat: Text.PlainText
                color: Theme.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
                Accessible.ignored: true
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: row.problem
                textFormat: Text.PlainText
                color: Theme.muted
                font.pixelSize: 12
                wrapMode: Text.Wrap
                Accessible.ignored: true
            }
        }
        RowLayout {
            Layout.alignment: row.compact ? Qt.AlignLeft : Qt.AlignRight
            visible: row.value !== "" || row.classification !== "" || row.status !== "" || row.canOpen
            spacing: 8

            Label {
                text: row.value
                visible: text !== ""
                color: Theme.text
                Accessible.ignored: true
            }
            Label {
                text: row.classification
                visible: text !== ""
                color: row.classificationColor
                font.pixelSize: 12
                Accessible.ignored: true
            }
            Label {
                text: row.status
                visible: text !== "" || (row.reserveStatus && !row.compact)
                Layout.preferredWidth: row.compact ? -1 : 70
                horizontalAlignment: Text.AlignRight
                color: Theme.muted
                font.pixelSize: 12
                Accessible.ignored: true
            }
            NavIcon {
                visible: row.canOpen
                glyph: NavIcon.Diagnostics
                ink: row.hovered || row.visualFocus ? Theme.accent : Theme.muted
            }
        }
    }
}
