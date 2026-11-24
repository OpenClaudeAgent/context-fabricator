import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

Button {
    id: control

    required property int glyph
    required property bool selected
    property bool showGlyph: true
    readonly property color foreground: selected ? Theme.accentText : Theme.text
    readonly property real preferredNavWidth: Math.ceil(captionMetrics.advanceWidth) + 2 * horizontalPadding + (showGlyph ? 28 : 0)

    implicitHeight: 34
    Layout.preferredWidth: preferredNavWidth
    horizontalPadding: showGlyph ? 11 : 5
    verticalPadding: 6
    hoverEnabled: true
    Accessible.role: Accessible.PageTab
    Accessible.selectable: true
    Accessible.selected: selected

    TextMetrics {
        id: captionMetrics

        text: control.text
        font.family: control.font.family
        font.pixelSize: control.font.pixelSize
        font.weight: Font.DemiBold
    }

    contentItem: RowLayout {
        spacing: 8

        NavIcon {
            visible: control.showGlyph
            glyph: control.glyph
            ink: control.foreground
        }
        Label {
            Layout.fillWidth: true
            text: control.text
            color: control.foreground
            textFormat: Text.PlainText
            elide: Text.ElideRight
            horizontalAlignment: control.showGlyph ? Text.AlignLeft : Text.AlignHCenter
            font.weight: control.selected ? Font.DemiBold : Font.Medium
        }
    }

    background: Rectangle {
        radius: Theme.controlRadius
        color: control.selected ? Theme.primary : control.hovered ? Theme.raised : "transparent"
        border.width: control.visualFocus ? 2 : 0
        border.color: Theme.accent
    }
}
