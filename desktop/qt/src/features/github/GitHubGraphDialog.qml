pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Controls
import QtQuick.Layouts

SurfaceDialog {
    id: dialog

    required property GitHubGraphBrowser browser
    required property bool refreshEnabled
    required property bool refreshing
    required property string refreshFeedback
    signal refreshRequested

    parent: Overlay.overlay
    anchors.centerIn: parent
    width: Math.min(600, parent.width - 24)
    height: Math.min(580, parent.height - 24, implicitHeight)
    implicitHeight: implicitHeaderHeight + implicitFooterHeight + topPadding + bottomPadding + graphContent.implicitHeight + Theme.gap
    modal: true
    visible: browser.visible
    closePolicy: Popup.CloseOnEscape
    title: qsTr("Code Graph")
    objectName: "githubGraphDialog"

    onOpened: {
        symbol.clear();
        viewport.contentY = viewport.originY;
    }
    onClosed: browser.close() // qmllint disable compiler

    ScrollView {
        id: scroll

        anchors.fill: parent
        contentWidth: availableWidth
        bottomPadding: Theme.gap
        clip: true

        Flickable {
            id: viewport

            contentWidth: scroll.availableWidth
            contentHeight: graphContent.implicitHeight

            ColumnLayout {
                id: graphContent

                width: scroll.availableWidth
                spacing: Theme.gap

                MetricRow {
                    title: qsTr("Branch")
                    value: dialog.browser.branch
                }
                MetricRow {
                    visible: value.length > 0
                    title: dialog.browser.access === GitHubGraphBrowser.Retained ? qsTr("Retained commit") : qsTr("Indexed commit")
                    value: dialog.browser.commit
                }
                Label {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: dialog.browser.feedback || dialog.refreshFeedback
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                }
                RowLayout {
                    visible: dialog.browser.busy && dialog.browser.access === GitHubGraphBrowser.Unknown

                    BusyIndicator {
                        Layout.preferredHeight: 24
                        Layout.preferredWidth: 24
                        running: parent.visible
                    }
                    Label {
                        Layout.fillWidth: true
                        text: qsTr("Loading code graph…")
                        wrapMode: Text.WordWrap
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: text.length > 0
                    text: dialog.coverageMessage()
                    color: Theme.muted
                    wrapMode: Text.WordWrap
                }
                ActionButton {
                    objectName: "githubGraphRefreshSource"
                    text: qsTr("Refresh Source")
                    visible: dialog.browser.access === GitHubGraphBrowser.Available
                    enabled: dialog.refreshEnabled && !dialog.refreshing && !dialog.browser.busy
                    onClicked: dialog.refreshRequested()
                }
                SurfaceGroup {
                    Layout.fillWidth: true
                    visible: dialog.browser.coverage === GitHubGraphBrowser.Published && dialog.browser.rootIndex >= 0
                    title: qsTr("Rust source root")

                    ColumnLayout {
                        width: parent.width
                        spacing: 8
                        enabled: !dialog.browser.busy

                        LocalizedComboBox {
                            Layout.fillWidth: true
                            objectName: "githubGraphRoot"
                            Accessible.name: qsTr("Rust source root")
                            labels: dialog.browser.roots
                            selectedIndex: dialog.browser.rootIndex
                            onActivated: function (index) {
                                dialog.browser.selectRoot(index); // qmllint disable compiler
                            }
                        }
                        Label {
                            Layout.fillWidth: true
                            text: dialog.browser.rootSummary
                            color: Theme.muted
                            wrapMode: Text.WordWrap
                        }
                        SurfaceInput {
                            id: symbol

                            Layout.fillWidth: true
                            objectName: "githubGraphSymbol"
                            Accessible.name: qsTr("Symbol")
                            placeholderText: qsTr("Name or crate::path::name")
                            maximumLength: 256
                            onAccepted: findSymbol.click()
                        }
                        ActionButton {
                            id: findSymbol

                            objectName: "githubGraphFind"
                            text: qsTr("Find Symbol")
                            highlighted: true
                            enabled: symbol.text.trim().length > 0
                            onClicked: dialog.browser.lookup(symbol.text) // qmllint disable compiler
                        }
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: dialog.browser.searchedSymbol.length > 0
                    objectName: "githubGraphResults"
                    text: qsTr("Results for %1 in %2").arg(dialog.browser.searchedSymbol).arg(dialog.browser.searchedRoot)
                    textFormat: Text.PlainText
                    wrapMode: Text.Wrap
                    font.weight: Font.DemiBold
                    Accessible.role: Accessible.Heading
                }
                Repeater {
                    model: dialog.browser.hits

                    GitHubGraphResult {
                        required property gitHubGraphOccurrence modelData

                        Layout.fillWidth: true
                        occurrence: modelData
                    }
                }
                Label {
                    Layout.fillWidth: true
                    visible: dialog.browser.searchedSymbol.length > 0 && dialog.browser.hits.length === 0
                    text: qsTr("No matching symbol in this source root.")
                    wrapMode: Text.WordWrap
                }
                Label {
                    Layout.fillWidth: true
                    visible: dialog.browser.truncated
                    text: qsTr("Showing the first 50 occurrences.")
                    wrapMode: Text.WordWrap
                }
            }
        }
    }

    function coverageMessage(): string {
        switch (browser.coverage) {
        case GitHubGraphBrowser.NoConventionalRustRoots:
            return qsTr("No src/lib.rs or src/main.rs found in this snapshot.");
        case GitHubGraphBrowser.Limit:
            return qsTr("This snapshot exceeds the graph projection limit.");
        case GitHubGraphBrowser.NoSnapshot:
            return browser.access !== GitHubGraphBrowser.Unknown ? qsTr("No graph has been published for this branch.") : "";
        default:
            return "";
        }
    }

    footer: DialogActions {
        ActionButton {
            objectName: "githubGraphBack"
            text: qsTr("Back")
            onClicked: dialog.browser.close() // qmllint disable compiler
        }
        ActionButton {
            objectName: "githubGraphReload"
            text: qsTr("Reload")
            enabled: !dialog.browser.busy
            onClicked: dialog.browser.reload() // qmllint disable compiler
        }
    }
}
