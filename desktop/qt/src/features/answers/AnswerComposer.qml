pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: composer

    required property Answers answers
    signal modelLibraryRequested
    spacing: 8

    RowLayout {
        Layout.fillWidth: true

        Label {
            Layout.fillWidth: true
            text: qsTr("Question")
            color: Theme.text
            font.weight: Font.DemiBold
            Accessible.role: Accessible.Heading
        }
        ActionButton {
            text: qsTr("Manage Models…")
            objectName: "manageModels"
            onClicked: composer.modelLibraryRequested()
        }
    }

    Pane {
        Layout.fillWidth: true
        padding: 0

        background: Rectangle {
            color: Theme.surface
            radius: Theme.radius
            border.color: questionEditor.activeFocus ? Theme.accent : Theme.outline
        }
        contentItem: ColumnLayout {
            spacing: 0

            ScrollView {
                Layout.fillWidth: true
                Layout.preferredHeight: 148
                Layout.margins: 1
                contentWidth: availableWidth
                clip: true

                TextArea {
                    id: questionEditor

                    objectName: "answerQuestion"
                    text: composer.answers.question
                    placeholderText: qsTr("Ask about your local sources…")
                    Accessible.name: qsTr("Question")
                    textFormat: TextEdit.PlainText
                    color: Theme.text
                    font.pixelSize: 15
                    padding: 12
                    wrapMode: TextEdit.Wrap
                    background: null
                    onTextChanged: composer.answers.setQuestion(text) // qmllint disable compiler
                    Keys.onPressed: function (event) {
                        if ((event.key === Qt.Key_Return || event.key === Qt.Key_Enter) && composer.answers.canAsk && !(event.modifiers & Qt.ShiftModifier)) {
                            event.accepted = true;
                            composer.answers.ask(); // qmllint disable compiler
                        }
                    }
                }
            }
            Rectangle {
                Layout.fillWidth: true
                Layout.preferredHeight: 1
                color: Theme.outline
            }
            RowLayout {
                Layout.fillWidth: true
                Layout.margins: 9
                spacing: 8

                LocalizedComboBox {
                    Layout.fillWidth: true
                    Layout.maximumWidth: 280
                    visible: composer.answers.modelLabels.length > 0
                    enabled: !composer.answers.modelsBusy && !composer.answers.busy
                    objectName: "answerModel"
                    Accessible.name: qsTr("Model")
                    labels: composer.answers.modelLabels
                    selectedIndex: composer.answers.activeModelIndex
                    displayText: selectedIndex >= 0 && selectedIndex < labels.length ? labels[selectedIndex] : ""
                    onActivated: function (index) {
                        composer.answers.selectModel(index); // qmllint disable compiler
                    }
                }
                Item {
                    Layout.fillWidth: true
                }
                BusyIndicator {
                    Layout.preferredWidth: 20
                    Layout.preferredHeight: 20
                    visible: running
                    running: composer.answers.busy || composer.answers.modelsBusy
                }
                ActionButton {
                    visible: composer.answers.busy
                    enabled: !composer.answers.cancelling
                    objectName: "cancelAnswer"
                    text: qsTr("Cancel answer")
                    onClicked: composer.answers.cancelAnswer() // qmllint disable compiler
                }
                ActionButton {
                    visible: !composer.answers.busy
                    highlighted: true
                    enabled: composer.answers.canAsk
                    objectName: "askLocally"
                    text: qsTr("Ask locally")
                    onClicked: composer.answers.ask() // qmllint disable compiler
                }
            }
        }
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: {
            if (composer.answers.questionTooLong)
                return qsTr("Enter a question of up to 512 characters.");
            if (!composer.answers.ready)
                return qsTr("Backend session unavailable");
            if (composer.answers.modelFeedback !== "")
                return composer.answers.modelFeedback;
            return "";
        }
        color: Theme.muted
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
    }
}
