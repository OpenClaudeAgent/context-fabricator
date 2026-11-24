import QtQuick
import QtQuick.Controls

SurfaceDialog {
    id: dialog

    required property string message
    property int maximumWidth: 420
    readonly property real preferredWidth: Math.min(maximumWidth, parent ? parent.width - 24 : maximumWidth)
    readonly property real messageWidth: Math.max(0, preferredWidth - leftPadding - rightPadding)

    parent: Overlay.overlay
    anchors.centerIn: parent
    modal: true
    width: preferredWidth
    implicitWidth: maximumWidth
    implicitHeight: implicitHeaderHeight + implicitFooterHeight + topPadding + bottomPadding + measurement.implicitHeight

    contentItem: Label {
        text: dialog.message
        textFormat: Text.PlainText
        font: dialog.font
        wrapMode: Text.WordWrap
    }

    // Measure the wrapped message without taking its width from the popup layout.
    Text {
        id: measurement

        visible: false
        Accessible.ignored: true
        width: dialog.messageWidth
        text: dialog.message
        textFormat: Text.PlainText
        font: dialog.font
        wrapMode: Text.WordWrap
    }
}
