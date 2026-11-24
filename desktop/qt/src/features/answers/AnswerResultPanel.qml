pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: panel

    required property answerResult result
    required property string traceId
    property string citationsTitle: qsTr("Sources")
    signal traceRequested(string traceId)

    spacing: 8

    RowLayout {
        Layout.fillWidth: true
        Label {
            Layout.fillWidth: true
            text: qsTr("Answer")
            color: Theme.text
            font.weight: Font.DemiBold
            Accessible.role: Accessible.Heading
        }
        ActionButton {
            visible: panel.traceId !== ""
            objectName: "viewAnswerTrace"
            text: qsTr("View Trace")
            onClicked: panel.traceRequested(panel.traceId)
        }
    }
    TextArea {
        Layout.fillWidth: true
        objectName: "answerResultText"
        padding: 0
        readOnly: true
        text: panel.result.text
        textFormat: TextEdit.PlainText
        wrapMode: TextEdit.Wrap
        background: null
    }
    SurfaceGroup {
        Layout.fillWidth: true
        Layout.topMargin: 8
        visible: panel.result.citations.length > 0
        title: panel.citationsTitle

        ColumnLayout {
            anchors.fill: parent
            spacing: 8

            Repeater {
                model: panel.result.citations
                delegate: ColumnLayout {
                    id: source

                    required property answerCitation modelData
                    required property int index
                    Layout.fillWidth: true
                    spacing: 6

                    Rectangle {
                        Layout.fillWidth: true
                        Layout.preferredHeight: 1
                        visible: source.index > 0
                        color: Theme.outline
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: source.modelData.lines.toString() === ""
                        text: source.modelData.label
                        color: Theme.text
                        textFormat: Text.PlainText
                        font.family: Theme.fixedFont
                        wrapMode: Text.Wrap
                    }
                    ActionButton {
                        Layout.maximumWidth: source.width
                        visible: source.modelData.lines.toString() !== ""
                        text: source.modelData.label
                        Accessible.name: qsTr("Open the cited lines on GitHub") + " " + text
                        onClicked: Qt.openUrlExternally(source.modelData.lines)
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: text !== ""
                        text: source.modelData.comparison || source.modelData.workflow
                        textFormat: Text.PlainText
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                    ActionButton {
                        visible: source.modelData.run.toString() !== ""
                        text: qsTr("View run")
                        onClicked: Qt.openUrlExternally(source.modelData.run)
                    }
                    TextArea {
                        Layout.fillWidth: true
                        objectName: "answerCitationQuote"
                        padding: 10
                        readOnly: true
                        text: source.modelData.quote
                        textFormat: TextEdit.PlainText
                        wrapMode: TextEdit.Wrap
                        background: Rectangle {
                            radius: Theme.controlRadius
                            color: Theme.raised
                        }
                    }
                }
            }
        }
    }
}
