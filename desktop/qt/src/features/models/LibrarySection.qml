import QtQuick
import QtQuick.Controls

GroupBox {
    id: group

    readonly property real headingHeight: heading.implicitHeight + 6
    padding: 12
    topPadding: headingHeight + padding

    label: Label {
        id: heading

        x: 0
        y: 0
        width: group.width
        text: group.title
        elide: Text.ElideRight
        color: Theme.text
        font.weight: Font.DemiBold
        Accessible.role: Accessible.Heading
    }

    background: Rectangle {
        y: group.headingHeight
        width: group.width
        height: Math.max(0, group.height - y)
        color: Theme.surface
        border.color: Theme.outline
        radius: Theme.radius
    }
}
