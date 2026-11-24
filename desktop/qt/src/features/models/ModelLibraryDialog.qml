pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property ModelLibrary library
    required property HubCatalog hub
    required property LocalRuntime runtime
    required property EmbeddingIndex embeddingIndex
    required property bool canRestart
    required property bool setupActive
    signal restartRequested
    signal setupRequested

    parent: Overlay.overlay
    anchors.centerIn: parent
    title: hub.active ? qsTr("Search Models") : qsTr("Manage Models")
    width: Math.min(760, parent.width - 24)
    height: Math.min(700, parent.height - 24)
    modal: true
    closePolicy: hub.active ? Popup.NoAutoClose : Popup.CloseOnEscape
    objectName: "modelLibraryDialog"

    onOpened: {
        embeddingIndex.setObserved(true); // qmllint disable compiler
    }
    onClosed: {
        hub.close(); // qmllint disable compiler
        embeddingIndex.setObserved(false); // qmllint disable compiler
    }

    Shortcut {
        enabled: dialog.visible && !dialog.hub.active && dialog.library.catalogReady
        sequences: [StandardKey.Find]
        onActivated: filter.forceActiveFocus()
    }

    Shortcut {
        enabled: dialog.visible && dialog.hub.active && !dialog.hub.addingDownload
        sequences: [StandardKey.Cancel]
        onActivated: dialog.hub.close() // qmllint disable compiler
    }

    header: Pane {
        id: headerPane

        horizontalPadding: Theme.dialogMargin
        topPadding: Theme.dialogMargin
        bottomPadding: 10
        background: Item {}

        contentItem: GridLayout {
            id: headerLayout

            readonly property real accessoryWidth: dialog.hub.active ? provider.implicitWidth : libraryActions.desiredWidth
            columns: headingMetrics.advanceWidth + accessoryWidth + columnSpacing <= headerPane.availableWidth ? 2 : 1
            columnSpacing: 8
            rowSpacing: 8

            TextMetrics {
                id: headingMetrics

                text: heading.text
                font: heading.font
            }

            Label {
                id: heading

                Layout.fillWidth: true
                text: dialog.title
                font.pixelSize: 20
                font.weight: Font.DemiBold
                color: Theme.text
                wrapMode: Text.WordWrap
                Accessible.role: Accessible.Heading
            }
            Label {
                id: provider

                Layout.alignment: headerLayout.columns === 1 ? Qt.AlignLeft : Qt.AlignRight
                visible: dialog.hub.active
                text: qsTr("Hugging Face")
                color: Theme.muted
            }
            Flow {
                id: libraryActions

                readonly property real desiredWidth: setupAnswers.implicitWidth + searchModels.implicitWidth + spacing
                Layout.preferredWidth: Math.min(headerPane.availableWidth, desiredWidth)
                spacing: 8

                Layout.alignment: headerLayout.columns === 1 ? Qt.AlignLeft : Qt.AlignRight
                visible: !dialog.hub.active

                ActionButton {
                    id: setupAnswers

                    enabled: dialog.library.ready
                    text: qsTr("Set Up Answers…")
                    objectName: "setupAnswersFromLibrary"
                    onClicked: dialog.setupRequested()
                }
                ActionButton {
                    id: searchModels

                    enabled: dialog.library.ready && !dialog.library.transferActive && dialog.library.pendingProfile === "" && !dialog.runtime.busy && !dialog.setupActive
                    text: qsTr("Search Models…")
                    objectName: "browseHuggingFace"
                    onClicked: dialog.hub.open() // qmllint disable compiler
                }
            }
        }
    }

    HubCatalogPage {
        id: catalogPage

        anchors.fill: parent
        visible: dialog.hub.active
        hub: dialog.hub
    }

    ColumnLayout {
        anchors.fill: parent
        visible: !dialog.hub.active
        spacing: 12

        RowLayout {
            Layout.fillWidth: true
            visible: dialog.library.catalogReady

            SurfaceInput {
                id: filter

                Layout.fillWidth: true
                objectName: "modelLibraryFilter"
                placeholderText: qsTr("Filter library…")
                Accessible.name: placeholderText
                text: dialog.library.query
                onTextEdited: dialog.library.query = text
            }
            ActionButton {
                visible: filter.text.length > 0
                text: qsTr("Clear Search")
                onClicked: {
                    dialog.library.query = "";
                    filter.forceActiveFocus();
                }
            }
        }

        ScrollView {
            id: scroll

            Layout.fillWidth: true
            Layout.fillHeight: true
            contentWidth: availableWidth
            clip: true

            ColumnLayout {
                width: scroll.availableWidth
                spacing: Theme.gap

                Label {
                    Layout.fillWidth: true
                    visible: text !== ""
                    text: {
                        if (!dialog.library.ready)
                            return qsTr("Backend session unavailable");
                        if (dialog.library.catalogFeedback !== "")
                            return dialog.library.catalogFeedback;
                        if (!dialog.library.catalogReady)
                            return qsTr("Loading the model library");
                        return "";
                    }
                    color: Theme.muted
                    wrapMode: Text.Wrap
                }

                LibraryModelSection {
                    Layout.fillWidth: true
                    visible: dialog.library.catalogReady && entries.count > 0
                    title: qsTr("Downloads")
                    library: dialog.library
                    entries: dialog.library.downloads
                    onDiscardRequested: function (candidate) {
                        discard.candidate = candidate;
                        discard.open();
                    }
                }

                Label {
                    Layout.fillWidth: true
                    visible: dialog.library.catalogReady && text !== ""
                    text: {
                        if (dialog.library.empty)
                            return qsTr("No models are available.");
                        const count = dialog.library.answerModels.count + dialog.library.embeddingModels.count + dialog.library.otherModels.count + dialog.library.downloads.count;
                        if (count === 0 && dialog.library.query.trim() !== "")
                            return qsTr("No catalog models match your search.");
                        return "";
                    }
                    color: Theme.muted
                    wrapMode: Text.Wrap
                }

                LibraryModelSection {
                    Layout.fillWidth: true
                    visible: dialog.library.catalogReady && entries.count > 0
                    title: qsTr("Answer models")
                    library: dialog.library
                    entries: dialog.library.answerModels
                }

                LibraryModelSection {
                    Layout.fillWidth: true
                    visible: dialog.library.catalogReady && entries.count > 0
                    title: qsTr("Embedding models")
                    library: dialog.library
                    entries: dialog.library.embeddingModels
                }

                LibraryModelSection {
                    Layout.fillWidth: true
                    visible: dialog.library.catalogReady && entries.count > 0
                    title: qsTr("Available models")
                    library: dialog.library
                    entries: dialog.library.otherModels
                }

                ColumnLayout {
                    Layout.fillWidth: true
                    visible: dialog.library.catalogReady && summary.text !== ""
                    spacing: 4

                    Label {
                        id: summary

                        Layout.fillWidth: true
                        text: dialog.library.storageSummary || dialog.library.storageFeedback
                        font.weight: Font.DemiBold
                        wrapMode: Text.Wrap
                    }
                    Label {
                        Layout.fillWidth: true
                        visible: text !== ""
                        text: dialog.library.storageDetails
                        color: Theme.muted
                        wrapMode: Text.Wrap
                    }
                }

                Label {
                    Layout.fillWidth: true
                    visible: text !== ""
                    text: dialog.library.feedbackProfile === "" ? dialog.library.feedback : ""
                    textFormat: Text.PlainText
                    color: Theme.danger
                    wrapMode: Text.Wrap
                }

                LocalRuntimeSection {
                    Layout.fillWidth: true
                    library: dialog.library
                    runtime: dialog.runtime
                }

                EmbeddingIndexSection {
                    Layout.fillWidth: true
                    index: dialog.embeddingIndex
                }
            }
        }
    }

    footer: DialogActions.Wrapping {
        contentItem: ColumnLayout {
            Flow {
                Layout.fillWidth: true
                visible: !dialog.hub.active
                layoutDirection: Qt.RightToLeft
                spacing: 8

                ActionButton {
                    text: qsTr("Done")
                    highlighted: true
                    onClicked: dialog.close()
                }
                ActionButton {
                    visible: dialog.library.ready
                    text: qsTr("Open Model Folder")
                    onClicked: {
                        const folder = dialog.library.modelFolder(); // qmllint disable compiler
                        if (folder.toString() === "" || !Qt.openUrlExternally(folder)) // qmllint disable compiler
                            folderError.open();
                    }
                }
                ActionButton {
                    visible: dialog.canRestart && (!dialog.library.ready || dialog.runtime.needsRestart)
                    text: dialog.library.ready ? qsTr("Restart Backend") : qsTr("Retry Connection")
                    onClicked: dialog.restartRequested()
                }
            }

            RowLayout {
                Layout.fillWidth: true
                visible: dialog.hub.active
                spacing: 8

                ActionButton {
                    visible: catalogPage.reviewing
                    enabled: !dialog.hub.addingDownload
                    text: qsTr("Back")
                    onClicked: {
                        dialog.hub.back(); // qmllint disable compiler
                        catalogPage.focusSearch();
                    }
                }
                Item {
                    Layout.fillWidth: true
                }
                ActionButton {
                    enabled: !dialog.hub.addingDownload
                    text: qsTr("Done")
                    onClicked: dialog.hub.close() // qmllint disable compiler
                }
                ActionButton {
                    visible: catalogPage.reviewing
                    enabled: dialog.hub.canDownload
                    highlighted: true
                    text: qsTr("Download")
                    objectName: "hubCatalogDownload"
                    onClicked: dialog.hub.download() // qmllint disable compiler
                }
            }
        }
    }

    ConfirmationDialog {
        id: discard

        property libraryModel candidate
        title: qsTr("Discard Partial Download?")
        message: qsTr("Discard %1 of partial data for %2? Installed weights are retained.").arg(candidate.partialSize).arg(candidate.label)
        closePolicy: Popup.CloseOnEscape
        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                onClicked: discard.close()
            }
            ActionButton {
                text: qsTr("Discard Partial Download")
                onClicked: {
                    dialog.library.discard(discard.candidate.profile); // qmllint disable compiler
                    discard.close();
                }
            }
        }
    }

    ConfirmationDialog {
        id: removal

        title: qsTr("Delete Model?")
        message: dialog.library.removalMessage
        visible: dialog.visible && message !== ""
        closePolicy: Popup.NoAutoClose

        Shortcut {
            sequences: [StandardKey.Cancel]
            enabled: removal.visible
            onActivated: dialog.library.cancelRemoval() // qmllint disable compiler
        }

        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                onClicked: dialog.library.cancelRemoval() // qmllint disable compiler
            }
            ActionButton {
                text: qsTr("Delete Model")
                onClicked: dialog.library.confirmRemoval() // qmllint disable compiler
            }
        }
    }

    ConfirmationDialog {
        id: folderError

        title: qsTr("Couldn’t Open the Model Folder")
        message: ""
        footer: DialogActions {
            ActionButton {
                text: qsTr("OK")
                onClicked: folderError.close()
            }
        }
    }
}
