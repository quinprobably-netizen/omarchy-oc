import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../Model.js" as Model

// One tile per physical core: "C2 / −20 / 4c ✗". `tiles` is
// Model.coreTiles() output ({core, tier, co, override, test}); a per-core
// override draws its value in the accent, the last test verdict is a strip
// along the bottom (green passed, urgent failed). A live test (the Test
// page) sets `test` "run" for a core under load (accent strip, "…") and
// `errors` for the count on a failed one ("3✗"). Tapping a tile emits
// `tapped(core)`; `selected` outlines that core.
Grid {
  id: grid

  property var tiles: []
  property int selected: -1
  property bool hasDense: true
  property string passText: ""
  property string failText: ""
  property color foreground: Commons.Color.foreground
  property color accent: Commons.Color.accent
  property color urgent: Commons.Color.urgent
  property color okColor: "#8fd694"
  property string fontFamily: Style.font.family

  signal tapped(int core)

  readonly property color dim: Qt.darker(foreground, 1.4)
  readonly property int perRow: Math.max(1, Math.min(8, tiles.length))

  columns: perRow
  columnSpacing: Style.space(5)
  rowSpacing: Style.space(5)
  // Disabled (no SMU): dimmed like the other controls, not just deaf.
  opacity: enabled ? 1 : 0.45
  readonly property real cell: (width - columnSpacing * (columns - 1)) / columns

  Repeater {
    model: grid.tiles

    Rectangle {
      id: tile
      required property var modelData
      readonly property bool on: modelData.core === grid.selected
      readonly property color verdict: modelData.test === "pass" ? grid.okColor : modelData.test === "run" ? grid.accent : grid.urgent

      width: grid.cell
      implicitHeight: col.implicitHeight + Style.space(12)
      radius: Style.cornerRadius
      color: tile.on ? Style.selectedFillFor(grid.foreground, grid.accent)
        : area.containsMouse ? Qt.rgba(grid.foreground.r, grid.foreground.g, grid.foreground.b, 0.12)
        : Qt.rgba(grid.foreground.r, grid.foreground.g, grid.foreground.b, 0.06)
      border.width: tile.on ? 1 : 0
      border.color: grid.accent
      clip: true

      Behavior on color { ColorAnimation { duration: Style.duration(120) } }

      Column {
        id: col
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        anchors.topMargin: Style.space(5)
        spacing: 0

        Text {
          width: parent.width
          horizontalAlignment: Text.AlignHCenter
          textFormat: Text.PlainText
          text: "C" + tile.modelData.core
          color: grid.dim
          font.family: grid.fontFamily
          font.pixelSize: Style.font.caption
        }
        Text {
          width: parent.width
          horizontalAlignment: Text.AlignHCenter
          textFormat: Text.PlainText
          text: Model.fmtCo(tile.modelData.co)
          color: tile.modelData.override ? grid.accent : grid.foreground
          font.family: grid.fontFamily
          font.pixelSize: Style.font.title
          font.bold: true
        }
        Text {
          width: parent.width
          horizontalAlignment: Text.AlignHCenter
          elide: Text.ElideRight
          textFormat: Text.PlainText
          text: {
            var t = Model.tierLabel(tile.modelData.tier, grid.hasDense)
            var n = tile.modelData.errors > 0 ? String(tile.modelData.errors) : ""
            var v = tile.modelData.test === "pass" ? "✓" : tile.modelData.test === "fail" ? n + "✗" : tile.modelData.test === "run" ? "…" : ""
            return [t, v].filter(function(s) { return s !== "" }).join(" ")
          }
          color: tile.modelData.test === "fail" ? grid.urgent : grid.dim
          font.family: grid.fontFamily
          font.pixelSize: Style.font.caption
        }
      }

      Rectangle {
        visible: tile.modelData.test !== null
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.bottom: parent.bottom
        height: Math.max(2, Style.space(2))
        color: tile.verdict
      }

      MouseArea {
        id: area
        anchors.fill: parent
        enabled: grid.enabled
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: grid.tapped(tile.modelData.core)
      }
    }
  }
}
