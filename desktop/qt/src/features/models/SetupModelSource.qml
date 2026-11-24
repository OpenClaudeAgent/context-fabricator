pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls

ItemDelegate {
    id: link

    required property libraryModel model
    visible: model.source !== ""
    enabled: model.sourceUrl.toString() !== ""
    padding: 0
    text: model.source
    Accessible.role: Accessible.Link
    onClicked: Qt.openUrlExternally(model.sourceUrl)

    contentItem: Label {
        text: link.text
        textFormat: Text.PlainText
        color: link.enabled ? Theme.accent : Theme.muted
        font.pixelSize: 12
        font.underline: link.hovered || link.visualFocus
        elide: Text.ElideRight
    }
    background: Item {}
}
