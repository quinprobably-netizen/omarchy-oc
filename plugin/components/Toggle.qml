import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// A compact labelled switch for a settings row ("Boost [on]"). The caller
// owns the value: bind `checked` and act on `toggled(checked)`, which
// carries the value the user asked for. Clicking the label counts too.
Item {
  id: toggle

  property string label: ""
  property bool checked: false
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property string fontFamily: Style.font.family
  property real fontSize: Style.font.caption

  signal toggled(bool checked)

  readonly property color dim: Qt.darker(foreground, 1.4)

  implicitWidth: row.implicitWidth
  implicitHeight: row.implicitHeight
  opacity: enabled ? 1 : 0.45

  Row {
    id: row
    spacing: Style.space(6)

    Text {
      anchors.verticalCenter: parent.verticalCenter
      textFormat: Text.PlainText
      text: toggle.label
      color: mouse.containsMouse ? toggle.foreground : toggle.dim
      font.family: toggle.fontFamily
      font.pixelSize: toggle.fontSize
      visible: toggle.label !== ""
    }

    ToggleSwitch {
      anchors.verticalCenter: parent.verticalCenter
      checked: toggle.checked
      interactive: false
      cursorRing: false
      trackHeight: Style.space(16)
      foreground: toggle.foreground
      accent: toggle.accent
    }
  }

  MouseArea {
    id: mouse
    anchors.fill: parent
    enabled: toggle.enabled
    hoverEnabled: true
    cursorShape: Qt.PointingHandCursor
    onClicked: toggle.toggled(!toggle.checked)
  }
}
