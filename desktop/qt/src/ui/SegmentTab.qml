import QtQuick
import QtQuick.Controls

TabButton {
    id: tab

    implicitHeight: 30
    font.weight: checked ? Font.DemiBold : Font.Medium

    contentItem: Label {
        text: tab.text
        font: tab.font
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
        elide: Text.ElideRight
        color: tab.checked ? Theme.accentText : Theme.text
    }

    background: Rectangle {
        radius: Theme.controlRadius
        color: tab.checked ? Theme.primary : tab.hovered ? Theme.hovered : "transparent"
        border.width: tab.visualFocus ? 2 : 0
        border.color: Theme.accent
    }
}
