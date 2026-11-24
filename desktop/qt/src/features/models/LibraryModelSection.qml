pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

LibrarySection {
    id: section

    required property ModelLibrary library
    required property LibraryEntries entries
    signal discardRequested(libraryModel candidate)

    visible: entries.count > 0
    contentItem: ColumnLayout {
        spacing: 8

        Repeater {
            model: section.entries

            delegate: LibraryModelRow {
                required property int index

                Layout.fillWidth: true
                library: section.library
                separator: index > 0
                onDiscardRequested: function (candidate) {
                    section.discardRequested(candidate);
                }
            }
        }
    }
}
