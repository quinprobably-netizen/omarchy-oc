import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../Model.js" as Model
import "../lib/memory.js" as Mem
import "../components"

// Memory page: RAM, swap and VRAM usage with the usage history, the DIMM
// table (SMBIOS, root only) and SPD profiles when the daemon can read them,
// and the daemon's note that timings are the BIOS's. Nothing here is
// tunable; the page reads `info` once and the status stream after that.
Column {
  id: page

  required property var root
  // Every dereference guards on `st` itself, never on `alive` (see Panel.qml).
  readonly property var st: root.status
  readonly property bool alive: root.alive
  readonly property var strings: root.strings
  readonly property color fg: root.fg
  readonly property color dim: root.dim
  readonly property color accent: root.accent
  readonly property color urgent: root.urgent
  readonly property string fontFamily: root.fontFamily

  // info.json, fetched once per page load.
  property var info: null
  property bool infoFailed: false

  readonly property var usage: Mem.usageRows(st, info)
  readonly property var dimms: Mem.dimmRows(info)
  readonly property string dimmHint: Mem.dimmHint(info)
  readonly property var spd: Mem.spdRows(info)
  readonly property string note: Mem.memNote(info)
  readonly property string caption: Mem.memCaption(info)
  readonly property var memReading: st && st.mem && st.mem.reading ? st.mem.reading : null
  readonly property var chartSeries: [{
    values: Model.series(st, "mem_used_pct"),
    color: String(accent),
    label: Model.fmt(memReading ? memReading.used_pct : null, 0, " %"),
    min: 0,
    max: 100
  }]

  readonly property var columns: [
    { key: "slot", label: strings.t("memory.dimm.slot"), width: Style.space(70) },
    { key: "size", label: strings.t("memory.dimm.size"), width: Style.space(54) },
    { key: "type", label: strings.t("memory.dimm.type"), width: Style.space(44) },
    { key: "speed", label: strings.t("memory.dimm.speed"), width: Style.space(108) },
    { key: "part", label: strings.t("memory.dimm.part"), width: 0 }
  ]

  function fetchInfo() {
    root.client.request("info", null, function(ok, r) {
      if (!page) return
      if (ok && r && typeof r === "object") page.info = r
      else page.infoFailed = true
    })
  }

  Component.onCompleted: if (alive) fetchInfo()
  onAliveChanged: if (alive && info === null) fetchInfo()

  function cell(row, key) {
    if (key !== "part") return String(row[key] || "")
    if (!row.populated) return page.strings.t("memory.dimm.empty")
    return (String(row.vendor || "") + " " + String(row.part || "")).trim()
  }

  spacing: Style.space(8)

  component Section: Text {
    elide: Text.ElideRight
    textFormat: Text.PlainText
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
    font.bold: true
    font.letterSpacing: 1.2
    topPadding: Style.space(4)
  }

  component Body: Text {
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  // ---- header ----
  Item {
    width: parent.width
    implicitHeight: Math.max(headLabels.implicitHeight, headRight.implicitHeight)

    Column {
      id: headLabels
      anchors.left: parent.left
      anchors.right: headRight.left
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(2)

      Text {
        textFormat: Text.PlainText
        text: page.strings.t("page.memory")
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.title
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        width: parent.width
        elide: Text.ElideRight
        text: (page.st ? page.caption : page.strings.t("overview.offline")).toUpperCase()
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.caption
        font.bold: true
        font.letterSpacing: 1.2
      }
    }

    Text {
      id: headRight
      textFormat: Text.PlainText
      text: page.usage.length > 0 ? page.usage[0].text : "—"
      color: page.dim
      font.family: page.fontFamily
      font.pixelSize: Style.font.bodySmall
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
    }
  }

  // ---- offline ----
  Body {
    visible: !page.alive
    text: Model.offlineHint(page.root.socketPath)
  }

  // ---- usage ----
  Section {
    visible: page.alive
    width: parent.width
    text: page.strings.t("memory.usage").toUpperCase()
  }

  Repeater {
    model: page.alive ? page.usage : []

    Item {
      id: usageRow
      required property var modelData
      width: parent.width
      implicitHeight: Style.space(18)

      Text {
        id: usageLabel
        width: Style.space(52)
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        elide: Text.ElideRight
        textFormat: Text.PlainText
        text: page.strings.t("memory." + usageRow.modelData.key)
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }

      Rectangle {
        id: bar
        anchors.left: usageLabel.right
        anchors.leftMargin: Style.space(8)
        anchors.right: usageText.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        height: Style.space(6)
        radius: height / 2
        color: Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.1)

        Rectangle {
          readonly property real frac: usageRow.modelData.frac === null ? 0 : Math.max(0, Math.min(1, Number(usageRow.modelData.frac)))
          anchors.left: parent.left
          height: parent.height
          radius: parent.radius
          width: parent.width * frac
          color: frac > 0.9 ? page.urgent : page.accent
          Behavior on width { NumberAnimation { duration: Style.duration(200); easing.type: Easing.OutCubic } }
        }
      }

      Text {
        id: usageText
        anchors.right: usageValue.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(96)
        horizontalAlignment: Text.AlignRight
        textFormat: Text.PlainText
        text: String(usageRow.modelData.text)
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Text {
        id: usageValue
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(40)
        horizontalAlignment: Text.AlignRight
        textFormat: Text.PlainText
        text: String(usageRow.modelData.value)
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
    }
  }

  Chart {
    visible: page.alive
    width: parent.width
    height: Style.space(56)
    series: page.chartSeries
    foreground: page.fg
    fontFamily: page.fontFamily
  }

  // ---- DIMM table ----
  Section {
    visible: page.alive
    width: parent.width
    text: page.strings.t("memory.dimms").toUpperCase()
  }

  Body {
    visible: page.alive && page.info === null
    text: page.infoFailed ? page.strings.t(page.dimmHint) : page.strings.t("memory.loading")
  }

  Body {
    visible: page.alive && page.info !== null && page.dimms.length === 0
    text: page.strings.t(page.dimmHint)
  }

  Column {
    visible: page.alive && page.dimms.length > 0
    width: parent.width
    spacing: Style.space(2)

    // Header.
    Row {
      width: parent.width
      spacing: Style.space(6)
      Repeater {
        model: page.columns
        Text {
          required property var modelData
          width: modelData.width > 0 ? modelData.width : Math.max(0, parent.width - page.columns.reduce(function(a, c) { return a + c.width + Style.space(6) }, 0))
          elide: Text.ElideRight
          textFormat: Text.PlainText
          text: String(modelData.label)
          color: page.dim
          font.family: page.fontFamily
          font.pixelSize: Style.font.caption
        }
      }
    }

    Repeater {
      model: page.dimms
      Row {
        id: dimmRow
        required property var modelData
        width: parent.width
        spacing: Style.space(6)
        opacity: modelData.populated ? 1 : 0.5
        Repeater {
          model: page.columns
          Text {
            required property var modelData
            width: modelData.width > 0 ? modelData.width : Math.max(0, parent.width - page.columns.reduce(function(a, c) { return a + c.width + Style.space(6) }, 0))
            elide: Text.ElideRight
            textFormat: Text.PlainText
            text: page.cell(dimmRow.modelData, modelData.key)
            color: modelData.key === "slot" ? page.dim : page.fg
            font.family: page.fontFamily
            font.pixelSize: Style.font.bodySmall
          }
        }
      }
    }
  }

  // ---- SPD: EXPO / XMP ----
  Section {
    visible: page.alive && page.spd.length > 0
    width: parent.width
    text: page.strings.t("memory.spd").toUpperCase()
  }

  Repeater {
    model: page.alive ? page.spd : []
    Body {
      required property var modelData
      color: page.fg
      text: [
        modelData.slot !== "" ? page.strings.t("memory.spd.slot").arg(modelData.slot) : "",
        (String(modelData.vendor) + " " + String(modelData.part)).trim(),
        String(modelData.type),
        String(modelData.jedec),
        String(modelData.profiles)
      ].filter(function(s) { return s !== "" }).join(" · ")
    }
  }

  // ---- note ----
  Section {
    visible: page.alive && page.note !== ""
    width: parent.width
    text: page.strings.t("memory.note").toUpperCase()
  }

  Body {
    visible: page.alive && page.note !== ""
    text: page.note
  }

  Text {
    readonly property string msg: page.root.error !== "" ? page.root.error : (page.st ? String(page.st.message || "") : "")
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
