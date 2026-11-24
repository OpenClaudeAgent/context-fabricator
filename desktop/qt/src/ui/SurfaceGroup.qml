import QtQuick
import QtQuick.Controls

GroupBox {
    id: group

    padding: Theme.gap
    topPadding: group.title.length > 0 ? 42 : Theme.gap
    spacing: Theme.gap

    label: Label {
        x: group.leftPadding
        y: 13
        width: group.availableWidth
        text: group.title
        elide: Text.ElideRight
        color: Theme.text
        font.pixelSize: 15
        font.weight: Font.DemiBold
        Accessible.role: Accessible.Heading
    }

    background: Rectangle {
        radius: Theme.radius
        color: Theme.surface
        border.color: Theme.outline
    }
}
