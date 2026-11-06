import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../Model.js" as Model

// An integer stepper: [label] [−] value [+] [clear]. Holding a step button
// repeats. The caller owns the value: bind `value` and act on
// `changed(v)` (clamped to from..to) and `cleared()` (the clear button,
// shown when `clearText` is set).
Row {
  id: stepper

  property string label: ""
  property int value: 0
  property int from: -50
  property int to: 30
  property string clearText: ""
  property string valueText: Model.fmtCo(value)
  property bool highlight: false
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property string fontFamily: Style.font.family

  signal changed(int v)
  signal cleared()

  readonly property color dim: Qt.darker(foreground, 1.4)

  spacing: Style.space(6)
  opacity: enabled ? 1 : 0.45

  function stepBy(d) {
    var v = Math.max(from, Math.min(to, value + d))
    if (v !== value) changed(v)
  }

  component StepButton: Rectangle {
    id: btn
    property string glyph: ""
    property int delta: 0
    width: Style.space(24)
    height: Style.space(22)
    radius: Style.cornerRadius
    color: area.pressed ? Qt.rgba(stepper.foreground.r, stepper.foreground.g, stepper.foreground.b, 0.18)
      : area.containsMouse ? Qt.rgba(stepper.foreground.r, stepper.foreground.g, stepper.foreground.b, 0.12)
      : Qt.rgba(stepper.foreground.r, stepper.foreground.g, stepper.foreground.b, 0.06)
    Behavior on color { ColorAnimation { duration: Style.duration(100) } }

    Text {
      anchors.centerIn: parent
      textFormat: Text.PlainText
      text: btn.glyph
      color: stepper.foreground
      font.family: stepper.fontFamily
      font.pixelSize: Style.font.body
      font.bold: true
    }

    MouseArea {
      id: area
      anchors.fill: parent
      enabled: stepper.enabled
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onPressed: { stepper.stepBy(btn.delta); repeatDelay.restart() }
      onReleased: { repeatDelay.stop(); repeater.stop() }
      onCanceled: { repeatDelay.stop(); repeater.stop() }
    }
    Timer { id: repeatDelay; interval: 350; onTriggered: repeater.start() }
    Timer { id: repeater; interval: 70; repeat: true; onTriggered: stepper.stepBy(btn.delta) }
  }

  Text {
    visible: stepper.label !== ""
    anchors.verticalCenter: parent.verticalCenter
    textFormat: Text.PlainText
    text: stepper.label
    color: stepper.dim
    font.family: stepper.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  StepButton { glyph: "−"; delta: -1; anchors.verticalCenter: parent.verticalCenter }

  Text {
    width: Style.space(40)
    anchors.verticalCenter: parent.verticalCenter
    horizontalAlignment: Text.AlignHCenter
    textFormat: Text.PlainText
    text: stepper.valueText
    color: stepper.highlight ? stepper.accent : stepper.foreground
    font.family: stepper.fontFamily
    font.pixelSize: Style.font.subtitle
    font.bold: true
  }

  StepButton { glyph: "+"; delta: 1; anchors.verticalCenter: parent.verticalCenter }

  Rectangle {
    visible: stepper.clearText !== ""
    anchors.verticalCenter: parent.verticalCenter
    width: clearLabel.implicitWidth + Style.space(14)
    height: Style.space(22)
    radius: Style.cornerRadius
    color: clearArea.containsMouse
      ? Qt.rgba(stepper.foreground.r, stepper.foreground.g, stepper.foreground.b, 0.12)
      : Qt.rgba(stepper.foreground.r, stepper.foreground.g, stepper.foreground.b, 0.06)
    Behavior on color { ColorAnimation { duration: Style.duration(100) } }

    Text {
      id: clearLabel
      anchors.centerIn: parent
      textFormat: Text.PlainText
      text: stepper.clearText
      color: stepper.dim
      font.family: stepper.fontFamily
      font.pixelSize: Style.font.caption
    }
    MouseArea {
      id: clearArea
      anchors.fill: parent
      enabled: stepper.enabled
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: stepper.cleared()
    }
  }
}
