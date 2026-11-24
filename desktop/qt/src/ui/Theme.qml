pragma Singleton

import QtQuick

QtObject {
    property string fixedFont: "monospace"

    readonly property color canvas: "#1c1c1d"
    readonly property color rail: "#141415"
    readonly property color surface: "#232324"
    readonly property color raised: "#2d2d2e"
    readonly property color hovered: "#343437"
    readonly property color field: "#1a1a1b"
    readonly property color outline: "#3e3e40"
    readonly property color text: "#e2e2e3"
    readonly property color muted: "#a8a8aa"
    readonly property color disabled: "#88888b"
    readonly property color accent: "#8fb8ff"
    readonly property color primary: "#2458ca"
    readonly property color accentText: "#ffffff"
    readonly property color selection: "#263651"
    readonly property color statusReady: "#60d980"
    readonly property color statusWaiting: "#f2c775"
    readonly property color danger: "#ff8e99"
    readonly property color scrim: "#a1000000"

    readonly property int margin: 24
    readonly property int workspaceWidth: 760
    readonly property int gap: 16
    readonly property int radius: 10
    readonly property int controlRadius: 6
    readonly property int dialogMargin: 18
}
