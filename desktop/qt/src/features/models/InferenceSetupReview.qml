pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: review

    required property InferenceSetup workflow
    required property libraryModel answerModel
    required property libraryModel embeddingModel
    required property string archiveName
    signal chooseArchive

    contentItem: ColumnLayout {
        spacing: 10

        ReviewedModel {
            title: qsTr("Answer model")
            model: review.answerModel
            profile: review.workflow.planAnswerProfile
            installed: review.workflow.planAnswerInstalled
        }
        ReviewedModel {
            visible: profile !== ""
            title: qsTr("Semantic search")
            model: review.embeddingModel
            profile: review.workflow.planEmbeddingProfile
            installed: review.workflow.planEmbeddingInstalled
        }
        MetricRow {
            visible: review.workflow.planRuntimeRequired
            title: qsTr("Inference engine")
            value: review.workflow.planNeedsArchive ? qsTr("Local ZIP") : qsTr("GitHub Releases")
        }
        ActionButton {
            visible: review.workflow.planNeedsArchive
            text: qsTr("Choose Runtime ZIP…")
            onClicked: review.chooseArchive()
        }
        Label {
            Layout.fillWidth: true
            visible: review.workflow.planNeedsArchive
            text: review.archiveName || qsTr("Choose a runtime ZIP before starting.")
            textFormat: Text.PlainText
            color: Theme.muted
            wrapMode: Text.Wrap
        }
        Rectangle {
            Layout.fillWidth: true
            implicitHeight: 1
            color: Theme.outline
        }
        MetricRow {
            title: qsTr("Total download")
            value: review.workflow.planTransfer
        }
        MetricRow {
            objectName: "setupSpaceEstimate"
            title: qsTr("Peak extra disk space")
            value: review.workflow.planPeak
        }
        MetricRow {
            title: qsTr("Available space")
            value: review.workflow.planAvailable
        }
        Label {
            Layout.fillWidth: true
            visible: text !== ""
            text: review.workflow.planBlocker
            color: Theme.danger
            wrapMode: Text.Wrap
        }
    }

    component ReviewedModel: RowLayout {
        id: row

        required property string title
        required property libraryModel model
        required property string profile
        required property bool installed
        Layout.fillWidth: true
        spacing: 10

        Label {
            Layout.fillWidth: true
            text: row.title
            color: Theme.muted
            wrapMode: Text.Wrap
        }
        ColumnLayout {
            Layout.maximumWidth: row.width * 0.6
            spacing: 3

            Label {
                Layout.fillWidth: true
                text: row.model.profile !== "" ? row.model.label : row.profile
                textFormat: Text.PlainText
                font.weight: Font.DemiBold
                horizontalAlignment: Text.AlignRight
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: row.installed ? qsTr("Already installed") : row.model.profile !== "" ? row.model.size : ""
                color: Theme.muted
                horizontalAlignment: Text.AlignRight
                wrapMode: Text.Wrap
            }
            SetupModelSource {
                Layout.alignment: Qt.AlignRight
                model: row.model
            }
        }
    }
}
