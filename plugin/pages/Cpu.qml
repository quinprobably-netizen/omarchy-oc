import QtQuick
import qs.Commons
import qs.Commons as Commons
import "../Model.js" as Model
import "../components"

// CPU page: amd-pstate mode, EPP, boost, SMT and C-state controls (live),
// SMU power limits with the stock tick and live draw (staged), and the
// Curve Optimizer core grid (staged, except back to 0 which is stock).
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

  readonly property var knobs: st && st.cpu && st.cpu.knobs ? st.cpu.knobs : null
  readonly property var profile: Model.profileEntry(st, root.active)
  readonly property var tune: profile && profile.tune ? profile.tune : null
  readonly property var smuTune: tune && tune.smu ? tune.smu : null
  readonly property var stagedChanges: root.staged ? root.staged.changes : null
  readonly property var smu: st ? st.smu : null
  readonly property string smuReason: Model.smuDisabledReason(st)
  readonly property bool smuOk: smuReason === ""
  readonly property var stockNotice: Model.smuStockNotice(st)
  readonly property var head: Model.cpuHeadline(st)
  readonly property var tiles: Model.coreTiles(st, tune, profile ? profile.cpu_test : null, stagedChanges)
  readonly property bool hasDense: !!(knobs && knobs.has_dense)
  readonly property var tdcRow: Model.limitRow(smu, "tdc")
  readonly property var edcRow: Model.limitRow(smu, "edc")

  property int selectedCore: -1
  readonly property var selectedTile: {
    for (var i = 0; i < tiles.length; i++) if (tiles[i].core === selectedCore) return tiles[i]
    return null
  }

  // Every control goes through root.edit: safe values live, risky staged.
  function edit(key, value) { root.edit(key, value) }

  function tuneLimit(key) {
    return smuTune ? smuTune[key.substring(4)] : null
  }

  readonly property var limitRows: [
    { stem: "fast", key: "smu.fast_w", label: strings.t("cpu.ppt_fast"), unit: " W" },
    { stem: "slow", key: "smu.slow_w", label: strings.t("cpu.ppt_slow"), unit: " W" },
    { stem: "tctl", key: "smu.tctl_c", label: strings.t("cpu.tctl"), unit: " °C" },
    { stem: "tdc", key: "smu.tdc_a", label: strings.t("cpu.tdc_edc"), unit: " A", pair: true }
  ]

  readonly property var cstateOptions: [
    { value: "all", label: strings.t("cpu.cstates.all") },
    { value: "C2", label: "≤C2" },
    { value: "C1", label: "≤C1" }
  ]

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
        text: page.strings.t("page.cpu")
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

  // ---- live knobs ----
  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    RowLabel { text: page.strings.t("cpu.mode") }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: page.knobs && page.knobs.pstate_modes ? page.knobs.pstate_modes : []
      value: page.knobs ? String(page.knobs.pstate_mode || "") : ""
      enabled: page.knobs !== null
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.edit("cpu.pstate_mode", v) }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    RowLabel { text: page.strings.t("cpu.epp") }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: Model.EPP_OPTIONS
      value: page.knobs ? String(page.knobs.epp || "") : ""
      enabled: page.knobs !== null
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.edit("cpu.epp", v) }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(14)
    Toggle {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("cpu.boost")
      checked: !!(page.knobs && page.knobs.boost)
      enabled: page.knobs !== null
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.edit("cpu.boost", on) }
    }
    Toggle {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("cpu.smt")
      checked: !!(page.knobs && (page.knobs.smt_active === true || page.knobs.smt === "on"))
      enabled: page.knobs !== null
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.edit("cpu.smt", on) }
    }
    Row {
      spacing: Style.space(6)
      anchors.verticalCenter: parent.verticalCenter
      Text {
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        text: page.strings.t("cpu.cstates")
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.caption
      }
      Segmented {
        anchors.verticalCenter: parent.verticalCenter
        options: page.cstateOptions
        value: Model.idleMax(page.knobs) || "all"
        enabled: page.knobs !== null
        foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
        onChanged: function(v) { page.edit("cpu.idle_max", v === "all" ? null : v) }
      }
    }
  }

  // ---- SMU limits ----
  Section {
    visible: page.alive
    width: parent.width
    text: (page.strings.t("cpu.limits") + " · " + page.strings.t(page.smuOk ? "cpu.limits.use" : "cpu.limits.unavailable")).toUpperCase()
  }

  Text {
    visible: page.alive && !page.smuOk
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.smuReason
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }

  // The SMU answers but the PM table does not read: no stock values, so a
  // changed limit stays until a reboot.
  Text {
    visible: page.alive && page.smuOk && page.stockNotice.noStock
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.strings.t("cpu.limits.nostock")
    color: page.urgent
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }

  Text {
    visible: page.alive && page.stockNotice.reboot !== ""
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.strings.t("cpu.limits.reboot").arg(page.stockNotice.reboot)
    color: page.urgent
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }

  Repeater {
    model: page.alive ? page.limitRows : []

    LimitSlider {
      id: limit
      required property var modelData
      readonly property var row: Model.limitRow(page.smu, modelData.stem)
      readonly property var edc: modelData.pair
        ? Model.shownLimit(page.stagedChanges, "smu.edc_a", page.tuneLimit("smu.edc_a"), page.edcRow.value)
        : null

      width: parent.width
      label: modelData.label
      value: Model.shownLimit(page.stagedChanges, modelData.key, page.tuneLimit(modelData.key), row.value)
      stock: row.stock
      use: row.use
      from: row.min
      to: row.max
      unit: modelData.unit
      emptyText: page.smuOk ? page.strings.t("cpu.stock") : "—"
      readout: modelData.pair && value !== null
        ? Model.fmt(value, 0) + " / " + Model.fmt(edc, 0) + modelData.unit
        : ""
      disabledReason: page.smuReason
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onMoved: function(v) {
        var n = Math.round(v)
        page.edit(modelData.key, n)
        if (modelData.pair) page.edit("smu.edc_a", Model.edcFor(n, page.tdcRow.stock, page.edcRow.stock))
      }
    }
  }

  // ---- Curve Optimizer ----
  Section {
    visible: page.alive
    width: parent.width
    text: (page.strings.t("cpu.co") + " · " + page.strings.t("cpu.co.hint")).toUpperCase()
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)

    Stepper {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("cpu.co.all")
      value: Model.coAll(page.tune, page.stagedChanges)
      from: -50
      to: 30
      clearText: page.strings.t("cpu.stock")
      enabled: page.smuOk
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.edit("smu.co_all", v) }
      onCleared: page.edit("smu.co_all", null)
    }
  }

  CoreGrid {
    visible: page.alive && page.tiles.length > 0
    width: parent.width
    tiles: page.tiles
    selected: page.selectedCore
    hasDense: page.hasDense
    enabled: page.smuOk
    foreground: page.fg; accent: page.accent; urgent: page.urgent; fontFamily: page.fontFamily
    onTapped: function(core) { page.selectedCore = page.selectedCore === core ? -1 : core }
  }

  Row {
    visible: page.alive && page.selectedTile !== null
    width: parent.width
    spacing: Style.space(8)
    Stepper {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("cpu.co.core").arg(page.selectedCore)
      value: page.selectedTile ? page.selectedTile.co : 0
      from: -50
      to: 30
      clearText: page.strings.t("cpu.stock")
      highlight: !!(page.selectedTile && page.selectedTile.override)
      enabled: page.smuOk
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.edit("smu.co_cores." + page.selectedCore, v) }
      onCleared: page.edit("smu.co_cores." + page.selectedCore, null)
    }
    Text {
      anchors.verticalCenter: parent.verticalCenter
      textFormat: Text.PlainText
      text: page.selectedTile && page.selectedTile.test !== null
        ? page.strings.t(page.selectedTile.test === "pass" ? "core.pass" : "core.fail")
        : ""
      color: page.selectedTile && page.selectedTile.test === "fail" ? page.urgent : page.dim
      font.family: page.fontFamily
      font.pixelSize: Style.font.caption
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
