pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: section

    required property EmbeddingIndex index
    visible: index.loaded || index.feedback !== ""
    spacing: Theme.gap

    LibrarySection {
        Layout.fillWidth: true
        title: qsTr("Embedding Index")
        visible: section.index.loaded

        contentItem: RowLayout {
            Label {
                Layout.fillWidth: true
                text: qsTr("Semantic indexing")
                Accessible.ignored: true
                wrapMode: Text.Wrap
            }
            Switch {
                objectName: "embeddingIndexEnabled"
                Accessible.name: qsTr("Semantic indexing")
                Accessible.checkable: true
                Accessible.checked: checked
                checkable: false
                checked: section.index.enabled
                enabled: !section.index.writing
                palette.dark: enabled ? Theme.primary : Theme.outline
                palette.midlight: Theme.outline
                palette.window: Theme.accentText
                palette.light: Theme.accentText
                palette.mid: Theme.muted
                palette.highlight: Theme.accent
                onClicked: section.index.setEnabled(!checked) // qmllint disable compiler
            }
        }
    }

    LibrarySection {
        Layout.fillWidth: true
        title: qsTr("Index Maintenance")
        visible: section.index.loaded

        contentItem: RowLayout {
            Label {
                Layout.fillWidth: true
                text: qsTr("Stored embeddings")
                wrapMode: Text.Wrap
            }
            ActionButton {
                objectName: "embeddingIndexDelete"
                text: qsTr("Delete Index…")
                enabled: !section.index.writing
                onClicked: confirmation.open()
            }
        }
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: section.index.feedback || section.index.removalSummary
        color: Theme.muted
        textFormat: Text.PlainText
        wrapMode: Text.Wrap
    }

    ConfirmationDialog {
        id: confirmation

        title: qsTr("Delete Embedding Index?")
        message: qsTr("This removes embedding vectors. Your documents and lexical search remain available. The index can be rebuilt when enabled.")
        closePolicy: Popup.CloseOnEscape

        footer: DialogActions {
            ActionButton {
                text: qsTr("Cancel")
                onClicked: confirmation.close()
            }
            ActionButton {
                text: qsTr("Delete Index")
                destructive: true
                enabled: !section.index.writing
                onClicked: {
                    confirmation.close();
                    section.index.remove(); // qmllint disable compiler
                }
            }
        }
    }
}
