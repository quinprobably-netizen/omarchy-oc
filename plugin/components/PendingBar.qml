import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../Model.js" as Model

// "Keep "custom"? 14 s · Keep · Revert": the Keep/Revert window, read from
// root.status.pending, under every page (the spec wants the countdown on
// the page and in the bar). Collapses to nothing while nothing is pending.
// Panel.qml wires keep() to root.keep() and revert() to root.revert().
Item {
  id: bar

  required property var root

  signal keep()
  signal revert()

  readonly property var info: Model.pendingInfo(root.status)
  readonly property bool shown: info !== null
  readonly property color fg: root.fg
  readonly property color accent: root.accent
  readonly property string fontFamily: root.fontFamily

  implicitHeight: shown ? card.implicitHeight : 0
  visible: shown || card.opacity > 0
  clip: true

  Behavior on implicitHeight { NumberAnimation { duration: Style.duration(180); easing.type: Easing.OutCubic } }

  Rectangle {
    id: card
    width: parent.width
    implicitHeight: row.implicitHeight + Style.space(14)
    radius: Style.cornerRadius
    color: Qt.rgba(bar.accent.r, bar.accent.g, bar.accent.b, 0.14)
    border.width: 1
    border.color: Qt.rgba(bar.accent.r, bar.accent.g, bar.accent.b, 0.5)
    opacity: bar.shown ? 1 : 0
    scale: bar.shown ? 1 : 0.97
    transformOrigin: Item.Top

    Behavior on opacity { NumberAnimation { duration: Style.duration(bar.shown ? 180 : 120); easing.type: Easing.OutCubic } }
    Behavior on scale {
      NumberAnimation { duration: Style.duration(180); easing.type: Easing.Bezier; easing.bezierCurve: Style.easeOut }
    }

    Row {
      id: row
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(8)

      Text {
        width: parent.width - keepButton.width - revertButton.width - parent.spacing * 2
        anchors.verticalCenter: parent.verticalCenter
        elide: Text.ElideRight
        textFormat: Text.PlainText
        text: bar.info ? bar.root.strings.t("pending.bar").arg(bar.info.label).arg(bar.info.remaining) : ""
        color: bar.fg
        font.family: bar.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Button {
        id: keepButton
        anchors.verticalCenter: parent.verticalCenter
        text: bar.root.strings.t("pending.keep")
        fontSize: Style.font.caption
        foreground: bar.fg
        accent: bar.accent
        fontFamily: bar.fontFamily
        bordered: true
        selected: true
        onClicked: bar.keep()
      }
      Button {
        id: revertButton
        anchors.verticalCenter: parent.verticalCenter
        text: bar.root.strings.t("pending.revert")
        fontSize: Style.font.caption
        foreground: bar.fg
        fontFamily: bar.fontFamily
        bordered: true
        onClicked: bar.revert()
      }
    }
  }
}
