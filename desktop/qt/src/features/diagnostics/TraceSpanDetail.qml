pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ScrollView {
    id: detail

    required property traceRecord span
    required property list<traceRecord> logs
    readonly property recordedAnswerDetails answerDetails: span.answerDetails
    signal workTraceRequested

    clip: true
    contentWidth: availableWidth
    objectName: "traceSpanDetail"

    onSpanChanged: {
        if (viewport)
            viewport.contentY = viewport.originY;
    }

    Flickable {
        id: viewport

        contentWidth: detail.availableWidth
        contentHeight: fields.implicitHeight

        ColumnLayout {
            id: fields

            width: detail.availableWidth
            spacing: 9

            Label {
                Layout.fillWidth: true
                text: detail.span.title
                textFormat: Text.PlainText
                font.pixelSize: 18
                font.weight: Font.DemiBold
                wrapMode: Text.Wrap
                Accessible.role: Accessible.Heading
            }
            Label {
                text: detail.span.outcome
                color: Theme.muted
            }

            SurfaceGroup {
                id: budget

                Layout.fillWidth: true
                visible: detail.answerDetails.budgetSummary !== ""
                title: qsTr("Model Context")
                objectName: "traceContextBudget"

                ColumnLayout {
                    width: budget.availableWidth
                    spacing: 6

                    Label {
                        objectName: "traceContextAdmitted"
                        Layout.fillWidth: true
                        text: detail.answerDetails.budgetSummary
                        font.weight: Font.DemiBold
                        wrapMode: Text.Wrap
                    }
                    Label {
                        objectName: "traceContextInputTokens"
                        Layout.fillWidth: true
                        text: detail.answerDetails.tokenBudget
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                    MetricRow {
                        title: qsTr("Token checks")
                        value: detail.answerDetails.tokenChecks
                    }
                }
            }

            DetailRow {
                title: qsTr("Operation")
                value: detail.span.operation
            }
            DetailRow {
                title: qsTr("HTTP request")
                value: detail.span.httpRequest
                objectName: "traceSpanHttpRequest"
            }
            DetailRow {
                title: qsTr("HTTP response")
                value: detail.span.httpStatus
                objectName: "traceSpanHttpStatus"
            }
            DetailRow {
                title: qsTr("Problem code")
                value: detail.span.problemCode
            }
            DetailRow {
                title: qsTr("Duration")
                value: detail.span.duration
            }
            DetailRow {
                title: qsTr("Client")
                value: detail.span.client
            }
            DetailRow {
                title: qsTr("Model")
                value: detail.span.modelProfile
            }
            DetailRow {
                title: detail.span.hasStart ? qsTr("Started") : qsTr("Recorded")
                value: detail.span.timestamp
            }
            DetailRow {
                title: qsTr("Span ID")
                value: detail.span.spanId
            }
            DetailRow {
                title: qsTr("Parent span ID")
                value: detail.span.parentSpanId
            }
            Label {
                Layout.fillWidth: true
                visible: detail.span.parentMissing
                text: qsTr("Parent span unavailable in the loaded records")
                color: Theme.muted
                wrapMode: Text.WordWrap
            }
            ActionButton {
                visible: detail.span.workTraceId.length > 0
                text: qsTr("View collection trace")
                objectName: "traceViewCollection"
                onClicked: detail.workTraceRequested()
            }

            TraceDetailsGroup {
                Layout.fillWidth: true
                title: qsTr("Inference Timing")
                fields: detail.answerDetails.inference
            }
            TraceDetailsGroup {
                Layout.fillWidth: true
                title: qsTr("Hybrid Fusion")
                fields: detail.answerDetails.fusion
                candidates: detail.answerDetails.fusionCandidates
                emptyCandidatesText: qsTr("No candidates recorded.")
            }
            TraceDetailsGroup {
                Layout.fillWidth: true
                title: qsTr("Recorded retrieval")
                fields: detail.answerDetails.retrieval
            }

            Label {
                visible: detail.span.events.length > 0
                text: qsTr("Span Events")
                Accessible.role: Accessible.Heading
                font.weight: Font.DemiBold
            }
            Repeater {
                model: detail.span.events

                Label {
                    required property string modelData

                    Layout.fillWidth: true
                    text: modelData
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
            }
            Label {
                visible: detail.logs.length > 0
                text: qsTr("Logs")
                Accessible.role: Accessible.Heading
                font.weight: Font.DemiBold
            }
            Repeater {
                model: detail.logs

                TraceLogRow {
                    required property traceRecord modelData

                    Layout.fillWidth: true
                    record: modelData
                }
            }
        }
    }

    component DetailRow: MetricRow {
        visible: value.length > 0
    }
}
