import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui

// The vertical page rail: one icon button per page.
Column {
  id: rail

  property var pages: []
  property string current: ""
  property int cursor: -1
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property string fontFamily: Style.font.family

  signal selected(string name)

  spacing: Style.space(6)

  Repeater {
    model: rail.pages
    Button {
      required property var modelData
      required property int index
      width: Style.space(36)
      implicitHeight: Style.space(34)
      horizontalPadding: 0
      verticalPadding: 0
      iconText: modelData.icon
      iconSize: Style.font.iconLarge
      tooltipText: modelData.label
      foreground: rail.foreground
      accent: rail.accent
      fontFamily: rail.fontFamily
      active: rail.current === modelData.key
      hasCursor: rail.cursor === index
      onClicked: rail.selected(modelData.key)
    }
  }
}
