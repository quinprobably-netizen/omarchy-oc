import QtQuick
import qs.Commons
import "../Model.js" as Model

// Stands in for a page that a later task builds.
Column {
  id: page

  required property var root
  spacing: Style.space(6)

  Text {
    textFormat: Text.PlainText
    text: Model.PAGES[Model.pageIndex(page.root.page)].label
    color: page.root.fg
    font.family: page.root.fontFamily
    font.pixelSize: Style.font.title
    font.bold: true
  }
  Text {
    textFormat: Text.PlainText
    text: page.root.strings.t("page.coming")
    color: page.root.dim
    font.family: page.root.fontFamily
    font.pixelSize: Style.font.bodySmall
  }
}
