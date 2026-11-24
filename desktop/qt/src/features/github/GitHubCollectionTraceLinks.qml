pragma ComponentBehavior: Bound

import QtQuick

Flow {
    id: group

    required property list<gitHubCollectionTraceLink> links
    required property string feature
    signal traceRequested(string traceId, string spanId)

    spacing: 8
    visible: links.length > 0

    Repeater {
        model: group.links

        ActionButton {
            required property gitHubCollectionTraceLink modelData

            objectName: "github-" + group.feature + "-trace-" + modelData.kind
            text: modelData.label
            onClicked: group.traceRequested(modelData.traceId, modelData.spanId)
        }
    }
}
