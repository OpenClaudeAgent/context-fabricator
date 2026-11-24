pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls

ComboBox {
    id: control

    required property list<string> labels
    required property int selectedIndex

    model: ListModel {
        id: entries
    }
    textRole: "label"
    currentIndex: selectedIndex < count ? selectedIndex : -1
    palette.dark: Theme.muted
    hoverEnabled: true

    background: Rectangle {
        implicitWidth: 140
        implicitHeight: 36
        radius: Theme.controlRadius
        color: !control.enabled ? Theme.field : control.hovered || control.down ? Theme.hovered : Theme.raised
        border.color: control.visualFocus ? Theme.accent : Theme.outline
        border.width: control.visualFocus ? 2 : 1
    }

    onLabelsChanged: updateLabels()
    Component.onCompleted: updateLabels()

    function updateLabels(): void {
        for (let index = 0; index < control.labels.length; ++index) {
            if (index < entries.count)
                entries.setProperty(index, "label", control.labels[index]);
            else
                entries.append({
                    "label": control.labels[index]
                });
        }
        if (entries.count > control.labels.length)
            entries.remove(control.labels.length, entries.count - control.labels.length);
    }
}
