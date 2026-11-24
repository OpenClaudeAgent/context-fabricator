import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ApplicationWindow {
    id: window

    required property Overview overview
    required property Answers answers
    required property PipelineInspector pipelineInspector
    required property ModelLibrary modelLibrary
    required property HubCatalog hubCatalog
    required property InferenceSetup inferenceSetup
    required property LocalRuntime localRuntime
    required property EmbeddingIndex embeddingIndex
    required property EmbeddingProfiles embeddingProfiles
    required property Preferences preferences
    required property HistoryPreferences history
    required property AnswerHistory answerHistory
    required property GitHubConnector github
    required property GitHubActionsActivity githubActions
    required property GitHubGraphBrowser githubGraph
    required property Diagnostics diagnostics
    required property DiagnosticLogs diagnosticLogs
    required property DiagnosticAnswerRequests answerRequests
    required property AnswerUsageModel answerUsage
    required property TraceInspector traceInspector
    required property TraceAnswer traceAnswer
    required property AppLanguage appLanguage
    property int currentPage: 0
    readonly property bool compact: width < 700
    readonly property int navMargin: compact ? 12 : 16
    readonly property bool diagnosticsVisible: visible && currentPage === 3
    readonly property bool settingsVisible: visible && currentPage === 1
    readonly property bool showNavGlyphs: !compact || width >= 560
    readonly property bool observingModels: modelsDialog.visible || setupDialog.visible

    onObservingModelsChanged: {
        if (observingModels) {
            modelLibrary.open(); // qmllint disable compiler
        } else {
            modelLibrary.close(); // qmllint disable compiler
        }
        localRuntime.setObserved(observingModels); // qmllint disable compiler
        inferenceSetup.setObserved(observingModels); // qmllint disable compiler
    }

    title: qsTr("Context Fabricator")
    objectName: "mainWindow"
    width: 1100
    height: 740
    minimumWidth: 400
    minimumHeight: 360
    visible: true
    color: Theme.canvas
    font.pixelSize: 14
    palette.window: Theme.canvas
    palette.windowText: Theme.text
    palette.base: Theme.field
    palette.alternateBase: Theme.surface
    palette.button: Theme.raised
    palette.buttonText: Theme.text
    palette.text: Theme.text
    palette.placeholderText: Theme.muted
    palette.highlight: Theme.selection
    palette.highlightedText: Theme.accent
    palette.link: Theme.accent
    palette.mid: Theme.outline
    palette.dark: Theme.outline
    palette.light: Theme.hovered
    palette.disabled.text: Theme.disabled
    palette.disabled.buttonText: Theme.disabled
    palette.disabled.windowText: Theme.disabled

    onDiagnosticsVisibleChanged: diagnostics.setPageVisible(diagnosticsVisible) // qmllint disable compiler
    onSettingsVisibleChanged: embeddingProfiles.setObserved(settingsVisible) // qmllint disable compiler
    onCurrentPageChanged: {
        if (currentPage === 1) {
            preferences.open(); // qmllint disable compiler
            history.open(); // qmllint disable compiler
        }
    }

    Shortcut {
        sequences: [StandardKey.Close]
        onActivated: window.close()
    }

    Shortcut {
        sequences: [StandardKey.Quit]
        onActivated: Qt.quit()
    }

    Shortcut {
        sequences: [StandardKey.Preferences]
        onActivated: window.currentPage = 1
    }

    GridLayout {
        anchors.fill: parent
        columns: window.compact ? 1 : 2
        rowSpacing: 0
        columnSpacing: 0

        Rectangle {
            Layout.fillWidth: window.compact
            Layout.fillHeight: !window.compact
            Layout.preferredWidth: window.compact ? window.width : 220
            Layout.preferredHeight: window.compact ? Math.max(96, railLayout.implicitHeight + 2 * window.navMargin) : window.height
            color: Theme.rail

            Rectangle {
                x: window.compact ? 0 : parent.width - 1
                y: window.compact ? parent.height - 1 : 0
                width: window.compact ? parent.width : 1
                height: window.compact ? 1 : parent.height
                color: Theme.outline
            }

            ColumnLayout {
                id: railLayout

                anchors.fill: parent
                anchors.margins: window.navMargin
                spacing: window.compact ? 8 : 18

                RowLayout {
                    Layout.fillWidth: true
                    spacing: 8

                    Rectangle {
                        Layout.preferredWidth: 26
                        Layout.preferredHeight: 26
                        radius: Theme.controlRadius
                        color: Theme.primary

                        NavIcon {
                            anchors.centerIn: parent
                            width: implicitWidth
                            height: implicitHeight
                            glyph: NavIcon.Fabric
                            ink: Theme.accentText
                        }
                    }
                    Label {
                        Layout.fillWidth: true
                        text: window.title
                        color: Theme.text
                        elide: Text.ElideRight
                        font.pixelSize: 14
                        font.weight: Font.DemiBold
                    }
                }

                GridLayout {
                    id: navigation

                    readonly property real singleRowWidth: overviewTab.preferredNavWidth + answersTab.preferredNavWidth + connectorsTab.preferredNavWidth + diagnosticsTab.preferredNavWidth + settingsTab.preferredNavWidth + 4 * columnSpacing
                    readonly property bool wraps: window.compact && singleRowWidth > window.width - 2 * window.navMargin

                    Layout.fillWidth: true
                    columns: window.compact ? (wraps ? 3 : 5) : 1
                    rowSpacing: 3
                    columnSpacing: 4

                    Label {
                        Layout.fillWidth: true
                        Layout.leftMargin: 9
                        visible: !window.compact
                        text: qsTr("Workspace")
                        color: Theme.muted
                        font.pixelSize: 12
                        font.weight: Font.DemiBold
                        Accessible.role: Accessible.Heading
                    }
                    NavButton {
                        id: overviewTab

                        Layout.fillWidth: true
                        objectName: "overviewTab"
                        glyph: NavIcon.Dashboard
                        showGlyph: window.showNavGlyphs
                        text: qsTr("Overview")
                        selected: window.currentPage === 0
                        onClicked: window.currentPage = 0
                        KeyNavigation.right: window.compact ? answersTab : null
                        KeyNavigation.down: window.compact ? (navigation.wraps ? diagnosticsTab : null) : answersTab
                    }
                    NavButton {
                        id: answersTab

                        Layout.fillWidth: true
                        objectName: "answersTab"
                        glyph: NavIcon.Answers
                        showGlyph: window.showNavGlyphs
                        text: qsTr("Answers")
                        selected: window.currentPage === 4
                        onClicked: window.currentPage = 4
                        KeyNavigation.right: window.compact ? connectorsTab : null
                        KeyNavigation.down: window.compact ? (navigation.wraps ? settingsTab : null) : connectorsTab
                    }
                    NavButton {
                        id: connectorsTab

                        Layout.fillWidth: true
                        objectName: "connectorsTab"
                        glyph: NavIcon.Connectors
                        showGlyph: window.showNavGlyphs
                        text: qsTr("Connectors")
                        selected: window.currentPage === 2
                        onClicked: window.currentPage = 2
                        KeyNavigation.right: window.compact ? diagnosticsTab : null
                        KeyNavigation.down: window.compact ? null : diagnosticsTab
                    }
                    Label {
                        Layout.fillWidth: true
                        Layout.topMargin: 12
                        Layout.leftMargin: 9
                        visible: !window.compact
                        text: qsTr("System")
                        color: Theme.muted
                        font.pixelSize: 12
                        font.weight: Font.DemiBold
                        Accessible.role: Accessible.Heading
                    }
                    NavButton {
                        id: diagnosticsTab

                        Layout.fillWidth: true
                        objectName: "diagnosticsTab"
                        glyph: NavIcon.Diagnostics
                        showGlyph: window.showNavGlyphs
                        text: qsTr("Diagnostics")
                        selected: window.currentPage === 3
                        onClicked: window.currentPage = 3
                        KeyNavigation.right: window.compact ? settingsTab : null
                        KeyNavigation.down: window.compact ? null : settingsTab
                    }
                    NavButton {
                        id: settingsTab

                        Layout.fillWidth: true
                        objectName: "settingsTab"
                        glyph: NavIcon.Settings
                        showGlyph: window.showNavGlyphs
                        text: qsTr("Settings")
                        selected: window.currentPage === 1
                        onClicked: window.currentPage = 1
                    }
                }

                Item {
                    Layout.fillHeight: true
                    visible: !window.compact
                }

                StatusChip {
                    Layout.alignment: Qt.AlignLeft
                    Layout.maximumWidth: 190
                    visible: !window.compact && window.currentPage !== 0
                    text: window.overview.serviceState
                    ready: window.overview.ready
                }
            }
        }

        StackLayout {
            Layout.fillWidth: true
            Layout.fillHeight: true
            currentIndex: window.currentPage

            OverviewPage {
                overview: window.overview
                onAnswersRequested: window.currentPage = 4
            }
            SettingsPage {
                id: settingsPage

                preferences: window.preferences
                history: window.history
                appLanguage: window.appLanguage
                embeddingProfiles: window.embeddingProfiles
                onModelLibraryRequested: modelsDialog.open()
            }
            ConnectorsPage {
                github: window.github
                githubActions: window.githubActions
                githubGraph: window.githubGraph
                onTraceRequested: function (traceId, spanId) {
                    window.traceInspector.open(traceId, spanId); // qmllint disable compiler
                }
            }
            DiagnosticsPage {
                diagnostics: window.diagnostics
                logs: window.diagnosticLogs
                requests: window.answerRequests
                usage: window.answerUsage
                onTraceRequested: function (traceId, spanId) {
                    window.traceInspector.open(traceId, spanId); // qmllint disable compiler
                }
            }
            AnswersPage {
                answers: window.answers
                history: window.answerHistory
                onPipelineRequested: pipelineDialog.openFor(window.answers.question)
                onModelLibraryRequested: modelsDialog.open()
                onInferenceSetupRequested: setupDialog.open()
                onHistorySettingsRequested: {
                    window.currentPage = 1;
                    Qt.callLater(settingsPage.showAnswerHistorySettings);
                }
                onTraceRequested: function (traceId) {
                    window.traceInspector.open(traceId, ""); // qmllint disable compiler
                }
            }
        }
    }

    TraceInspectorDialog {
        inspector: window.traceInspector
        answer: window.traceAnswer
    }

    PipelineInspectorDialog {
        id: pipelineDialog
        pipeline: window.pipelineInspector
    }

    ModelLibraryDialog {
        id: modelsDialog

        library: window.modelLibrary
        hub: window.hubCatalog
        runtime: window.localRuntime
        embeddingIndex: window.embeddingIndex
        setupActive: window.inferenceSetup.inProgress || window.inferenceSetup.writing
        canRestart: window.overview.canRestart && !window.answers.busy && !window.answers.modelsBusy && !window.modelLibrary.transferActive && window.modelLibrary.pendingProfile === "" && !window.localRuntime.busy && !window.embeddingIndex.writing && !window.embeddingProfiles.busy && !setupActive
        onSetupRequested: setupDialog.open()
        onRestartRequested: window.overview.restart() // qmllint disable compiler
    }

    InferenceSetupDialog {
        id: setupDialog

        workflow: window.inferenceSetup
        library: window.modelLibrary
        runtime: window.localRuntime
        onManageDownloads: {
            if (!modelsDialog.visible)
                modelsDialog.open();
            setupDialog.close();
        }
    }
}
