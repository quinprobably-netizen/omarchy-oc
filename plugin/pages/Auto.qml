import QtQuick
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../Model.js" as Model
import "../lib/auto.js" as Auto
import "../components"

// Auto page: the auto-switch toggle, the rules list (first match wins) with
// a per-rule profile pick and an "add from running programs" picker, the
// "Otherwise" base profile, apply-at-boot, the watchdog (limits, errors,
// fallback) and the Keep/Revert window. Everything here is a plain daemon
// request; nothing is staged.
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

  readonly property bool autoOn: !!(st && st.auto_switch)
  readonly property var wd: st && st.watchdog ? st.watchdog : null
  readonly property bool wdOn: !!(wd && wd.enabled)
  // Repeater models. Every status tick is a new object, so these are
  // replaced only when their content changes (Auto.listChanged); otherwise
  // the Repeaters would rebuild their delegates every second.
  property var rules: []
  property var profiles: []
  property var matchers: []
  // The rule whose profile picker is open (−1: none). Page state, not
  // delegate state, so it survives a rebuilt delegate.
  property int pickingIndex: -1
  readonly property string caption: {
    if (!st) return strings.t("overview.offline")
    if (st.auto_switch && st.auto_profile) return strings.t("auto.caption.on").arg(Model.plain(Model.profileLabel(st.auto_profile)))
    return strings.t(st.auto_switch ? "auto.caption.idle" : "auto.caption.off").arg(Model.plain(Model.profileLabel(st.base)))
  }

  // ---- add-rule picker ----
  property bool adding: false
  property string addProfile: ""
  property var procs: null
  property bool procsLoading: false
  property bool procsFailed: false
  onProcsChanged: refreshMatchers()

  // ---- watchdog steppers: edits overlay the status until it catches up ----
  property var wdEdits: ({})

  onStChanged: {
    wdEdits = Auto.watchdogSettled(wdEdits, wd)
    var nextRules = Auto.ruleRows(st)
    if (Auto.listChanged(rules, nextRules)) {
      rules = nextRules
      pickingIndex = -1
    }
    var nextProfiles = Auto.profileOptions(st, Model.profileLabel)
    if (Auto.listChanged(profiles, nextProfiles)) profiles = nextProfiles
    refreshMatchers()
  }
  Component.onCompleted: stChanged()

  function refreshMatchers() {
    var next = Auto.matcherOptions(procs, st ? st.rules : null)
    if (Auto.listChanged(matchers, next)) matchers = next
  }

  function togglePick(index) {
    pickingIndex = pickingIndex === index ? -1 : index
  }

  // Scripting hook (and how the live check drives the pickers):
  //   omarchy-shell qqq.oc.auto pick 0      open rule 0's profile picker
  //   omarchy-shell qqq.oc.auto add         open the add-rule picker
  IpcHandler {
    target: "qqq.oc.auto"
    function pick(index: int): void { page.pickingIndex = index }
    function add(): void { page.openAdd() }
  }

  function request(cmd, args, cb) {
    root.client.request(cmd, args, function(ok, r) {
      root.requestDone(ok, r)
      if (cb) cb(ok, r)
    })
  }

  function openAdd() {
    adding = true
    addProfile = root.active !== "" ? root.active : "performance"
    procs = null
    procsFailed = false
    procsLoading = true
    root.client.request("processes", null, function(ok, r) {
      page.procsLoading = false
      page.procsFailed = !ok
      page.procs = ok ? r : null
    })
  }

  function addRule(matcher) {
    request("rule-add", { matcher: String(matcher), profile: addProfile }, function(ok) { if (ok) page.adding = false })
  }

  function stepWatchdog(key, v) {
    var e = wdEdits
    e[key] = Auto.watchdogClamp(key, v)
    wdEdits = e
    wdTimer.restart()
  }

  Timer {
    id: wdTimer
    interval: 250
    onTriggered: {
      var args = {}
      var any = false
      for (var k in page.wdEdits) { args[k] = page.wdEdits[k]; any = true }
      if (!any) return
      page.request("watchdog", args, function(ok) { if (!ok) page.wdEdits = {} })
    }
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

  component RowLabel: Text {
    anchors.verticalCenter: parent.verticalCenter
    elide: Text.ElideRight
    textFormat: Text.PlainText
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  component Hint: Text {
    anchors.verticalCenter: parent.verticalCenter
    elide: Text.ElideRight
    textFormat: Text.PlainText
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }

  // A pill in the add-rule picker.
  component Pick: Rectangle {
    id: pick
    property string label: ""
    signal clicked()
    implicitWidth: pickText.implicitWidth + Style.space(18)
    implicitHeight: pickText.implicitHeight + Style.space(8)
    radius: height / 2
    color: pickArea.containsMouse ? Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.14) : Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.06)
    Behavior on color { ColorAnimation { duration: Style.duration(120) } }
    Text {
      id: pickText
      anchors.centerIn: parent
      textFormat: Text.PlainText
      text: pick.label
      color: page.fg
      font.family: page.fontFamily
      font.pixelSize: Style.font.caption
    }
    MouseArea {
      id: pickArea
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: pick.clicked()
    }
  }

  // ---- header ----
  Item {
    width: parent.width
    implicitHeight: Math.max(headLabels.implicitHeight, autoToggle.implicitHeight)

    Column {
      id: headLabels
      anchors.left: parent.left
      anchors.right: autoToggle.left
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(2)

      Text {
        textFormat: Text.PlainText
        text: page.strings.t("page.auto")
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.title
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        width: parent.width
        elide: Text.ElideRight
        text: page.caption.toUpperCase()
        color: page.autoOn && page.st && page.st.auto_profile ? page.accent : page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.caption
        font.bold: true
        font.letterSpacing: 1.2
      }
    }

    Toggle {
      id: autoToggle
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.toggle")
      checked: page.autoOn
      enabled: page.alive
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.request("auto", { enabled: on }) }
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

  // ---- rules ----
  Section {
    visible: page.alive
    width: parent.width
    text: (page.strings.t("auto.rules") + " · " + page.strings.t("auto.rules.hint")).toUpperCase()
  }

  Text {
    visible: page.alive && page.rules.length === 0
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.strings.t("auto.rules.none")
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }

  Repeater {
    model: page.alive ? page.rules : []

    RuleRow {
      required property var modelData
      width: parent.width
      label: modelData.label
      profile: modelData.profile
      profileLabel: Model.profileLabel(modelData.profile)
      profiles: page.profiles
      removeTooltip: page.strings.t("auto.rule.remove")
      foreground: page.fg; accent: page.accent; urgent: page.urgent; fontFamily: page.fontFamily
      picking: page.pickingIndex === modelData.index
      onToggled: page.togglePick(modelData.index)
      onPicked: function(p) { page.pickingIndex = -1; page.request("rule-set", { index: modelData.index, profile: p }) }
      onRemoved: page.request("rule-del", { index: modelData.index })
    }
  }

  // "+ Add rule" (dashed), or the picker in its place.
  Rectangle {
    visible: page.alive && !page.adding
    width: parent.width
    implicitHeight: addText.implicitHeight + Style.space(12)
    radius: Style.cornerRadius
    color: addArea.containsMouse ? Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.06) : "transparent"
    border.width: 1
    border.color: Qt.rgba(page.fg.r, page.fg.g, page.fg.b, addArea.containsMouse ? 0.35 : 0.2)
    Behavior on color { ColorAnimation { duration: Style.duration(120) } }

    Text {
      id: addText
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      elide: Text.ElideRight
      textFormat: Text.PlainText
      text: page.strings.t("auto.add")
      color: addArea.containsMouse ? page.fg : page.dim
      font.family: page.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
    MouseArea {
      id: addArea
      anchors.fill: parent
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: page.openAdd()
    }
  }

  Rectangle {
    visible: page.alive && page.adding
    width: parent.width
    implicitHeight: addBody.implicitHeight + Style.space(16)
    radius: Style.cornerRadius
    color: Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.06)
    border.width: 1
    border.color: Qt.rgba(page.accent.r, page.accent.g, page.accent.b, 0.5)

    Column {
      id: addBody
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(8)

      Row {
        width: parent.width
        spacing: Style.space(8)
        RowLabel { text: page.strings.t("auto.add.profile") }
        Segmented {
          anchors.verticalCenter: parent.verticalCenter
          options: page.profiles
          value: page.addProfile
          foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
          onChanged: function(v) { page.addProfile = v }
        }
      }

      Row {
        width: parent.width
        spacing: Style.space(8)
        RowLabel { id: whenLabel; text: page.strings.t("auto.add.when") }
        Flow {
          width: parent.width - whenLabel.width - parent.spacing
          spacing: Style.space(6)
          Repeater {
            model: page.matchers
            Pick {
              required property var modelData
              label: modelData.label
              onClicked: page.addRule(modelData.matcher)
            }
          }
        }
      }

      Text {
        visible: page.procsLoading || page.procsFailed
        width: parent.width
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
        text: page.strings.t(page.procsLoading ? "auto.add.loading" : "auto.add.noprocs")
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.caption
      }

      Button {
        text: page.strings.t("auto.add.cancel")
        fontSize: Style.font.caption
        foreground: page.fg
        fontFamily: page.fontFamily
        bordered: true
        onClicked: page.adding = false
      }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    RowLabel { text: page.strings.t("auto.otherwise") }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: page.profiles
      value: page.st ? String(page.st.base || "") : ""
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.root.applyProfile(v) }
    }
  }

  // ---- safety ----
  Section {
    visible: page.alive
    width: parent.width
    text: page.strings.t("auto.safety").toUpperCase()
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    Toggle {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.boot")
      checked: !!(page.st && page.st.boot_apply)
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.request("boot", { enabled: on }) }
    }
    Hint { text: page.strings.t("auto.boot.hint") }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    Toggle {
      id: wdToggle
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.watchdog")
      checked: page.wdOn
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.request("watchdog", { enabled: on }) }
    }
    Hint {
      width: parent.width - wdToggle.width - parent.spacing
      text: Auto.watchdogLine(page.wd, Model.plain(Model.profileLabel(page.wd ? page.wd.fallback : "")))
      color: page.wdOn ? page.fg : page.dim
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(14)
    Stepper {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.watchdog.cpu")
      value: Auto.watchdogShown(page.wdEdits, page.wd, "cpu_max_c")
      valueText: value + "°"
      from: Auto.WATCHDOG_BOUNDS.cpu_max_c[0]
      to: Auto.WATCHDOG_BOUNDS.cpu_max_c[1]
      enabled: page.wdOn
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.stepWatchdog("cpu_max_c", v) }
    }
    Stepper {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.watchdog.gpu")
      value: Auto.watchdogShown(page.wdEdits, page.wd, "gpu_max_c")
      valueText: value + "°"
      from: Auto.WATCHDOG_BOUNDS.gpu_max_c[0]
      to: Auto.WATCHDOG_BOUNDS.gpu_max_c[1]
      enabled: page.wdOn
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.stepWatchdog("gpu_max_c", v) }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(14)
    Stepper {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.watchdog.for")
      value: Auto.watchdogShown(page.wdEdits, page.wd, "seconds")
      valueText: value + " s"
      from: Auto.WATCHDOG_BOUNDS.seconds[0]
      to: Auto.WATCHDOG_BOUNDS.seconds[1]
      enabled: page.wdOn
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.stepWatchdog("seconds", v) }
    }
    Toggle {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("auto.watchdog.errors")
      checked: !!(page.wd && page.wd.on_errors)
      enabled: page.wdOn
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.request("watchdog", { on_errors: on }) }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    RowLabel { text: page.strings.t("auto.watchdog.fallback") }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: page.profiles
      value: page.wd ? String(page.wd.fallback || "") : ""
      enabled: page.wdOn
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.request("watchdog", { fallback: v }) }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)
    RowLabel { text: page.strings.t("auto.confirm") }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: Auto.CONFIRM_OPTIONS
      value: Auto.confirmValue(page.st)
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) {
        var s = Auto.confirmSeconds(v)
        if (s !== null) page.request("confirm-seconds", { seconds: s })
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
