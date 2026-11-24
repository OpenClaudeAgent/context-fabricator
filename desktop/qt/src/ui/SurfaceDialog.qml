import QtQuick
import QtQuick.Controls

Dialog {
    id: dialog

    padding: Theme.dialogMargin
    topPadding: 8
    bottomPadding: 12
    spacing: 0

    header: Label {
        text: dialog.title
        font.pixelSize: 20
        font.weight: Font.DemiBold
        color: Theme.text
        padding: Theme.dialogMargin
        bottomPadding: 10
        wrapMode: Text.WordWrap
        Accessible.role: Accessible.Heading
    }

    background: Rectangle {
        color: Theme.surface
        border.color: Theme.outline
        radius: Theme.radius
    }

    Overlay.modal: Rectangle {
        color: Theme.scrim
    }
}
