pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: list

    required property Diagnostics diagnostics
    required property bool showsMetrics
    readonly property bool compact: width < 535
    signal traceRequested(string traceId, string spanId)

    spacing: 2

    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        Layout.bottomMargin: 6
        text: list.showsMetrics ? qsTr("Latest Operation Counts") : qsTr("Recorded Spans")
        color: Theme.text
        font.weight: Font.DemiBold
        Accessible.role: Accessible.Heading
    }

    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        Layout.bottomMargin: 6
        visible: list.showsMetrics
        text: qsTr("Counts reset when the backend restarts. The latest retained reading can come from an earlier session.")
        color: Theme.muted
        font.pixelSize: 12
        wrapMode: Text.WordWrap
    }

    Label {
        Layout.fillWidth: true
        visible: records.count === 0
        text: list.diagnostics.busy ? qsTr("Loading diagnostics…") : !list.diagnostics.ready ? qsTr("Diagnostics unavailable") : list.showsMetrics ? qsTr("No metrics match this filter") : qsTr("No recorded spans match this filter")
        color: Theme.muted
        wrapMode: Text.WordWrap
    }

    Repeater {
        id: records

        model: list.showsMetrics ? list.diagnostics.metrics : list.diagnostics.traces

        delegate: DiagnosticSignalRow {
            id: row

            required property traceRecord modelData

            Layout.fillWidth: true
            objectName: list.showsMetrics ? "diagnosticMetricRow" : "diagnosticTraceRow"
            compact: list.compact
            title: modelData.title
            observedAt: modelData.observedAt
            detail: list.showsMetrics ? "" : (modelData.operation || modelData.httpRequest)
            canOpen: !list.showsMetrics && modelData.traceId !== ""
            value: list.showsMetrics ? (modelData.count || "—") : modelData.duration
            classification: list.showsMetrics ? "" : modelData.outcome
            classificationColor: modelData.failed ? Theme.danger : modelData.succeeded ? Theme.statusReady : Theme.muted
            reserveStatus: !list.showsMetrics
            status: list.showsMetrics || modelData.httpStatus === "" ? "" : qsTr("HTTP %1").arg(modelData.httpStatus)
            onClicked: {
                if (canOpen)
                    list.traceRequested(modelData.traceId, modelData.spanId);
            }
        }
    }
}
