pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceGroup {
    id: result

    required property gitHubGraphOccurrence occurrence
    objectName: "githubGraph-" + occurrence.role + "-" + occurrence.resolution

    function roleText(): string {
        switch (occurrence.role) {
        case "definition":
            return qsTr("Definition");
        case "import":
            return qsTr("Import");
        case "call":
            return qsTr("Call");
        default:
            return occurrence.role;
        }
    }

    function resolutionText(): string {
        switch (occurrence.resolution) {
        case "resolved":
            return qsTr("Resolved");
        case "unresolved":
            return qsTr("Unresolved");
        case "ambiguous":
            return qsTr("Ambiguous");
        default:
            return occurrence.resolution;
        }
    }

    ColumnLayout {
        width: parent.width
        spacing: 4

        Label {
            Layout.fillWidth: true
            text: result.occurrence.name
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            font.weight: Font.DemiBold
        }
        Label {
            Layout.fillWidth: true
            text: result.occurrence.role === "definition" ? result.roleText() : "%1 · %2".arg(result.roleText()).arg(result.resolutionText())
            textFormat: Text.PlainText
            wrapMode: Text.WordWrap
            color: Theme.muted
        }
        SourceLink {
            Layout.fillWidth: true
            text: result.occurrence.sourceText
            destination: result.occurrence.sourceUrl
        }
        SourceLink {
            Layout.fillWidth: true
            visible: result.occurrence.targetText.length > 0
            text: "%1 · %2".arg(qsTr("Definition")).arg(result.occurrence.targetText)
            destination: result.occurrence.targetUrl
        }
    }

    component SourceLink: ItemDelegate {
        id: link

        required property url destination

        enabled: destination.toString().length > 0
        Accessible.role: Accessible.Link
        onClicked: Qt.openUrlExternally(destination)

        contentItem: Label {
            text: link.text
            textFormat: Text.PlainText
            wrapMode: Text.Wrap
            color: link.enabled ? Theme.accent : Theme.muted
            font.underline: link.enabled
        }
    }
}
