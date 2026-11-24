pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

ColumnLayout {
    id: page

    required property HubCatalog hub
    readonly property bool reviewing: hub.repositoryName !== ""
    readonly property bool embedding: hub.kind === HubCatalog.Embedding
    spacing: 12

    function focusSearch(): void {
        query.forceActiveFocus();
    }

    onVisibleChanged: {
        if (visible)
            focusSearch();
    }

    TabBar {
        Layout.fillWidth: true
        enabled: !page.hub.addingDownload
        Accessible.name: qsTr("Model role")
        currentIndex: page.embedding ? 1 : 0
        spacing: 0

        background: Rectangle {
            radius: Theme.controlRadius
            color: Theme.raised
        }

        SegmentTab {
            text: qsTr("Answer Models")
            objectName: "hubAnswerModels"
            onClicked: page.hub.selectKind(HubCatalog.Generation) // qmllint disable compiler
        }
        SegmentTab {
            text: qsTr("Embedding Models")
            objectName: "hubEmbeddingModels"
            onClicked: page.hub.selectKind(HubCatalog.Embedding) // qmllint disable compiler
        }
    }

    RowLayout {
        Layout.fillWidth: true
        visible: !page.reviewing
        spacing: 8

        SurfaceInput {
            id: query

            Layout.fillWidth: true
            objectName: "hubCatalogQuery"
            placeholderText: qsTr("Model name or publisher")
            Accessible.name: placeholderText
            text: page.hub.query
            onTextEdited: page.hub.query = text
            onAccepted: page.hub.search() // qmllint disable compiler
        }
        ActionButton {
            text: qsTr("Search")
            objectName: "hubCatalogSearch"
            enabled: !page.hub.busy && query.text.trim() !== ""
            onClicked: page.hub.search() // qmllint disable compiler
        }
    }

    ColumnLayout {
        Layout.fillWidth: true
        visible: page.reviewing
        spacing: 3

        Label {
            Layout.fillWidth: true
            text: qsTr("Compatible files")
            Accessible.role: Accessible.Heading
            font.weight: Font.DemiBold
            color: Theme.text
            wrapMode: Text.Wrap
        }

        ItemDelegate {
            id: repositoryLink

            Layout.fillWidth: true
            padding: 0
            text: page.hub.repositoryName
            Accessible.role: Accessible.Link
            onClicked: Qt.openUrlExternally(page.hub.repositoryUrl)

            contentItem: Label {
                text: repositoryLink.text
                textFormat: Text.PlainText
                color: Theme.accent
                elide: Text.ElideMiddle
                font.underline: repositoryLink.hovered || repositoryLink.visualFocus
            }
            background: Item {}
        }
    }

    Label {
        Layout.fillWidth: true
        visible: text !== ""
        text: page.hub.feedback
        textFormat: Text.PlainText
        color: Theme.danger
        wrapMode: Text.Wrap
    }

    ScrollView {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: !page.hub.busy && results.count > 0
        clip: true

        ListView {
            id: results

            model: page.reviewing ? page.hub.variants : page.hub.repositories
            spacing: 6
            currentIndex: -1

            delegate: ItemDelegate {
                id: row

                required property hubCatalogRow modelData
                required property int index
                readonly property bool selected: page.reviewing && index === page.hub.selectedVariantIndex
                readonly property string detail: {
                    if (!page.reviewing)
                        return "";
                    return modelData.selectable ? modelData.size : qsTr("Verification unavailable");
                }

                width: ListView.view.width
                objectName: page.reviewing ? "hubCatalogFile" : "hubCatalogRepository"
                enabled: modelData.selectable && !page.hub.busy
                focusPolicy: Qt.StrongFocus
                padding: 10
                Accessible.name: modelData.label
                Accessible.description: detail
                Accessible.selected: selected
                onClicked: {
                    if (page.reviewing) {
                        page.hub.selectVariant(index); // qmllint disable compiler
                    } else {
                        page.hub.openRepository(index); // qmllint disable compiler
                    }
                }

                background: Rectangle {
                    radius: Theme.controlRadius
                    color: row.selected || row.hovered ? Theme.hovered : Theme.surface
                    border.color: row.visualFocus || row.selected ? Theme.accent : Theme.outline
                    border.width: row.visualFocus ? 2 : 1
                }

                contentItem: RowLayout {
                    spacing: 10

                    ColumnLayout {
                        Layout.fillWidth: true
                        spacing: 3

                        Label {
                            Layout.fillWidth: true
                            text: row.modelData.label
                            textFormat: Text.PlainText
                            elide: Text.ElideMiddle
                            font.weight: Font.DemiBold
                            color: row.enabled ? Theme.text : Theme.muted
                        }
                        Label {
                            Layout.fillWidth: true
                            visible: text !== ""
                            text: row.detail
                            color: Theme.muted
                            wrapMode: Text.Wrap
                        }
                    }

                    Label {
                        text: page.reviewing ? (row.selected ? "✓" : "○") : "›"
                        color: row.selected ? Theme.accent : Theme.muted
                        Accessible.ignored: true
                    }
                }
            }
        }
    }

    Item {
        Layout.fillWidth: true
        Layout.fillHeight: true
        visible: page.hub.busy || results.count === 0

        ColumnLayout {
            anchors.centerIn: parent
            width: Math.min(400, parent.width)
            spacing: 8

            BusyIndicator {
                Layout.alignment: Qt.AlignHCenter
                visible: page.hub.busy
                running: visible
            }

            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: {
                    if (page.hub.busy)
                        return page.hub.busyText;
                    if (page.hub.feedback !== "")
                        return "";
                    if (page.reviewing)
                        return page.hub.variantsReady ? qsTr("No Compatible Files") : "";
                    return page.hub.hasResults ? qsTr("No Models Found") : qsTr("Search Hugging Face");
                }
                Accessible.role: Accessible.Heading
                color: Theme.text
                font.weight: Font.DemiBold
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
            }
            Label {
                Layout.fillWidth: true
                visible: text !== ""
                text: {
                    if (page.hub.busy || page.hub.feedback !== "")
                        return "";
                    if (page.reviewing)
                        return page.hub.variantsReady ? qsTr("Return to search results and choose another model.") : "";
                    if (!page.hub.hasResults)
                        return page.embedding ? qsTr("Find public Qwen3-Embedding-0.6B GGUF models compatible with semantic search.") : qsTr("Find public Qwen3 4B Instruct models compatible with local Answers.");
                    return page.embedding ? qsTr("Try another name or publisher for a public Qwen3-Embedding-0.6B GGUF model.") : qsTr("Try another model name or publisher.");
                }
                color: Theme.muted
                horizontalAlignment: Text.AlignHCenter
                wrapMode: Text.Wrap
            }
        }
    }
}
