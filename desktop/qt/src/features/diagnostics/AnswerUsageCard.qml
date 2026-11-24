pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: card

    required property answerUsageRow group
    signal traceRequested(string traceId)

    ColumnLayout {
        width: card.availableWidth
        spacing: 10

        RowLayout {
            Layout.fillWidth: true
            Layout.alignment: Qt.AlignTop
            spacing: 10

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 3

                Label {
                    Layout.fillWidth: true
                    text: card.group.client
                    color: Theme.text
                    font.weight: Font.DemiBold
                    wrapMode: Text.WordWrap
                    Accessible.role: Accessible.Heading
                }
                Label {
                    Layout.fillWidth: true
                    text: card.group.profile
                    textFormat: Text.PlainText
                    color: Theme.muted
                    elide: Text.ElideMiddle
                    font.family: Theme.fixedFont
                    font.pixelSize: 12
                }
            }
            ActionButton {
                objectName: "answerUsageTrace"
                Layout.alignment: Qt.AlignTop
                visible: card.group.traceId !== ""
                text: qsTr("Latest Trace")
                onClicked: card.traceRequested(card.group.traceId)
            }
        }
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 1
            color: Theme.outline
        }
        GridLayout {
            Layout.fillWidth: true
            columns: card.availableWidth < 410 ? 3 : 5
            columnSpacing: 8
            rowSpacing: 8
            uniformCellWidths: true

            UsageMetric {
                title: qsTr("Published")
                count: card.group.published
                tint: Theme.statusReady
            }
            UsageMetric {
                title: qsTr("Abstained")
                count: card.group.abstained
                tint: Theme.statusWaiting
            }
            UsageMetric {
                title: qsTr("Cancelled")
                count: card.group.cancelled
            }
            UsageMetric {
                title: qsTr("Failed")
                count: card.group.failed
                tint: Theme.danger
            }
            UsageMetric {
                title: qsTr("Unknown")
                count: card.group.unknown
                tint: Theme.statusWaiting
            }
        }
        RowLayout {
            Layout.fillWidth: true
            spacing: 16

            Latency {
                title: qsTr("Median")
                value: card.group.median
            }
            Latency {
                title: qsTr("P95")
                value: card.group.p95
            }
            Item {
                Layout.fillWidth: true
            }
        }
    }

    component UsageMetric: ColumnLayout {
        id: metric

        required property string title
        required property int count
        property color tint: Theme.text

        Layout.fillWidth: true
        spacing: 3

        Label {
            Layout.fillWidth: true
            text: metric.title
            color: Theme.muted
            font.pixelSize: 12
            wrapMode: Text.WordWrap
        }
        Label {
            Layout.fillWidth: true
            text: metric.count.toLocaleString(Qt.locale(), "f", 0)
            color: metric.count > 0 ? metric.tint : Theme.text
            font.pixelSize: 17
            font.weight: Font.DemiBold
            Accessible.name: metric.title + " " + text
        }
    }

    component Latency: Label {
        required property string title
        required property string value
        text: "%1  %2".arg(title).arg(value || "—")
        wrapMode: Text.WordWrap
        font.pixelSize: 12
        color: Theme.text
    }
}
