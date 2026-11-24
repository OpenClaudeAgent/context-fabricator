pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: panel

    required property list<traceModelSource> sources
    property bool collapsible: false
    property string disclosureKey: ""
    spacing: 8

    Label {
        Layout.fillWidth: true
        visible: panel.sources.length > 0
        text: qsTr("Sources Given to the Model")
        color: Theme.text
        font.weight: Font.DemiBold
        Accessible.role: Accessible.Heading
        wrapMode: Text.Wrap
    }
    Label {
        Layout.fillWidth: true
        visible: panel.sources.length === 0
        text: qsTr("Model context is unavailable for this answer.")
        color: Theme.muted
        wrapMode: Text.Wrap
    }
    Repeater {
        model: panel.sources

        delegate: SurfaceGroup {
            id: source

            required property traceModelSource modelData
            required property int index
            Layout.fillWidth: true
            title: panel.collapsible ? "" : modelData.title

            ColumnLayout {
                width: source.availableWidth
                spacing: 8

                ItemDelegate {
                    id: toggle
                    Layout.fillWidth: true
                    visible: panel.collapsible
                    objectName: "pipelineModelPassage-" + source.index
                    text: (checked ? "▾  " : "▸  ") + source.modelData.title
                    Accessible.name: source.modelData.title
                    checkable: true
                    contentItem: RowLayout {
                        Label {
                            Layout.fillWidth: true
                            text: toggle.text
                            elide: Text.ElideRight
                        }
                        Label {
                            Layout.maximumWidth: toggle.width * 0.6
                            visible: text !== ""
                            text: source.modelData.parts[0]?.location ?? ""
                            textFormat: Text.PlainText
                            elide: Text.ElideMiddle
                            color: Theme.muted
                        }
                    }
                    Connections {
                        target: panel
                        function onDisclosureKeyChanged() {
                            toggle.checked = false;
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: text !== "" && (!panel.collapsible || toggle.checked)
                    text: source.modelData.scope
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.muted
                }
                Repeater {
                    model: !panel.collapsible || toggle.checked ? source.modelData.changes : []

                    delegate: Label {
                        required property string modelData
                        Layout.fillWidth: true
                        text: modelData
                        textFormat: Text.PlainText
                        wrapMode: Text.Wrap
                        color: Theme.muted
                        font.family: Theme.fixedFont
                    }
                }
                Repeater {
                    model: !panel.collapsible || toggle.checked ? source.modelData.parts : []

                    delegate: TraceExcerptView {
                        required property traceAnswerExcerpt modelData
                        Layout.fillWidth: true
                        record: modelData
                    }
                }
            }
        }
    }
}
