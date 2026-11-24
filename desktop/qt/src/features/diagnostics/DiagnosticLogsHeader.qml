pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: header

    required property DiagnosticLogs logs
    required property int recordCount
    readonly property bool hasEvents: events.count > 0
    readonly property bool hasLogs: recordCount > 0 || logs.hasMore
    readonly property bool compact: width < 535

    spacing: 0

    RowLayout {
        Layout.fillWidth: true
        Layout.bottomMargin: 10
        spacing: 8

        Item {
            Layout.fillWidth: true
        }
        BusyIndicator {
            Layout.preferredWidth: 20
            Layout.preferredHeight: 20
            visible: running
            running: header.logs.busy && (header.hasEvents || header.hasLogs)
            Accessible.name: qsTr("Loading logs")
        }
        Label {
            text: qsTr("Level")
            color: Theme.muted
            Accessible.ignored: true
        }
        LocalizedComboBox {
            id: levelPicker

            objectName: "diagnosticLogLevel"
            Layout.preferredWidth: 137
            implicitHeight: 32
            Accessible.name: qsTr("Log level")
            labels: [qsTr("All Levels"), qsTr("Info"), qsTr("Error"), qsTr("Debug")]
            selectedIndex: header.logs.level
            enabled: header.logs.ready
            onActivated: function (index) {
                header.logs.setLevel(index); // qmllint disable compiler
            }

            background: Rectangle {
                radius: Theme.controlRadius
                color: levelPicker.hovered ? Theme.hovered : Theme.raised
                border.color: levelPicker.visualFocus ? Theme.accent : Theme.outline
            }

            indicator: Label {
                x: levelPicker.width - width - 10
                anchors.verticalCenter: parent.verticalCenter
                text: "⌄"
                color: Theme.muted
                Accessible.ignored: true
            }
        }
    }

    Rectangle {
        Layout.fillWidth: true
        implicitHeight: 1
        color: Theme.outline
    }

    Label {
        Layout.fillWidth: true
        Layout.topMargin: 12
        Layout.bottomMargin: 8
        visible: header.logs.feedback !== "" && (header.hasLogs || header.hasEvents)
        text: header.logs.feedback
        textFormat: Text.PlainText
        color: Theme.muted
        wrapMode: Text.Wrap
    }

    SectionHeading {
        visible: header.hasEvents
        text: qsTr("Backend Events")
    }
    Repeater {
        id: events

        model: header.logs.events
        delegate: DiagnosticSignalRow {
            required property diagnosticEvent modelData

            Layout.fillWidth: true
            compact: header.compact
            observedAt: modelData.observedAt
            title: modelData.title
            detail: modelData.code
        }
    }

    SectionHeading {
        Layout.topMargin: header.hasEvents ? 18 : 10
        visible: header.hasLogs
        text: qsTr("Telemetry Logs")
    }

    ColumnLayout {
        Layout.fillWidth: true
        Layout.topMargin: 16
        visible: !header.hasEvents && !header.hasLogs
        spacing: 4

        Label {
            Layout.fillWidth: true
            color: Theme.text
            font.weight: Font.DemiBold
            text: {
                if (header.logs.busy)
                    return qsTr("Loading logs…");
                if (!header.logs.ready)
                    return qsTr("Logs Unavailable");
                if (header.logs.feedback !== "")
                    return qsTr("Couldn’t Load Logs");
                if (header.logs.query.trim() !== "")
                    return qsTr("No logs match this search");
                return header.logs.allLevels ? qsTr("No Logs Yet") : qsTr("No Logs at This Level");
            }
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            text: header.logs.feedback
            textFormat: Text.PlainText
            color: Theme.muted
            wrapMode: Text.Wrap
        }
    }

    component SectionHeading: Label {
        Layout.fillWidth: true
        Layout.leftMargin: 12
        Layout.topMargin: 10
        Layout.bottomMargin: 6
        color: Theme.muted
        font.weight: Font.DemiBold
        Accessible.role: Accessible.Heading
    }
}
