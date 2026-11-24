import QtQuick
import QtQuick.Controls

DialogButtonBox {
    padding: Theme.dialogMargin
    topPadding: 12
    spacing: 10
    alignment: Qt.AlignRight

    background: Divider {}

    component Wrapping: Pane {
        horizontalPadding: Theme.dialogMargin
        topPadding: 12
        bottomPadding: 12
        background: Divider {}
    }

    component Divider: Item {
        Rectangle {
            anchors.top: parent.top
            anchors.left: parent.left
            anchors.right: parent.right
            anchors.leftMargin: Theme.dialogMargin
            anchors.rightMargin: Theme.dialogMargin
            height: 1
            color: Theme.outline
        }
    }
}
