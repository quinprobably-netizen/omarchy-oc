import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../Model.js" as Model

// "N changes to "custom" · Discard · Apply": the staged risky edits, read
// from root.staged. Collapses to nothing while there is nothing staged.
// Panel.qml wires apply() to root.applyStaged() and discard() to
// root.discardStaged().
Item {
  id: bar

  required property var root

  signal apply()
  signal discard()

  readonly property int count: Model.stagedCount(root.staged)
  readonly property bool shown: count > 0
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
    color: Qt.rgba(bar.accent.r, bar.accent.g, bar.accent.b, 0.16)
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
        width: parent.width - discardButton.width - applyButton.width - parent.spacing * 2
        anchors.verticalCenter: parent.verticalCenter
        elide: Text.ElideRight
        textFormat: Text.PlainText
        text: bar.root.strings.tn("stage.changes", bar.count).arg(bar.count).arg(bar.root.staged ? String(bar.root.staged.profile) : "")
        color: bar.fg
        font.family: bar.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Button {
        id: discardButton
        anchors.verticalCenter: parent.verticalCenter
        text: bar.root.strings.t("stage.discard")
        fontSize: Style.font.caption
        foreground: bar.fg
        fontFamily: bar.fontFamily
        bordered: true
        onClicked: bar.discard()
      }
      Button {
        id: applyButton
        anchors.verticalCenter: parent.verticalCenter
        text: bar.root.strings.t("stage.apply")
        fontSize: Style.font.caption
        foreground: bar.fg
        accent: bar.accent
        fontFamily: bar.fontFamily
        bordered: true
        selected: true
        onClicked: bar.apply()
      }
    }
  }
}
