pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: panel

    required property traceAnswerDetails details
    spacing: Theme.gap

    SurfaceGroup {
        id: searched

        Layout.fillWidth: true
        title: qsTr("Sources searched")

        ColumnLayout {
            width: searched.availableWidth
            spacing: 8

            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: panel.details.reviewScope
                textFormat: Text.PlainText
                wrapMode: Text.Wrap
                color: Theme.muted
            }
            Repeater {
                model: panel.details.searches

                delegate: RowLayout {
                    id: search

                    required property traceAnswerExcerpt modelData
                    Layout.fillWidth: true
                    Layout.preferredHeight: 38
                    spacing: 8

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 3

                        Label {
                            Layout.fillWidth: true
                            text: search.modelData.title
                            color: Theme.text
                            elide: Text.ElideRight
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: text !== ""
                            text: search.modelData.location
                            textFormat: Text.PlainText
                            elide: Text.ElideMiddle
                            color: Theme.muted
                            font.pixelSize: 12
                            Accessible.ignored: text === ""
                        }
                    }
                    Label {
                        Layout.alignment: Qt.AlignVCenter | Qt.AlignRight
                        text: search.modelData.status
                        color: Theme.muted
                    }
                }
            }
        }
    }

    SurfaceGroup {
        id: candidates

        Layout.fillWidth: true
        title: qsTr("Candidates")

        ColumnLayout {
            width: candidates.availableWidth
            spacing: 10

            Label {
                Layout.fillWidth: true
                visible: panel.details.candidates.length === 0
                text: qsTr("There were no candidates to review.")
                wrapMode: Text.Wrap
                color: Theme.muted
            }
            Repeater {
                model: panel.details.candidates

                delegate: TraceExcerptView {
                    required property traceAnswerExcerpt modelData
                    Layout.fillWidth: true
                    record: modelData
                }
            }
        }
    }
}
