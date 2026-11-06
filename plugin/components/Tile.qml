import QtQuick
import qs.Commons
import qs.Commons as Commons

// One Overview tile: caption, big value, detail line, sparkline.
Rectangle {
  id: tile

  property string title: ""
  property string value: ""
  property string sub: ""
  property var values: []
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property string fontFamily: Style.font.family
  readonly property color dim: Qt.darker(foreground, 1.4)

  radius: Style.cornerRadius
  color: Qt.rgba(foreground.r, foreground.g, foreground.b, 0.06)
  implicitHeight: column.implicitHeight + Style.space(16)

  Column {
    id: column
    anchors.left: parent.left
    anchors.right: parent.right
    anchors.top: parent.top
    anchors.margins: Style.space(8)
    spacing: Style.space(2)

    Text {
      width: parent.width
      elide: Text.ElideRight
      textFormat: Text.PlainText
      text: tile.title
      color: tile.dim
      font.family: tile.fontFamily
      font.pixelSize: Style.font.caption
    }
    Text {
      width: parent.width
      elide: Text.ElideRight
      textFormat: Text.PlainText
      text: tile.value
      color: tile.foreground
      font.family: tile.fontFamily
      font.pixelSize: Style.font.heading
      font.bold: true
    }
    Text {
      width: parent.width
      elide: Text.ElideRight
      textFormat: Text.PlainText
      text: tile.sub
      color: tile.dim
      font.family: tile.fontFamily
      font.pixelSize: Style.font.caption
    }
    Sparkline {
      width: parent.width
      height: Style.space(30)
      values: tile.values
      color: tile.accent
    }
  }
}
