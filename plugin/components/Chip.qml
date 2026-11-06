import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons

// A profile pill. `on` is the active profile; `warn` marks a risky profile
// that has never been kept through its Keep/Revert window.
Rectangle {
  id: chip

  property string icon: ""
  property string text: ""
  property bool on: false
  property bool warn: false
  property string tooltipText: ""
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property color urgent: Commons.Color.urgent
  property string fontFamily: Style.font.family

  signal clicked()

  readonly property bool hot: mouseArea.containsMouse
  readonly property color warnColor: urgent

  implicitWidth: label.implicitWidth + Style.space(20)
  implicitHeight: label.implicitHeight + Style.space(8)
  radius: height / 2
  color: on ? accent
    : hot ? Qt.rgba(foreground.r, foreground.g, foreground.b, 0.12)
    : Qt.rgba(foreground.r, foreground.g, foreground.b, 0.06)
  border.width: warn && !on ? 1 : 0
  border.color: Qt.rgba(warnColor.r, warnColor.g, warnColor.b, 0.7)

  Behavior on color { ColorAnimation { duration: Style.duration(120) } }
  scale: mouseArea.pressed ? 0.97 : 1
  Behavior on scale {
    NumberAnimation { duration: Style.duration(160); easing.type: Easing.Bezier; easing.bezierCurve: Style.easeOut }
  }

  Text {
    id: label
    anchors.centerIn: parent
    textFormat: Text.PlainText
    text: (chip.icon !== "" ? chip.icon + " " : "") + chip.text
    color: chip.on ? Commons.Color.background : (chip.warn ? chip.warnColor : chip.foreground)
    font.family: chip.fontFamily
    font.pixelSize: Style.font.bodySmall
    font.bold: chip.on
  }

  ToolTip {
    visible: chip.tooltipText !== "" && mouseArea.containsMouse
    text: chip.tooltipText
    delay: 400
    contentItem: Text {
      textFormat: Text.PlainText
      text: chip.tooltipText
      color: Commons.Color.tooltip.text
      font.family: chip.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
    background: Rectangle {
      color: Commons.Color.tooltip.background
      border.width: 1
      border.color: Commons.Color.tooltip.border
    }
  }

  MouseArea {
    id: mouseArea
    anchors.fill: parent
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onClicked: chip.clicked()
  }
}
