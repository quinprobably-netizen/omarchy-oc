import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../Model.js" as Model
import "../lib/gpu.js" as Gpu
import "../components"

// GPU page: the live chart (clock, temperature, power), the PL1 slider
// (live at or below stock, staged above), min/max clock sliders and the
// power profile (live), and the programs using the GPU.
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

  // info.json, fetched once per page load: vendor, PCIe link and ReBAR for
  // the caption. The page works without it.
  property var info: null

  readonly property var knobs: st && st.gpu && st.gpu.knobs ? st.gpu.knobs : null
  readonly property bool hasGpu: !!(st && st.caps && st.caps.gpu && knobs)
  readonly property var profile: Model.profileEntry(st, root.active)
  readonly property var gpuTune: profile && profile.tune && profile.tune.gpu ? profile.tune.gpu : null
  readonly property var stagedChanges: root.staged ? root.staged.changes : null
  readonly property var head: Gpu.gpuHeadline(st, info)
  readonly property var powerRow: Gpu.powerRow(st)
  readonly property var maxRow: Gpu.clockRow(st, "max")
  readonly property var minRow: Gpu.clockRow(st, "min")
  // The clocks as the daemon will validate them: the unflushed live batch,
  // then the profile's tune, then the hardware knob. The sliders show
  // these and clamp against them, so the pair sent is never min > max.
  readonly property var liveChanges: root.liveChanges
  readonly property var inflight: root.inflightChanges
  readonly property var shownMax: Gpu.shownClock("max", liveChanges, gpuTune, knobs, inflight)
  readonly property var shownMin: Gpu.shownClock("min", liveChanges, gpuTune, knobs, inflight)
  readonly property var clients: Gpu.gpuClients(st)
  readonly property string throttle: Gpu.throttleText(st)
  readonly property var chartSeries: Gpu.chartSeries(st, { clock: String(accent), temp: "#e8c26a", power: "#ff6b8b" }, strings.t("gpu.idle"))

  // Every control goes through root.edit: safe values live, risky staged.
  function edit(key, value) { root.edit(key, value) }

  function fetchInfo() {
    root.client.request("info", null, function(ok, r) {
      if (page && ok && r && typeof r === "object") page.info = r
    })
  }

  Component.onCompleted: if (alive) fetchInfo()
  onAliveChanged: if (alive && info === null) fetchInfo()

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

  component RowLabel: Text {
    width: Style.space(52)
    anchors.verticalCenter: parent.verticalCenter
    elide: Text.ElideRight
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
        text: page.strings.t("page.gpu")
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.title
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        width: parent.width
        elide: Text.ElideRight
        text: (page.st ? page.head.caption : page.strings.t("overview.offline")).toUpperCase()
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
      text: page.head.right
      color: page.dim
      font.family: page.fontFamily
      font.pixelSize: Style.font.bodySmall
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
    }
  }

  // ---- offline / no GPU ----
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

  Text {
    visible: page.alive && !page.hasGpu
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.strings.t("gpu.unavailable")
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  // ---- chart ----
  Chart {
    visible: page.alive && page.hasGpu
    width: parent.width
    height: Style.space(96)
    series: page.chartSeries
    foreground: page.fg
    fontFamily: page.fontFamily
  }

  // ---- limits ----
  Section {
    visible: page.alive && page.hasGpu
    width: parent.width
    text: (page.strings.t("gpu.limits") + " · " + page.strings.t("gpu.limits.hint")).toUpperCase()
  }

  LimitSlider {
    visible: page.alive && page.hasGpu
    width: parent.width
    label: page.strings.t("gpu.power")
    value: Model.shownLimit(page.stagedChanges, "gpu.power_w", page.gpuTune ? page.gpuTune.power_w : null, page.powerRow.value)
    stock: page.powerRow.stock
    use: page.powerRow.use
    from: page.powerRow.min
    to: page.powerRow.max
    unit: " W"
    emptyText: page.strings.t("gpu.stock")
    foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    onMoved: function(v) { page.edit("gpu.power_w", Math.round(v)) }
  }

  LimitSlider {
    visible: page.alive && page.hasGpu
    width: parent.width
    label: page.strings.t("gpu.max_clock")
    value: page.shownMax
    stock: page.maxRow.stock
    from: page.maxRow.min
    to: page.maxRow.max
    step: 50
    unit: " MHz"
    foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    onMoved: function(v) { page.edit("gpu.max_mhz", Gpu.clampClockEdit("max", v, page.root.liveChanges, page.gpuTune, page.knobs, page.root.inflightChanges)) }
  }

  LimitSlider {
    visible: page.alive && page.hasGpu
    width: parent.width
    label: page.strings.t("gpu.min_clock")
    value: page.shownMin
    stock: page.minRow.stock
    from: page.minRow.min
    to: page.minRow.max
    step: 50
    unit: " MHz"
    foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    onMoved: function(v) { page.edit("gpu.min_mhz", Gpu.clampClockEdit("min", v, page.root.liveChanges, page.gpuTune, page.knobs, page.root.inflightChanges)) }
  }

  // ---- power profile + throttle state ----
  Item {
    visible: page.alive && page.hasGpu
    width: parent.width
    implicitHeight: profileRow.implicitHeight

    Row {
      id: profileRow
      anchors.left: parent.left
      spacing: Style.space(8)
      RowLabel { text: page.strings.t("gpu.profile") }
      Segmented {
        anchors.verticalCenter: parent.verticalCenter
        options: Gpu.profileOptions(page.knobs)
        value: page.knobs ? String(page.knobs.power_profile || "") : ""
        enabled: page.knobs !== null
        foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
        onChanged: function(v) { page.edit("gpu.power_profile", v) }
      }
    }

    Text {
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      width: Math.max(0, parent.width - profileRow.width - Style.space(10))
      horizontalAlignment: Text.AlignRight
      elide: Text.ElideLeft
      textFormat: Text.PlainText
      text: page.throttle === "" ? page.strings.t("gpu.throttle.none") : page.strings.t("gpu.throttle").arg(page.throttle)
      color: page.throttle === "" ? page.dim : page.urgent
      font.family: page.fontFamily
      font.pixelSize: Style.font.caption
    }
  }

  // ---- clients ----
  Section {
    visible: page.alive && page.hasGpu
    width: parent.width
    text: (page.strings.t("gpu.clients") + " · " + page.strings.t("gpu.clients.hint")).toUpperCase()
  }

  Text {
    visible: page.alive && page.hasGpu && page.clients.length === 0
    width: parent.width
    textFormat: Text.PlainText
    text: page.strings.t("gpu.clients.none")
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  Repeater {
    model: page.alive && page.hasGpu ? page.clients : []

    Item {
      id: client
      required property var modelData
      width: parent.width
      implicitHeight: clientName.implicitHeight + Style.space(4)

      // Engine time as a faint strip under the row.
      Rectangle {
        anchors.left: parent.left
        anchors.bottom: parent.bottom
        height: Math.max(1, Style.space(2))
        width: parent.width * Math.max(0, Math.min(1, Number(client.modelData.busy) / 100))
        radius: height / 2
        color: page.accent
        opacity: 0.7
        Behavior on width { NumberAnimation { duration: Style.duration(200); easing.type: Easing.OutCubic } }
      }

      Text {
        id: clientName
        anchors.left: parent.left
        anchors.right: clientVram.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        elide: Text.ElideRight
        textFormat: Text.PlainText
        text: String(client.modelData.name)
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Text {
        id: clientVram
        anchors.right: clientBusy.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(64)
        horizontalAlignment: Text.AlignRight
        textFormat: Text.PlainText
        text: Gpu.fmtVram(client.modelData.vram)
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Text {
        id: clientBusy
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        width: Style.space(44)
        horizontalAlignment: Text.AlignRight
        textFormat: Text.PlainText
        text: Model.fmt(client.modelData.busy, 0, " %")
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
    }
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
