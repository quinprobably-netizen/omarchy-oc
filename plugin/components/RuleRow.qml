import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons

// One automation rule: "‹when›   [profile]   ✕". Clicking the profile pill
// unfolds a picker under the row. The caller owns the values: bind `label`,
// `profile` and `profiles`, act on `picked(name)` and `removed()`.
// `label` is daemon text (a process name), so every sink is PlainText.
Column {
  id: rule

  property string label: ""
  property string profile: ""
  property string profileLabel: profile
  property var profiles: []
  property string removeTooltip: ""
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property color urgent: Commons.Color.urgent
  property string fontFamily: Style.font.family
  // Owned by the caller too (the page keeps which row is open, so a
  // rebuilt delegate comes back open): bind it and act on `toggled()`.
  property bool picking: false

  signal toggled()
  signal picked(string profile)
  signal removed()

  readonly property color dim: Qt.darker(foreground, 1.4)

  spacing: 0
  opacity: enabled ? 1 : 0.45

  Rectangle {
    width: parent.width
    implicitHeight: line.implicitHeight + Style.space(10)
    radius: Style.cornerRadius
    color: Qt.rgba(rule.foreground.r, rule.foreground.g, rule.foreground.b, 0.06)

    Row {
      id: line
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(8)

      Text {
        width: parent.width - pill.width - remove.width - parent.spacing * 2
        anchors.verticalCenter: parent.verticalCenter
        elide: Text.ElideRight
        textFormat: Text.PlainText
        text: rule.label
        color: rule.foreground
        font.family: rule.fontFamily
        font.pixelSize: Style.font.bodySmall
      }

      // The profile pill: accent fill, opens the picker.
      Rectangle {
        id: pill
        anchors.verticalCenter: parent.verticalCenter
        implicitWidth: pillText.implicitWidth + Style.space(18)
        implicitHeight: pillText.implicitHeight + Style.space(6)
        radius: height / 2
        color: rule.picking ? rule.accent
          : pillArea.containsMouse ? Qt.rgba(rule.accent.r, rule.accent.g, rule.accent.b, 0.4)
          : Qt.rgba(rule.accent.r, rule.accent.g, rule.accent.b, 0.25)
        Behavior on color { ColorAnimation { duration: Style.duration(120) } }

        Text {
          id: pillText
          anchors.centerIn: parent
          textFormat: Text.PlainText
          text: rule.profileLabel
          color: rule.picking ? Commons.Color.background : rule.foreground
          font.family: rule.fontFamily
          font.pixelSize: Style.font.caption
          font.bold: true
        }
        MouseArea {
          id: pillArea
          anchors.fill: parent
          enabled: rule.enabled
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          onClicked: rule.toggled()
        }
      }

      Rectangle {
        id: remove
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(22)
        height: Style.space(22)
        radius: Style.cornerRadius
        color: removeArea.containsMouse ? Qt.rgba(rule.urgent.r, rule.urgent.g, rule.urgent.b, 0.18) : "transparent"
        Behavior on color { ColorAnimation { duration: Style.duration(120) } }

        Text {
          anchors.centerIn: parent
          textFormat: Text.PlainText
          text: "✕"
          color: removeArea.containsMouse ? rule.urgent : rule.dim
          font.family: rule.fontFamily
          font.pixelSize: Style.font.caption
          font.bold: true
        }
        MouseArea {
          id: removeArea
          anchors.fill: parent
          enabled: rule.enabled
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          onClicked: rule.removed()
        }
        ToolTip {
          visible: rule.removeTooltip !== "" && removeArea.containsMouse
          text: rule.removeTooltip
          delay: 400
          contentItem: Text {
            textFormat: Text.PlainText
            text: rule.removeTooltip
            color: Commons.Color.tooltip.text
            font.family: rule.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
          background: Rectangle {
            color: Commons.Color.tooltip.background
            border.width: 1
            border.color: Commons.Color.tooltip.border
          }
        }
      }
    }
  }

  // The picker unfolds under the row (Style.duration, ease-out).
  Item {
    width: parent.width
    implicitHeight: rule.picking ? picker.implicitHeight + Style.space(6) : 0
    visible: rule.picking || picker.opacity > 0
    clip: true
    Behavior on implicitHeight { NumberAnimation { duration: Style.duration(160); easing.type: Easing.OutCubic } }

    Segmented {
      id: picker
      x: Style.space(8)
      y: Style.space(6)
      options: rule.profiles
      value: rule.profile
      enabled: rule.enabled
      opacity: rule.picking ? 1 : 0
      Behavior on opacity { NumberAnimation { duration: Style.duration(rule.picking ? 160 : 120); easing.type: Easing.OutCubic } }
      foreground: rule.foreground; accent: rule.accent; fontFamily: rule.fontFamily
      onChanged: function(v) { rule.picked(v) }
    }
  }
}
