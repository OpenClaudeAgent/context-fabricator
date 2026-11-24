import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: panel

    required property AnswerUsageModel usage
    required property int groupCount

    spacing: 4

    RowLayout {
        Layout.fillWidth: true
        spacing: 8

        Label {
            Layout.fillWidth: true
            text: qsTr("Retained Answer Requests")
            color: Theme.text
            font.weight: Font.DemiBold
            wrapMode: Text.WordWrap
            Accessible.role: Accessible.Heading
        }
        BusyIndicator {
            Layout.preferredWidth: 24
            Layout.preferredHeight: 24
            running: panel.usage.busy && panel.groupCount > 0
            visible: running
            Accessible.name: qsTr("Loading answer usage…")
        }
    }
    Label {
        Layout.fillWidth: true
        text: qsTr("Backend latency for published answers · up to 500 retained requests")
        color: Theme.muted
        font.pixelSize: 12
        wrapMode: Text.WordWrap
    }
    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: panel.usage.period
        color: Theme.muted
        font.pixelSize: 12
        wrapMode: Text.WordWrap
    }
    Label {
        Layout.fillWidth: true
        Layout.topMargin: 6
        visible: panel.usage.feedback !== ""
        text: panel.usage.hasSnapshot ? qsTr("Refresh failed. Showing the last successful reading.") : panel.usage.feedback
        textFormat: Text.PlainText
        color: Theme.statusWaiting
        wrapMode: Text.Wrap
        ToolTip.text: panel.usage.feedback
    }
    Label {
        Layout.fillWidth: true
        Layout.topMargin: 12
        visible: panel.groupCount === 0
        text: panel.usage.busy ? qsTr("Loading answer usage…") : panel.usage.ready && panel.usage.feedback === "" ? qsTr("No Answer Usage Yet") : qsTr("Answer Usage Unavailable")
        color: Theme.muted
        wrapMode: Text.Wrap
    }
}
