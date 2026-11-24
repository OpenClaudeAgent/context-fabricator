import QtQuick
import QtQuick.Controls

Button {
    id: control

    property bool destructive: false

    horizontalPadding: 12
    verticalPadding: 7
    implicitHeight: Math.max(32, implicitContentHeight + topPadding + bottomPadding)
    font.weight: Font.Medium
    hoverEnabled: true

    contentItem: Label {
        text: control.text
        font: control.font
        color: !control.enabled ? Theme.disabled : control.destructive ? Theme.danger : control.highlighted ? Theme.accentText : Theme.text
        elide: Text.ElideRight
        horizontalAlignment: Text.AlignHCenter
        verticalAlignment: Text.AlignVCenter
    }

    background: Rectangle {
        radius: Theme.controlRadius
        color: !control.enabled ? Theme.field : control.highlighted ? Theme.primary : control.hovered || control.down ? Theme.hovered : Theme.raised
        border.color: control.visualFocus ? Theme.accent : control.highlighted && control.enabled ? Theme.primary : Theme.outline
        border.width: control.visualFocus ? 2 : 1
    }
}
