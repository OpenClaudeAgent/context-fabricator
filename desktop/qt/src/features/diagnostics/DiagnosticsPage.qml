pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

Item {
    id: page

    enum Section {
        Requests,
        Logs,
        Metrics,
        Traces,
        Storage,
        Usage
    }

    required property Diagnostics diagnostics
    required property DiagnosticLogs logs
    required property DiagnosticAnswerRequests requests
    required property AnswerUsageModel usage
    property int selectedDiagnosticTab: DiagnosticsPage.Requests
    property string signalQuery
    readonly property int gutter: width < 580 ? 16 : Theme.margin
    readonly property real rowWidth: width - 2 * gutter
    readonly property bool inRequests: page.selectedDiagnosticTab === DiagnosticsPage.Requests
    readonly property bool requestsActive: visible && inRequests
    readonly property bool inLogs: page.selectedDiagnosticTab === DiagnosticsPage.Logs
    readonly property bool logsActive: visible && inLogs
    readonly property bool showsPagedFeed: inRequests || inLogs
    readonly property DiagnosticLogs pagedFeed: inRequests ? requests.feed : logs
    readonly property bool inUsage: selectedDiagnosticTab === DiagnosticsPage.Usage
    readonly property bool usageActive: visible && inUsage
    signal traceRequested(string traceId, string spanId)

    onRequestsActiveChanged: requests.feed.setActive(requestsActive) // qmllint disable compiler
    onLogsActiveChanged: logs.setActive(logsActive) // qmllint disable compiler
    onUsageActiveChanged: usage.setActive(usageActive) // qmllint disable compiler

    ColumnLayout {
        anchors.fill: parent
        spacing: 0

        Item {
            Layout.fillWidth: true
            Layout.preferredHeight: headerLayout.implicitHeight + page.gutter + Theme.gap

            ColumnLayout {
                id: headerLayout
                x: page.gutter
                y: page.gutter
                width: page.rowWidth
                spacing: Theme.gap

                GridLayout {
                    Layout.fillWidth: true
                    columns: page.rowWidth < 520 ? 1 : 2
                    rowSpacing: 10
                    columnSpacing: 12

                    Flow {
                        Layout.fillWidth: true
                        layoutDirection: Qt.RightToLeft
                        spacing: 8

                        ActionButton {
                            objectName: "exportDiagnostics"
                            text: qsTr("Export diagnostics")
                            enabled: page.diagnostics.ready
                            onClicked: saveDialog.open()
                        }
                        ActionButton {
                            objectName: "openDiagnosticTrace"
                            text: qsTr("Open Trace…")
                            enabled: page.diagnostics.ready
                            onClicked: lookup.open()
                        }
                        ActionButton {
                            objectName: "refreshDiagnostics"
                            text: qsTr("Refresh")
                            enabled: page.inUsage ? !page.usage.busy : !page.diagnostics.busy && (!page.showsPagedFeed || !page.pagedFeed.busy)
                            onClicked: {
                                if (page.inUsage) {
                                    page.usage.refresh(); // qmllint disable compiler
                                } else {
                                    page.diagnostics.refresh(); // qmllint disable compiler
                                    if (page.showsPagedFeed)
                                        page.pagedFeed.refresh(); // qmllint disable compiler
                                }
                            }
                        }
                    }

                    SurfaceInput {
                        Layout.fillWidth: true
                        Layout.maximumWidth: page.rowWidth < 520 ? -1 : 320
                        objectName: page.inLogs ? "searchDiagnosticLogs" : "filterDiagnostics"
                        visible: !page.inUsage && !page.inRequests && page.selectedDiagnosticTab !== DiagnosticsPage.Storage
                        enabled: !page.inLogs || page.logs.ready
                        placeholderText: page.inLogs ? qsTr("Search logs") : qsTr("Filter diagnostics")
                        text: page.inLogs ? page.logs.query : page.signalQuery
                        Accessible.name: placeholderText
                        onTextEdited: {
                            if (page.inLogs) {
                                page.logs.setQuery(text); // qmllint disable compiler
                            } else {
                                page.signalQuery = text;
                                page.diagnostics.setFilter(text); // qmllint disable compiler
                            }
                        }
                    }
                }

                GridLayout {
                    Layout.fillWidth: true
                    columns: page.rowWidth < 460 ? 2 : 3
                    rowSpacing: 8
                    columnSpacing: 10

                    PageTitle {
                        Layout.fillWidth: true
                        text: qsTr("Diagnostics")
                    }
                    StatusChip {
                        objectName: "diagnosticRecording"
                        visible: page.diagnostics.ready
                        text: page.diagnostics.healthStatus
                        ready: page.diagnostics.recording
                    }
                    ActionButton {
                        Layout.columnSpan: page.rowWidth < 460 ? 2 : 1
                        Layout.alignment: Qt.AlignRight
                        objectName: "toggleDiagnosticDebug"
                        text: page.diagnostics.debugActive ? qsTr("Disable debug") : qsTr("Debug for 5 minutes")
                        visible: page.diagnostics.ready
                        enabled: !page.diagnostics.busy
                        onClicked: page.diagnostics.toggleDebug() // qmllint disable compiler
                    }
                }

                Label {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: page.diagnostics.feedback
                    textFormat: Text.PlainText
                    color: Theme.muted
                    wrapMode: Text.Wrap
                }

                Pane {
                    id: tabStrip

                    readonly property list<Item> sections: [requestsTab, logsTab, metricsTab, tracesTab, storageTab, usageTab]
                    readonly property int currentIndex: page.selectedDiagnosticTab
                    readonly property real buttonSpace: Math.max(0, Math.min(page.rowWidth, 648) - (sections.length - 1) * tabFlow.spacing)
                    readonly property real minimumButtonSpace: requestsTab.minimumTabWidth + logsTab.minimumTabWidth + metricsTab.minimumTabWidth + tracesTab.minimumTabWidth + storageTab.minimumTabWidth + usageTab.minimumTabWidth
                    readonly property real extraWidth: Math.floor(Math.max(0, (buttonSpace - minimumButtonSpace) / sections.length))

                    function moveSelection(index: int, offset: int): void {
                        const next = (index + offset + sections.length) % sections.length;
                        page.selectedDiagnosticTab = next;
                        sections[next].forceActiveFocus(Qt.TabFocusReason);
                    }

                    Layout.fillWidth: true
                    Layout.minimumWidth: 0
                    Layout.maximumWidth: 648
                    implicitHeight: tabFlow.implicitHeight
                    padding: 0
                    objectName: "diagnosticTabs"
                    Accessible.name: qsTr("Diagnostics section")
                    Accessible.role: Accessible.PageTabList
                    background: Rectangle {
                        color: Theme.raised
                        radius: Theme.controlRadius
                    }
                    ButtonGroup {
                        id: sectionButtons
                    }

                    contentItem: Flow {
                        id: tabFlow

                        spacing: 2

                        SectionTab {
                            id: requestsTab

                            section: DiagnosticsPage.Requests
                            objectName: "diagnosticRequestsTab"
                            text: qsTr("Requests")
                        }
                        SectionTab {
                            id: logsTab

                            section: DiagnosticsPage.Logs
                            text: qsTr("Logs")
                        }
                        SectionTab {
                            id: metricsTab

                            section: DiagnosticsPage.Metrics
                            text: qsTr("Metrics")
                        }
                        SectionTab {
                            id: tracesTab

                            section: DiagnosticsPage.Traces
                            text: qsTr("Traces")
                        }
                        SectionTab {
                            id: storageTab

                            section: DiagnosticsPage.Storage
                            text: qsTr("Storage")
                        }
                        SectionTab {
                            id: usageTab

                            section: DiagnosticsPage.Usage
                            objectName: "diagnosticUsageTab"
                            text: qsTr("Answer Usage")
                        }
                    }
                }

                CheckBox {
                    id: backgroundChecks

                    objectName: "includeBackgroundChecks"
                    implicitHeight: 28
                    visible: page.selectedDiagnosticTab === DiagnosticsPage.Traces
                    text: qsTr("Include background checks")
                    onToggled: page.diagnostics.setIncludeBackgroundChecks(checked) // qmllint disable compiler

                    indicator: Rectangle {
                        x: backgroundChecks.leftPadding
                        y: (backgroundChecks.height - height) / 2
                        width: 17
                        height: 17
                        radius: 4
                        color: backgroundChecks.checked ? Theme.primary : Theme.raised
                        border.color: backgroundChecks.visualFocus ? Theme.accent : Theme.outline

                        Label {
                            anchors.centerIn: parent
                            visible: backgroundChecks.checked
                            color: Theme.accentText
                            text: "✓"
                            Accessible.ignored: true
                        }
                    }

                    contentItem: Label {
                        leftPadding: backgroundChecks.indicator.width + 8
                        text: backgroundChecks.text
                        color: Theme.text
                        verticalAlignment: Text.AlignVCenter
                    }
                }
            }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: page.inRequests ? requestPane.StackLayout.index : page.inLogs ? logPane.StackLayout.index : page.inUsage ? usagePane.StackLayout.index : signalPane.StackLayout.index

            DiagnosticSectionList {
                id: requestPane

                model: page.requests
                delegate: requestDelegate
                feed: page.requests.feed
                loadMoreLabel: qsTr("Load Older Requests")
                sectionHeader: DiagnosticRequestsHeader {
                    requests: page.requests
                    recordCount: requestPane.count
                    onTraceRequested: function (traceId) {
                        page.traceRequested(traceId, "");
                    }
                }
            }

            DiagnosticSectionList {
                id: logPane

                model: page.logs
                delegate: logDelegate
                feed: page.logs
                loadMoreLabel: qsTr("Load more logs")
                sectionHeader: DiagnosticLogsHeader {
                    logs: page.logs
                    recordCount: logPane.count
                }
            }

            DiagnosticSectionList {
                id: usagePane

                model: page.usage.groups
                delegate: usageDelegate
                sectionHeader: AnswerUsageHeader {
                    usage: page.usage
                    groupCount: usagePane.count
                }
            }

            DiagnosticSectionList {
                id: signalPane

                sectionHeader: page.selectedDiagnosticTab === DiagnosticsPage.Storage ? storage : signals

                Connections {
                    target: page

                    function onSelectedDiagnosticTabChanged(): void {
                        if (signalPane.visible)
                            Qt.callLater(signalPane.positionViewAtBeginning);
                    }
                }
            }
        }
    }

    Component {
        id: logDelegate

        Item {
            id: logEntry

            required property traceRecord sample
            width: page.width
            height: logRow.implicitHeight

            DiagnosticSignalRow {
                id: logRow

                readonly property traceRecord record: logEntry.sample
                x: page.gutter
                width: page.rowWidth
                objectName: "diagnosticLogRow"
                compact: width < 535
                observedAt: record.observedAt
                title: record.title
                detail: record.httpRequest || record.operation
                problem: record.problemCode
                canOpen: record.traceId !== ""
                value: record.duration
                classification: record.level
                classificationColor: record.errorLevel ? Theme.danger : record.debugLevel ? Theme.accent : Theme.muted
                reserveStatus: true
                status: record.httpStatus === "" ? "" : qsTr("HTTP %1").arg(record.httpStatus)
                onClicked: {
                    if (canOpen)
                        page.traceRequested(record.traceId, record.spanId);
                }
            }
        }
    }

    Component {
        id: requestDelegate

        Item {
            id: requestEntry

            required property traceRecord sample
            required property string question
            width: page.width
            height: requestRow.implicitHeight + 14

            DiagnosticRequestRow {
                id: requestRow

                x: page.gutter + 10
                y: 7
                width: page.rowWidth - 20
                record: requestEntry.sample
                question: requestEntry.question
                onTraceRequested: function (traceId) {
                    page.traceRequested(traceId, "");
                }
            }
            Rectangle {
                x: page.gutter
                width: page.rowWidth
                anchors.bottom: parent.bottom
                height: 1
                color: Theme.outline
            }
        }
    }

    Component {
        id: usageDelegate

        Item {
            id: usageEntry

            required property answerUsageRow modelData
            width: page.width
            height: card.implicitHeight + 10

            AnswerUsageCard {
                id: card

                x: page.gutter
                width: page.rowWidth
                group: usageEntry.modelData
                onTraceRequested: function (traceId) {
                    page.traceRequested(traceId, "");
                }
            }
        }
    }

    component DiagnosticSectionList: ListView {
        id: list

        required property Component sectionHeader
        property DiagnosticLogs feed: null
        property string loadMoreLabel
        property int previousRecordCount: 0

        clip: true
        ScrollBar.vertical: ScrollBar {}
        onVisibleChanged: {
            if (visible)
                Qt.callLater(positionViewAtBeginning);
        }
        onWidthChanged: {
            if (page.inUsage || page.inRequests)
                Qt.callLater(positionViewAtBeginning);
        }
        onCountChanged: {
            if (previousRecordCount === 0 && count > 0)
                Qt.callLater(positionViewAtBeginning);
            previousRecordCount = count;
        }

        header: Item {
            width: list.width
            height: sectionHeading.implicitHeight + (page.inUsage && list.count > 0 ? 10 : 0)

            Loader {
                id: sectionHeading

                x: page.gutter
                width: page.rowWidth
                sourceComponent: list.sectionHeader
            }
        }

        footer: Item {
            width: list.width
            height: more.visible ? more.implicitHeight + page.gutter : page.gutter

            ActionButton {
                id: more

                anchors.right: parent.right
                anchors.rightMargin: page.gutter
                anchors.bottom: parent.bottom
                anchors.bottomMargin: page.gutter
                objectName: "loadMoreDiagnosticLogs"
                visible: list.feed !== null && list.feed.hasMore
                enabled: list.feed !== null && list.feed.ready && !list.feed.busy
                text: list.loadMoreLabel
                onClicked: list.feed.loadMore() // qmllint disable compiler
            }
        }
    }

    component SectionTab: SegmentTab {
        id: tab

        required property int section
        readonly property real minimumTabWidth: captionMetrics.advanceWidth + 14

        width: minimumTabWidth + tabStrip.extraWidth
        horizontalPadding: 7
        font.pixelSize: page.rowWidth < 500 ? 12 : 14
        ButtonGroup.group: sectionButtons
        checked: page.selectedDiagnosticTab === tab.section
        onClicked: page.selectedDiagnosticTab = tab.section
        Keys.onLeftPressed: tabStrip.moveSelection(tab.section, -1)
        Keys.onRightPressed: tabStrip.moveSelection(tab.section, 1)

        TextMetrics {
            id: captionMetrics

            text: tab.text
            font.family: tab.font.family
            font.pixelSize: tab.font.pixelSize
            font.weight: Font.DemiBold
        }
    }

    Component {
        id: signals

        DiagnosticsSignalList {
            diagnostics: page.diagnostics
            showsMetrics: page.selectedDiagnosticTab === DiagnosticsPage.Metrics
            onTraceRequested: function (traceId, spanId) {
                page.traceRequested(traceId, spanId);
            }
        }
    }

    Component {
        id: storage

        DiagnosticsStorage {
            diagnostics: page.diagnostics
        }
    }

    FileDialog {
        id: saveDialog

        title: qsTr("Export diagnostics")
        fileMode: FileDialog.SaveFile
        defaultSuffix: "json"
        onAccepted: page.diagnostics.saveSnapshot(selectedFile) // qmllint disable compiler
    }

    SurfaceDialog {
        id: lookup

        property string pendingId
        readonly property string normalizedId: traceInput.text.trim().toLowerCase()
        readonly property bool validId: /^[a-f0-9]{32}$/.test(normalizedId)

        function submit(): void {
            if (validId) {
                pendingId = normalizedId;
                accept();
            }
        }

        parent: Overlay.overlay
        anchors.centerIn: parent
        width: Math.min(320, parent.width - 32)
        modal: true
        closePolicy: Popup.CloseOnEscape
        horizontalPadding: 14
        topPadding: 0
        bottomPadding: 0
        title: qsTr("Open Trace")
        objectName: "diagnosticTraceLookup"

        onAboutToShow: {
            traceInput.clear();
            pendingId = "";
        }
        onOpened: traceInput.forceActiveFocus()
        onClosed: {
            if (pendingId !== "")
                page.traceRequested(pendingId, "");
        }

        header: Label {
            text: lookup.title
            color: Theme.text
            font.pixelSize: 16
            font.weight: Font.DemiBold
            padding: 14
            bottomPadding: 8
            Accessible.role: Accessible.Heading
        }

        background: Rectangle {
            color: Theme.surface
            border.color: Theme.outline
            radius: 16
        }

        contentItem: SurfaceInput {
            id: traceInput

            objectName: "diagnosticTraceId"
            placeholderText: qsTr("Trace ID")
            Accessible.name: placeholderText
            inputMethodHints: Qt.ImhNoAutoUppercase | Qt.ImhNoPredictiveText | Qt.ImhPreferLatin
            onAccepted: lookup.submit()
        }

        footer: DialogActions {
            padding: 14
            topPadding: 10
            background: null

            ActionButton {
                objectName: "cancelDiagnosticTrace"
                text: qsTr("Cancel")
                DialogButtonBox.buttonRole: DialogButtonBox.ActionRole
                onClicked: lookup.reject()
            }

            ActionButton {
                objectName: "confirmOpenDiagnosticTrace"
                text: qsTr("Open")
                highlighted: true
                enabled: lookup.validId
                DialogButtonBox.buttonRole: DialogButtonBox.ActionRole
                onClicked: lookup.submit()
            }
        }
    }
}
