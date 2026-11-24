pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: header

    required property DiagnosticAnswerRequests requests
    required property int recordCount
    readonly property DiagnosticLogs feed: requests.feed
    readonly property bool hasLatest: requests.latestTraceId !== ""
    signal traceRequested(string traceId)

    spacing: 8

    SurfaceGroup {
        Layout.fillWidth: true
        visible: header.hasLatest
        objectName: "latestAnswerRequest"

        contentItem: GridLayout {
            columns: width < 390 ? 1 : 2
            columnSpacing: 12
            rowSpacing: 8

            ColumnLayout {
                Layout.fillWidth: true
                spacing: 4

                Label {
                    Layout.fillWidth: true
                    text: qsTr("Latest Answer Request")
                    font.weight: Font.DemiBold
                    wrapMode: Text.Wrap
                    Accessible.role: Accessible.Heading
                }
                Label {
                    Layout.fillWidth: true
                    objectName: "diagnosticLatestAnswerQuestion"
                    visible: text !== ""
                    text: header.requests.latestQuestion
                    textFormat: Text.PlainText
                    maximumLineCount: 2
                    wrapMode: Text.Wrap
                    elide: Text.ElideRight
                    color: Theme.muted
                }
                Label {
                    Layout.fillWidth: true
                    visible: header.requests.latestAbstained
                    text: qsTr("Insufficient evidence")
                    color: Theme.statusWaiting
                    wrapMode: Text.Wrap
                }
            }
            ActionButton {
                Layout.alignment: Qt.AlignRight
                objectName: "viewLatestAnswerTrace"
                text: qsTr("View Trace")
                highlighted: true
                onClicked: header.traceRequested(header.requests.latestTraceId)
            }
        }
    }

    RowLayout {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        objectName: "diagnosticAnswerInProgress"
        visible: header.requests.pending
        spacing: 8

        BusyIndicator {
            Layout.preferredWidth: 22
            Layout.preferredHeight: 22
            running: header.requests.pending
            Accessible.ignored: true
        }
        Label {
            Layout.fillWidth: true
            text: qsTr("Answer request in progress…")
            color: Theme.muted
            wrapMode: Text.Wrap
        }
    }

    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        visible: header.feed.ready && (header.recordCount > 0 || header.feed.hasMore || !header.hasLatest && !header.requests.pending)
        text: qsTr("Recent Answer Requests")
        color: Theme.text
        font.weight: Font.DemiBold
        Accessible.role: Accessible.Heading
    }
    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        visible: text !== ""
        text: header.feed.feedback
        textFormat: Text.PlainText
        color: Theme.statusWaiting
        wrapMode: Text.Wrap
    }
    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        visible: text !== ""
        text: {
            if (!header.feed.ready)
                return qsTr("Requests Unavailable");
            if (header.hasLatest || header.requests.pending || header.recordCount > 0 || header.feed.hasMore)
                return "";
            if (header.feed.busy)
                return qsTr("Loading requests…");
            if (header.feed.feedback === "")
                return qsTr("No Answer Requests Yet");
            return "";
        }
        color: Theme.muted
        wrapMode: Text.Wrap
    }
    Label {
        Layout.fillWidth: true
        Layout.leftMargin: 10
        visible: header.feed.ready && !header.feed.busy && !header.hasLatest && !header.requests.pending && header.recordCount === 0 && !header.feed.hasMore && header.feed.feedback === ""
        text: qsTr("Send a question in Answers or run ctxfab ask.")
        color: Theme.muted
        wrapMode: Text.Wrap
    }
}
