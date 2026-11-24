import QtQuick
import QtQuick.Controls

TextField {
    id: field

    padding: 10
    color: field.enabled ? Theme.text : Theme.disabled
    placeholderTextColor: Theme.muted
    selectionColor: Theme.primary
    selectedTextColor: Theme.accentText

    background: Rectangle {
        radius: Theme.controlRadius
        color: Theme.field
        border.color: field.activeFocus ? Theme.accent : Theme.outline
        border.width: field.activeFocus ? 2 : 1
    }
}
