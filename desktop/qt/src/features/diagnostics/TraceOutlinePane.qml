pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: pane

    required property TraceInspector inspector
    title: qsTr("Span hierarchy")

    ListView {
        id: list

        anchors.fill: parent
        clip: true
        spacing: 2
        model: pane.inspector.spans
        currentIndex: pane.inspector.selectedIndex
        ScrollBar.vertical: ScrollBar {
            id: scroll
        }
        delegate: ItemDelegate {
            id: step

            required property int index
            required property traceRecord modelData

            width: list.width - scroll.width
            highlighted: pane.inspector.selectedIndex === index
            objectName: "trace-step-" + modelData.name
            Accessible.name: "%1 · %2".arg(modelData.title).arg(modelData.outcome)
            onClicked: pane.inspector.selectSpan(index) // qmllint disable compiler

            contentItem: RowLayout {
                spacing: 6

                Label {
                    Layout.fillWidth: true
                    Layout.leftMargin: Math.min(step.modelData.depth, 4) * 9
                    text: step.modelData.depth > 0 ? "↳ " + step.modelData.title : step.modelData.title
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                Label {
                    visible: text.length > 0
                    text: step.modelData.duration
                    color: Theme.muted
                    font.pixelSize: 12
                }
            }
        }
    }
}
