import QtQuick
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../Model.js" as Model
import "../components"

// Overview page: profile chips, the automation line, live tiles with
// sparklines, and the pending/alert banners.
Column {
  id: page

  required property var root
  readonly property var st: root.status
  readonly property bool alive: root.alive
  readonly property var strings: root.strings
  readonly property color fg: root.fg
  readonly property color dim: root.dim
  readonly property color accent: root.accent
  readonly property color urgent: root.urgent
  readonly property string fontFamily: root.fontFamily

  readonly property var cpu: Model.cpuTile(st)
  readonly property var gpu: Model.gpuTile(st)
  readonly property var mem: Model.memTile(st)
  readonly property var fans: Model.fansTile(st)

  spacing: Style.space(10)

  // ---- header ----
  Item {
    width: parent.width
    implicitHeight: Math.max(headLabels.implicitHeight, headTemp.implicitHeight)

    Column {
      id: headLabels
      anchors.left: parent.left
      anchors.right: headTemp.left
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(2)

      Text {
        textFormat: Text.PlainText
        text: page.strings.t("page.overview")
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.title
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        width: parent.width
        elide: Text.ElideRight
        text: (page.alive
          ? Model.profileLabel(page.st.active) + (page.st.auto_switch && page.st.auto_profile ? " · " + page.strings.t("overview.auto") : "")
          : page.strings.t("overview.offline")).toUpperCase()
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.caption
        font.bold: true
        font.letterSpacing: 1.2
      }
    }

    Text {
      id: headTemp
      textFormat: Text.PlainText
      text: page.alive ? Model.fmt(Model.cpuTemp(page.st), 0, "°") : "—"
      color: page.fg
      font.family: page.fontFamily
      font.pixelSize: Style.font.displayLarge
      font.bold: true
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
    }
  }

  // ---- offline ----
  Text {
    visible: !page.alive
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: Model.offlineHint(page.root.socketPath)
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  // ---- pending: Keep / Revert ----
  Rectangle {
    id: pendingBanner
    visible: page.root.pending !== null
    width: parent.width
    implicitHeight: pendingRow.implicitHeight + Style.space(16)
    radius: Style.cornerRadius
    color: Qt.rgba(page.accent.r, page.accent.g, page.accent.b, 0.14)
    border.width: 1
    border.color: Qt.rgba(page.accent.r, page.accent.g, page.accent.b, 0.5)

    Row {
      id: pendingRow
      anchors.verticalCenter: parent.verticalCenter
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      spacing: Style.space(8)

      Text {
        width: parent.width - keepButton.width - revertButton.width - parent.spacing * 2
        anchors.verticalCenter: parent.verticalCenter
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
        text: page.root.pending
          ? page.strings.t("pending.text").arg(Model.profileLabel(page.root.pending.profile)).arg(Model.pendingRemaining(page.st))
          : ""
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Button {
        id: keepButton
        anchors.verticalCenter: parent.verticalCenter
        text: page.strings.t("pending.keep")
        fontSize: Style.font.caption
        foreground: page.fg
        fontFamily: page.fontFamily
        bordered: true
        selected: true
        onClicked: page.root.keep()
      }
      Button {
        id: revertButton
        anchors.verticalCenter: parent.verticalCenter
        text: page.strings.t("pending.revert")
        fontSize: Style.font.caption
        foreground: page.fg
        fontFamily: page.fontFamily
        bordered: true
        onClicked: page.root.revert()
      }
    }
  }

  // ---- alert ----
  Rectangle {
    visible: page.root.alert !== ""
    width: parent.width
    implicitHeight: alertRow.implicitHeight + Style.space(16)
    radius: Style.cornerRadius
    color: Qt.rgba(page.urgent.r, page.urgent.g, page.urgent.b, 0.14)
    border.width: 1
    border.color: Qt.rgba(page.urgent.r, page.urgent.g, page.urgent.b, 0.5)

    Row {
      id: alertRow
      anchors.verticalCenter: parent.verticalCenter
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      spacing: Style.space(8)

      Text {
        width: parent.width - clearButton.width - parent.spacing
        anchors.verticalCenter: parent.verticalCenter
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
        text: page.root.alert
        color: page.urgent
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Button {
        id: clearButton
        anchors.verticalCenter: parent.verticalCenter
        text: page.strings.t("alert.clear")
        fontSize: Style.font.caption
        foreground: page.fg
        fontFamily: page.fontFamily
        bordered: true
        onClicked: page.root.clearAlert()
      }
    }
  }

  // ---- automation line ----
  Rectangle {
    visible: page.alive
    width: parent.width
    implicitHeight: autoText.implicitHeight + Style.space(14)
    radius: Style.cornerRadius
    color: Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.06)

    Rectangle {
      id: autoDot
      width: Style.space(8)
      height: width
      radius: width / 2
      anchors.left: parent.left
      anchors.leftMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      color: page.alive && page.st.auto_switch ? page.accent : page.dim
    }
    Text {
      id: autoText
      anchors.left: autoDot.right
      anchors.leftMargin: Style.space(8)
      anchors.right: parent.right
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      elide: Text.ElideRight
      textFormat: Text.PlainText
      text: Model.automationLine(page.st)
      color: page.fg
      font.family: page.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
  }

  // ---- tiles ----
  Grid {
    id: grid
    visible: page.alive
    width: parent.width
    columns: 2
    columnSpacing: Style.space(8)
    rowSpacing: Style.space(8)
    readonly property real cell: (width - columnSpacing) / 2

    Tile {
      width: grid.cell
      title: page.cpu.title; value: page.cpu.value; sub: page.cpu.sub
      values: Model.series(page.st, page.cpu.seriesName)
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    }
    Tile {
      width: grid.cell
      title: page.gpu.title; value: page.gpu.value; sub: page.gpu.sub
      values: Model.series(page.st, page.gpu.seriesName)
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    }
    Tile {
      width: grid.cell
      title: page.mem.title; value: page.mem.value; sub: page.mem.sub
      values: Model.series(page.st, page.mem.seriesName)
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    }
    Tile {
      width: grid.cell
      title: page.fans.title; value: page.fans.value; sub: page.fans.sub
      values: Model.series(page.st, page.fans.seriesName)
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    }
  }

  // ---- profiles ----
  Flow {
    visible: page.alive
    width: parent.width
    spacing: Style.space(6)

    Repeater {
      model: Model.profileChips(page.st)
      Chip {
        required property var modelData
        icon: modelData.icon
        text: modelData.label
        on: modelData.on
        warn: modelData.warn
        foreground: page.fg
        accent: page.accent
        urgent: page.urgent
        fontFamily: page.fontFamily
        tooltipText: modelData.warn ? page.strings.t("profiles.untested") : ""
        onClicked: page.root.applyProfile(modelData.name)
      }
    }
  }

  Text {
    readonly property string msg: page.root.error !== "" ? page.root.error : (page.alive ? String(page.st.message || "") : "")
    visible: msg !== ""
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: msg
    color: page.root.error !== "" ? page.urgent : page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }
}
