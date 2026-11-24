pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Dialogs
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property InferenceSetup workflow
    required property ModelLibrary library
    required property LocalRuntime runtime
    signal manageDownloads

    property string answerProfile
    property bool embedding: false
    property string embeddingProfile
    property bool preferLocalArchive: false
    property url archive
    property bool editingAgain: false
    property bool selectionAvailable: false
    property libraryModel chosenAnswer
    property libraryModel chosenEmbedding
    property libraryModel reviewedAnswer
    property libraryModel reviewedEmbedding
    property libraryModel activeModel

    readonly property bool useLocalArchive: !runtime.ready && runtime.canImport && (preferLocalArchive || !runtime.canDownload)
    readonly property bool canPlan: workflow.statusKnown && library.ready && library.catalogReady && library.catalogFeedback === "" && selectionAvailable && (runtime.ready || runtime.canDownload || runtime.canImport)
    readonly property bool showingResult: workflow.hasStatus && !workflow.inProgress && !workflow.writing && !editingAgain
    readonly property bool choosing: !showingResult && !workflow.inProgress && !workflow.hasPlan && !workflow.reviewing && !workflow.writing
    readonly property bool showManageDownloads: workflow.canManageDownloads && !editingAgain && !workflow.writing
    readonly property string archiveName: archive.toString() === "" ? "" : decodeURIComponent(archive.toString().split("/").pop())

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(530, parent.width - 24)
    implicitHeight: implicitHeaderHeight + details.implicitHeight + implicitFooterHeight + topPadding + bottomPadding
    height: Math.min(implicitHeight, 560, parent.height - 24)
    title: qsTr("Set Up Answers")
    modal: true
    closePolicy: Popup.CloseOnEscape
    objectName: "inferenceSetupDialog"

    function refreshModels(): void {
        if (answerProfile === "" && library.setupAnswers.count > 0)
            answerProfile = library.setupAnswers.entryAt(0).profile; // qmllint disable compiler
        selectionAvailable = library.setupAnswers.containsProfile(answerProfile) && (!embedding || embeddingProfile === "" || library.setupEmbeddings.containsProfile(embeddingProfile)); // qmllint disable compiler
        chosenAnswer = library.modelForProfile(answerProfile); // qmllint disable compiler
        chosenEmbedding = library.modelForProfile(embeddingProfile); // qmllint disable compiler
        reviewedAnswer = library.modelForProfile(workflow.planAnswerProfile); // qmllint disable compiler
        reviewedEmbedding = library.modelForProfile(workflow.planEmbeddingProfile); // qmllint disable compiler
        activeModel = library.modelForProfile(workflow.activeProfile); // qmllint disable compiler
    }

    function editOptions(): void {
        editingAgain = true;
        if (workflow.lastAnswerProfile !== "")
            answerProfile = workflow.lastAnswerProfile;
        embedding = workflow.lastEmbedding;
        embeddingProfile = workflow.lastEmbeddingProfile;
        refreshModels();
    }

    function chooseArchive(operation: string): void {
        picker.operation = operation;
        picker.open();
    }

    onAboutToShow: {
        editingAgain = false;
        answerProfile = "";
        embedding = false;
        embeddingProfile = "";
        preferLocalArchive = false;
        archive = "";
        refreshModels();
    }

    onClosed: workflow.discardPlan() // qmllint disable compiler

    Connections {
        target: dialog.library
        enabled: dialog.visible
        function onChanged() {
            dialog.refreshModels();
        }
    }
    Connections {
        target: dialog.workflow
        enabled: dialog.visible
        function onChanged() {
            if (dialog.workflow.inProgress)
                dialog.editingAgain = false;
            dialog.refreshModels();
        }
    }

    ScrollView {
        id: scroll

        anchors.fill: parent
        contentWidth: availableWidth
        clip: true

        ColumnLayout {
            id: details

            width: scroll.availableWidth
            spacing: 14

            Label {
                Layout.fillWidth: true
                visible: dialog.choosing
                text: qsTr("Choose a local model to answer questions from your sources.")
                color: Theme.muted
                wrapMode: Text.Wrap
            }
            LibrarySection {
                Layout.fillWidth: true
                visible: dialog.choosing
                title: qsTr("Inference engine")
                contentItem: ColumnLayout {
                    spacing: 6

                    MetricRow {
                        visible: dialog.runtime.ready
                        title: dialog.runtime.engine
                        value: dialog.runtime.status
                    }
                    ButtonGroup {
                        id: runtimeSources
                    }
                    RadioButton {
                        visible: !dialog.runtime.ready && dialog.runtime.canDownload
                        text: qsTr("GitHub Releases")
                        objectName: "setupHostedRuntime"
                        ButtonGroup.group: runtimeSources
                        checked: !dialog.useLocalArchive
                        onClicked: dialog.preferLocalArchive = false
                    }
                    RadioButton {
                        visible: !dialog.runtime.ready && dialog.runtime.canImport
                        text: qsTr("Local ZIP")
                        objectName: "setupLocalArchive"
                        ButtonGroup.group: runtimeSources
                        checked: dialog.useLocalArchive
                        onClicked: dialog.preferLocalArchive = true
                    }
                    ActionButton {
                        visible: dialog.useLocalArchive
                        text: qsTr("Choose Runtime ZIP…")
                        onClicked: dialog.chooseArchive("")
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: dialog.useLocalArchive
                        text: dialog.archiveName || qsTr("No archive selected")
                        textFormat: Text.PlainText
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: !dialog.runtime.ready && !dialog.runtime.canDownload && !dialog.runtime.canImport
                        text: dialog.runtime.feedback || qsTr("Local inference engine is unavailable.")
                        textFormat: Text.PlainText
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                }
            }
            LibrarySection {
                Layout.fillWidth: true
                visible: dialog.choosing
                title: qsTr("Answer model")
                contentItem: ColumnLayout {
                    spacing: 5
                    ButtonGroup {
                        id: answerChoices
                    }
                    Repeater {
                        model: dialog.library.setupAnswers
                        delegate: ModelChoice {
                            ButtonGroup.group: answerChoices
                            checked: entry.profile === dialog.answerProfile
                            onClicked: {
                                dialog.answerProfile = entry.profile;
                                dialog.refreshModels();
                            }
                        }
                    }
                    SetupModelSource {
                        model: dialog.chosenAnswer
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: dialog.library.setupAnswers.count === 0 || dialog.library.catalogFeedback !== ""
                        text: dialog.library.catalogFeedback || (dialog.library.catalogReady ? qsTr("Recommended model choices are unavailable.") : qsTr("Loading model choices…"))
                        textFormat: Text.PlainText
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                }
            }
            LibrarySection {
                Layout.fillWidth: true
                visible: dialog.choosing && dialog.library.setupEmbeddings.count > 0
                title: qsTr("Semantic search")
                contentItem: ColumnLayout {
                    spacing: 5

                    Switch {
                        text: qsTr("Add semantic search")
                        objectName: "setupOptionalEmbedding"
                        checked: dialog.embedding
                        onToggled: {
                            dialog.embedding = checked;
                            dialog.refreshModels();
                        }
                    }
                    ButtonGroup {
                        id: embeddingChoices
                    }
                    RadioButton {
                        visible: dialog.embedding
                        text: qsTr("Automatic")
                        ButtonGroup.group: embeddingChoices
                        checked: dialog.embeddingProfile === ""
                        onClicked: {
                            dialog.embeddingProfile = "";
                            dialog.refreshModels();
                        }
                    }
                    Repeater {
                        model: dialog.library.setupEmbeddings
                        delegate: ModelChoice {
                            ButtonGroup.group: embeddingChoices
                            visible: dialog.embedding
                            checked: entry.profile === dialog.embeddingProfile
                            onClicked: {
                                dialog.embeddingProfile = entry.profile;
                                dialog.refreshModels();
                            }
                        }
                    }
                    SetupModelSource {
                        model: dialog.chosenEmbedding
                        visible: dialog.embedding && model.source !== ""
                    }
                }
            }
            InferenceSetupReview {
                Layout.fillWidth: true
                visible: dialog.workflow.hasPlan
                workflow: dialog.workflow
                answerModel: dialog.reviewedAnswer
                embeddingModel: dialog.reviewedEmbedding
                archiveName: dialog.archiveName
                onChooseArchive: dialog.chooseArchive("")
            }
            RowLayout {
                Layout.fillWidth: true
                visible: dialog.workflow.reviewing || dialog.workflow.writing
                BusyIndicator {
                    Layout.preferredWidth: 22
                    Layout.preferredHeight: 22
                    running: parent.visible
                }
                Label {
                    Layout.fillWidth: true
                    text: dialog.workflow.writing ? qsTr("Applying setup…") : qsTr("Checking setup requirements…")
                    wrapMode: Text.Wrap
                }
            }
            SurfaceGroup {
                Layout.fillWidth: true
                visible: !dialog.workflow.writing && !dialog.workflow.reviewing && !dialog.workflow.hasPlan && (dialog.workflow.inProgress || dialog.showingResult)
                contentItem: ColumnLayout {
                    spacing: 8
                    Label {
                        Layout.fillWidth: true
                        objectName: "setupActiveModel"
                        text: dialog.activeModel.profile !== "" ? dialog.activeModel.label : dialog.workflow.activeProfile
                        visible: text !== ""
                        textFormat: Text.PlainText
                        font.weight: Font.DemiBold
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        objectName: "setupPhase"
                        text: dialog.workflow.statusText
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: dialog.workflow.transferring && dialog.activeModel.profile !== ""
                        objectName: "setupProgressBytes"
                        text: qsTr("%1 of %2").arg(dialog.activeModel.partialSize).arg(dialog.activeModel.size)
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                    ProgressBar {
                        Layout.fillWidth: true
                        visible: dialog.workflow.inProgress && !dialog.workflow.waitingForArchive
                        objectName: "setupProgress"
                        value: dialog.activeModel.progress
                        indeterminate: !dialog.workflow.transferring || !dialog.activeModel.determinate
                        Accessible.name: dialog.workflow.statusText
                    }
                    SetupModelSource {
                        model: dialog.activeModel
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: text !== ""
                        text: dialog.workflow.statusError
                        textFormat: Text.PlainText
                        color: Theme.danger
                        wrapMode: Text.Wrap
                    }
                    Flow {
                        Layout.fillWidth: true
                        visible: dialog.workflow.waitingForArchive
                        spacing: 6
                        ActionButton {
                            text: qsTr("Choose Runtime ZIP…")
                            onClicked: dialog.chooseArchive(dialog.workflow.operationId)
                        }
                        ActionButton {
                            text: qsTr("Cancel Waiting Setup…")
                            onClicked: {
                                cancelWaiting.operation = dialog.workflow.operationId;
                                cancelWaiting.open();
                            }
                        }
                    }
                }
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: dialog.workflow.feedback
                textFormat: Text.PlainText
                color: Theme.danger
                wrapMode: Text.Wrap
            }
        }
    }

    footer: DialogActions.Wrapping {
        id: footerPane

        contentItem: GridLayout {
            columns: leading.visible && leading.implicitWidth + trailing.naturalWidth + columnSpacing <= footerPane.availableWidth ? 2 : 1
            columnSpacing: 8
            rowSpacing: 8

            RowLayout {
                id: leading

                visible: dialog.workflow.hasPlan || dialog.showManageDownloads
                ActionButton {
                    id: back

                    visible: dialog.workflow.hasPlan
                    text: qsTr("Back")
                    onClicked: dialog.workflow.discardPlan() // qmllint disable compiler
                }
                ActionButton {
                    id: downloads

                    visible: dialog.showManageDownloads
                    text: qsTr("Manage Downloads…")
                    objectName: "manageSetupDownloads"
                    onClicked: dialog.manageDownloads()
                }
            }
            Flow {
                id: trailing

                readonly property real naturalWidth: dismissAction.implicitWidth + (dialog.workflow.hasPlan ? confirmAction.implicitWidth + spacing : dialog.choosing ? reviewAction.implicitWidth + spacing : dialog.showingResult ? editAction.implicitWidth + spacing : 0)
                Layout.fillWidth: true
                Layout.minimumWidth: 0
                layoutDirection: Qt.RightToLeft
                spacing: 8

                ActionButton {
                    id: reviewAction

                    visible: dialog.choosing
                    text: qsTr("Review Setup")
                    objectName: "reviewInferenceSetup"
                    highlighted: true
                    enabled: dialog.canPlan
                    onClicked: dialog.workflow.review(dialog.answerProfile, dialog.embedding, dialog.embeddingProfile, dialog.useLocalArchive) // qmllint disable compiler
                }
                ActionButton {
                    id: confirmAction

                    visible: dialog.workflow.hasPlan
                    text: dialog.workflow.planHasTransfer ? qsTr("Download and Enable") : qsTr("Enable Model")
                    objectName: "confirmInferenceSetup"
                    highlighted: true
                    enabled: dialog.workflow.planAllowed && (!dialog.workflow.planNeedsArchive || dialog.archiveName !== "")
                    onClicked: dialog.workflow.confirm(dialog.archive) // qmllint disable compiler
                }
                ActionButton {
                    id: dismissAction

                    text: {
                        if (dialog.showingResult)
                            return qsTr("Done");
                        if (dialog.workflow.waitingForArchive || dialog.workflow.writing)
                            return qsTr("Close");
                        return dialog.workflow.inProgress ? qsTr("Continue in Background") : qsTr("Cancel");
                    }
                    onClicked: dialog.close()
                }
                ActionButton {
                    id: editAction

                    visible: dialog.showingResult
                    text: dialog.workflow.canManageDownloads ? qsTr("Review Options") : qsTr("Set Up Again")
                    onClicked: dialog.editOptions()
                }
            }
        }
    }

    FileDialog {
        id: picker

        property string operation
        title: qsTr("Choose Runtime ZIP…")
        fileMode: FileDialog.OpenFile
        nameFilters: [qsTr("ZIP archives (*.zip)")]
        onAccepted: {
            if (operation !== "") {
                dialog.workflow.provideArchive(operation, selectedFile); // qmllint disable compiler
            } else {
                dialog.archive = selectedFile;
                dialog.workflow.discardPlan(); // qmllint disable compiler
            }
        }
    }

    ConfirmationDialog {
        id: cancelWaiting

        property string operation
        title: qsTr("Cancel Waiting Setup?")
        message: qsTr("The waiting setup will be cancelled. Already installed components are retained.")
        closePolicy: Popup.CloseOnEscape
        footer: DialogActions {
            ActionButton {
                text: qsTr("Keep Waiting")
                onClicked: cancelWaiting.close()
            }
            ActionButton {
                destructive: true
                text: qsTr("Cancel Setup")
                onClicked: {
                    dialog.workflow.cancelArchive(cancelWaiting.operation); // qmllint disable compiler
                    cancelWaiting.close();
                }
            }
        }
    }

    component ModelChoice: RadioButton {
        id: choice

        required property libraryModel entry
        Layout.fillWidth: true
        objectName: "setupChoice-" + entry.profile
        text: entry.recommendedForSetup ? qsTr("%1 · Recommended · %2").arg(entry.label).arg(entry.size) : "%1 · %2".arg(entry.label).arg(entry.size)
        contentItem: Label {
            text: choice.text
            textFormat: Text.PlainText
            color: Theme.text
            leftPadding: choice.indicator.width + choice.spacing
            verticalAlignment: Text.AlignVCenter
            wrapMode: Text.Wrap
        }
    }
}
