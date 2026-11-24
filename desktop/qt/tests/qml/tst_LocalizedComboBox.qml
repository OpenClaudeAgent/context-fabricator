import QtQuick
import QtTest
import "../../src/ui" as Product

Item {
    width: 360
    height: 200

    Product.LocalizedComboBox {
        id: combo

        labels: ["One day", "Seven days", "Thirty days"]
        selectedIndex: 2
        onActivated: index => selectedIndex = index
    }

    TestCase {
        name: "LocalizedComboBox"
        when: windowShown

        function test_changesLabelsWithoutLosingSelection() {
            compare(combo.currentIndex, 2);
            compare(combo.currentText, "Thirty days");

            combo.labels = ["Un jour", "Sept jours", "Trente jours"];
            compare(combo.currentIndex, 2);
            compare(combo.currentText, "Trente jours");

            combo.forceActiveFocus();
            keyClick(Qt.Key_S);
            compare(combo.currentIndex, 1);
            compare(combo.currentText, "Sept jours");
        }
    }
}
