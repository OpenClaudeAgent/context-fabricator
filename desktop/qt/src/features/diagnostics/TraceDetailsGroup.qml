pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: section

    required property list<traceDetailField> fields
    property list<traceDetailField> candidates
    property string emptyCandidatesText

    visible: fields.length > 0

    ColumnLayout {
        width: section.availableWidth
        spacing: 7

        Repeater {
            model: section.fields
            delegate: MetricRow {
                required property traceDetailField modelData

                title: modelData.title
                value: modelData.value
            }
        }
        Label {
            Layout.fillWidth: true
            visible: section.candidates.length === 0 && text !== ""
            text: section.emptyCandidatesText
            color: Theme.muted
            wrapMode: Text.Wrap
        }
        Repeater {
            model: section.candidates
            delegate: ColumnLayout {
                id: candidate

                required property traceDetailField modelData

                Layout.fillWidth: true
                spacing: 3

                Rectangle {
                    Layout.fillWidth: true
                    implicitHeight: 1
                    color: Theme.outline
                }
                Label {
                    Layout.fillWidth: true
                    text: candidate.modelData.title
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.weight: Font.DemiBold
                }
                Label {
                    Layout.fillWidth: true
                    text: candidate.modelData.value
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    color: Theme.muted
                }
            }
        }
    }
}
