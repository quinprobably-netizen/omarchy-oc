import QtQuick
import QtQuick.Controls
import qs.Commons
import qs.Commons as Commons
import "../Model.js" as Model

// One limit: label, a track with the stock value as a gold tick and the
// live draw as a green strip underneath, and the value readout.
//
//   value           the limit in force (null: unknown, knob hidden)
//   stock           gold tick (null hides it)
//   use             green strip, in the slider's units (null hides it)
//   disabledReason  non-empty dims the control, refuses input and shows
//                   the reason as a tooltip
//
// `moved(v)` fires on every drag step and wheel notch, already snapped to
// `step` and clamped to from..to. The caller owns the value; while a drag
// is in progress the knob follows the pointer regardless.
Item {
  id: slider

  property string label: ""
  property var value: null
  property var stock: null
  property var use: null
  property real from: 0
  property real to: 100
  property real step: 1
  property string unit: ""
  property string emptyText: "—"
  property string readout: ""
  property string disabledReason: ""
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property color stockColor: "#e8c26a"
  property color useColor: "#8fd694"
  property string fontFamily: Style.font.family
  property real labelWidth: Style.space(78)
  property real readoutWidth: Style.space(62)

  signal moved(real v)

  readonly property bool active: disabledReason === "" && enabled
  readonly property color dim: Qt.darker(foreground, 1.4)
  property bool dragging: false
  property real liveValue: 0
  readonly property var shownValue: dragging ? liveValue : Model.num(value)
  readonly property bool hasValue: shownValue !== null
  readonly property real range: Math.max(0.0001, to - from)
  readonly property real knobSize: Style.space(14)

  function frac(v) {
    var n = Model.num(v)
    return n === null ? 0 : Math.max(0, Math.min(1, (n - from) / range))
  }

  function snap(raw) {
    var v = step > 0 ? Math.round(raw / step) * step : raw
    return Math.max(from, Math.min(to, v))
  }

  implicitHeight: Style.space(30)
  opacity: active ? 1 : 0.45

  Text {
    id: labelText
    width: slider.labelWidth
    anchors.left: parent.left
    anchors.verticalCenter: parent.verticalCenter
    elide: Text.ElideRight
    textFormat: Text.PlainText
    text: slider.label
    color: slider.dim
    font.family: slider.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  Item {
    id: area
    anchors.left: labelText.right
    anchors.leftMargin: Style.space(8)
    anchors.right: readoutText.left
    anchors.rightMargin: Style.space(8)
    anchors.top: parent.top
    anchors.bottom: parent.bottom

    Rectangle {
      id: track
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.leftMargin: slider.knobSize / 2
      anchors.rightMargin: slider.knobSize / 2
      y: Style.space(7)
      height: Style.space(6)
      radius: height / 2
      color: Qt.rgba(slider.foreground.r, slider.foreground.g, slider.foreground.b, 0.1)

      Rectangle {
        id: fill
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        height: parent.height
        radius: parent.radius
        width: slider.hasValue ? parent.width * slider.frac(slider.shownValue) : 0
        color: slider.accent
        Behavior on width {
          enabled: !slider.dragging
          NumberAnimation { duration: Style.duration(140); easing.type: Easing.OutCubic }
        }
      }

      // Stock value: a gold tick through the track.
      Rectangle {
        visible: Model.num(slider.stock) !== null
        width: Math.max(2, Style.space(2))
        height: parent.height + Style.space(4)
        anchors.verticalCenter: parent.verticalCenter
        x: parent.width * slider.frac(slider.stock) - width / 2
        radius: 1
        color: slider.stockColor
      }
    }

    Rectangle {
      id: knob
      visible: slider.hasValue
      width: slider.knobSize
      height: slider.knobSize
      radius: width / 2
      anchors.verticalCenter: track.verticalCenter
      x: track.x + track.width * slider.frac(slider.shownValue) - width / 2
      color: slider.foreground
      border.width: Math.max(1, Style.space(2))
      border.color: Commons.Color.popups.background
      scale: mouse.containsMouse || slider.dragging ? 1.15 : 1
      Behavior on x {
        enabled: !slider.dragging
        NumberAnimation { duration: Style.duration(140); easing.type: Easing.OutCubic }
      }
      Behavior on scale { NumberAnimation { duration: Style.duration(110); easing.type: Easing.OutCubic } }
    }

    // Live draw against the limit.
    Rectangle {
      id: useTrack
      visible: Model.num(slider.use) !== null
      anchors.left: track.left
      anchors.right: track.right
      y: track.y + track.height + Style.space(3)
      height: Style.space(4)
      radius: height / 2
      color: Qt.rgba(slider.foreground.r, slider.foreground.g, slider.foreground.b, 0.08)

      Rectangle {
        anchors.left: parent.left
        height: parent.height
        radius: parent.radius
        width: parent.width * slider.frac(slider.use)
        color: slider.useColor
        Behavior on width { NumberAnimation { duration: Style.duration(200); easing.type: Easing.OutCubic } }
      }
    }

    MouseArea {
      id: mouse
      anchors.fill: parent
      enabled: slider.active
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor

      function valueFromX(x) {
        if (track.width <= 0) return slider.from
        var f = Math.max(0, Math.min(1, (x - track.x) / track.width))
        return slider.snap(slider.from + f * slider.range)
      }

      onPressed: function(m) {
        slider.dragging = true
        slider.liveValue = valueFromX(m.x)
        slider.moved(slider.liveValue)
      }
      onPositionChanged: function(m) {
        if (!slider.dragging) return
        var v = valueFromX(m.x)
        if (v === slider.liveValue) return
        slider.liveValue = v
        slider.moved(v)
      }
      onReleased: slider.dragging = false
      onCanceled: slider.dragging = false
      onWheel: function(w) {
        var base = slider.hasValue ? Number(slider.shownValue) : (Model.num(slider.stock) !== null ? Number(slider.stock) : slider.from)
        var v = slider.snap(base + (w.angleDelta.y > 0 ? slider.step : -slider.step))
        if (v !== base) slider.moved(v)
      }
    }
  }

  Text {
    id: readoutText
    width: slider.readoutWidth
    anchors.right: parent.right
    anchors.verticalCenter: parent.verticalCenter
    horizontalAlignment: Text.AlignRight
    elide: Text.ElideLeft
    textFormat: Text.PlainText
    text: slider.readout !== "" ? slider.readout
      : slider.hasValue ? Model.fmt(slider.shownValue, 0) + slider.unit
      : slider.emptyText
    color: slider.foreground
    font.family: slider.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  // Why the control is off, on hover.
  MouseArea {
    id: hover
    anchors.fill: parent
    enabled: slider.disabledReason !== ""
    hoverEnabled: true
    acceptedButtons: Qt.NoButton
  }

  ToolTip {
    visible: slider.disabledReason !== "" && hover.containsMouse
    delay: 400
    contentItem: Text {
      textFormat: Text.PlainText
      text: slider.disabledReason
      color: Commons.Color.tooltip.text
      font.family: slider.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
    background: Rectangle {
      color: Commons.Color.tooltip.background
      border.width: 1
      border.color: Commons.Color.tooltip.border
    }
  }
}
