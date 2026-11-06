import QtQuick
import qs.Commons
import qs.Commons as Commons

// A pick-one strip: a dim pill with the chosen segment filled.
//
// `options` is a string[] (label == value) or an array of {value, label}.
// The caller owns the value: bind `value` to real state and act on
// `changed(value)`. Set `enabled: false` to dim it and refuse clicks.
Rectangle {
  id: seg

  property var options: []
  property string value: ""
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property string fontFamily: Style.font.family
  property real fontSize: Style.font.caption

  signal changed(string value)

  readonly property color dim: Qt.darker(foreground, 1.4)
  readonly property int pad: Style.space(2)

  function valueOf(o) { return o && typeof o === "object" ? String(o.value) : String(o) }
  function labelOf(o) { return o && typeof o === "object" && o.label !== undefined ? String(o.label) : String(o) }

  implicitWidth: row.implicitWidth + pad * 2
  implicitHeight: row.implicitHeight + pad * 2
  radius: Style.cornerRadius
  color: Qt.rgba(foreground.r, foreground.g, foreground.b, 0.06)
  opacity: enabled ? 1 : 0.45

  Row {
    id: row
    x: seg.pad
    y: seg.pad
    spacing: seg.pad

    Repeater {
      model: seg.options

      Rectangle {
        id: cell
        required property var modelData
        readonly property string v: seg.valueOf(modelData)
        readonly property bool on: v === seg.value

        implicitWidth: label.implicitWidth + Style.space(16)
        implicitHeight: label.implicitHeight + Style.space(6)
        radius: seg.radius > 0 ? Math.max(2, seg.radius - seg.pad) : 0
        color: on ? Style.selectedFillFor(seg.foreground, seg.accent)
          : mouse.containsMouse ? Qt.rgba(seg.foreground.r, seg.foreground.g, seg.foreground.b, 0.08)
          : "transparent"

        Behavior on color { ColorAnimation { duration: Style.duration(120) } }

        Text {
          id: label
          anchors.centerIn: parent
          textFormat: Text.PlainText
          text: seg.labelOf(cell.modelData)
          color: cell.on ? seg.foreground : seg.dim
          font.family: seg.fontFamily
          font.pixelSize: seg.fontSize
          font.bold: cell.on
        }

        MouseArea {
          id: mouse
          anchors.fill: parent
          enabled: seg.enabled
          hoverEnabled: true
          cursorShape: Qt.PointingHandCursor
          onClicked: if (!cell.on) seg.changed(cell.v)
        }
      }
    }
  }
}
