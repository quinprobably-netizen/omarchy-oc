import QtQuick
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "../Model.js" as Model
import "../lib/test.js" as T
import "../components"

// Test page: starts the daemon-run stability tests (CPU all cores, CPU per
// core, GPU) on the active profile, follows their progress from the
// daemon's test-progress / test-done events (the daemon owns the run, so a
// closed panel loses nothing), draws the per-core results live, offers a
// safer Curve Optimizer value for a failing core, and lists the results
// each profile keeps.
//
//   omarchy-shell qqq.oc.test start cpu 2     a 2-minute all-core CPU test
//   omarchy-shell qqq.oc.test stop
//   omarchy-shell qqq.oc.test set             stage the suggested CO value
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
  readonly property color okColor: "#8fd694"

  readonly property var stress: st && st.stress ? st.stress : null
  readonly property bool running: stress !== null
  readonly property real stressStarted: stress ? (Number(stress.started) || 0) : 0
  readonly property real elapsed: running && st ? Math.max(0, (Number(st.time) || 0) - stressStarted) : 0
  readonly property var profile: Model.profileEntry(st, root.active)
  readonly property var tune: profile && profile.tune ? profile.tune : null
  readonly property var stagedChanges: root.staged ? root.staged.changes : null
  readonly property var coreList: st && st.cpu && st.cpu.cores ? st.cpu.cores : []
  readonly property bool hasDense: !!(st && st.cpu && st.cpu.knobs && st.cpu.knobs.has_dense)

  // The test in progress as the status reports it, with the tune it runs
  // on; `run` keeps the last one once it ends, so the result and its
  // suggestion refer to the settings that were tested.
  readonly property var liveRun: {
    if (!stress) return null
    var p = Model.profileEntry(st, stress.profile)
    return {
      kind: String(stress.kind || ""),
      seconds: stress.seconds === null || stress.seconds === undefined ? null : Number(stress.seconds),
      profile: String(stress.profile || ""),
      tune: p && p.tune ? p.tune : null
    }
  }
  property var run: null
  onLiveRunChanged: if (liveRun !== null) run = liveRun

  // The current test's lines, one per phase (T.pushEvent). A new test
  // drops the previous verdict.
  property var events: []
  // A refusal ("a test is already running") is about the test of the
  // moment: it goes when that test starts or ends.
  onStressStartedChanged: {
    startError = ""
    if (stressStarted > 0 && view.done !== null) events = []
  }

  Connections {
    target: page.root.client
    function onEvent(ev) { page.events = T.pushEvent(page.events, ev) }
  }

  readonly property var view: T.testView(events, {
    kind: run ? run.kind : "",
    seconds: run ? run.seconds : null,
    cores: coreList,
    tune: run ? run.tune : tune,
    running: running,
    elapsed: running ? elapsed : undefined
  })
  readonly property var tiles: T.tileStates(Model.coreTiles(st, run ? run.tune : tune, null, null), view)
  readonly property bool showRun: view.running || view.done !== null
  readonly property var history: T.testHistory(st)

  // The suggestion stands while the profile still has the core deeper than
  // the suggested value; once that value is staged the button says so.
  readonly property var suggestion: view.suggestion !== null && T.coFor(tune, view.suggestion.core) < view.suggestion.co ? view.suggestion : null
  readonly property string suggestionKey: suggestion ? "smu.co_cores." + suggestion.core : ""
  readonly property bool suggestionStaged: suggestion !== null && Model.stagedHas(stagedChanges, suggestionKey) && Model.num(stagedChanges[suggestionKey]) === suggestion.co

  property string seconds: "600"
  property string startError: ""

  function start(kind) {
    startError = ""
    root.client.request("test-start", { kind: String(kind), seconds: Math.round(Number(seconds)) || 600 }, function(ok, r) {
      if (!ok) { page.startError = Model.requestError(ok, r); return }
      page.events = []
    })
  }

  function stop() {
    root.client.request("test-stop", null, function(ok, r) { if (!ok) page.startError = Model.requestError(ok, r) })
  }

  // "Set −15": a Curve Optimizer value is risky, so root.edit stages it.
  function applySuggestion() {
    var s = suggestion
    if (s === null) return
    root.edit("smu.co_cores." + s.core, s.co)
  }

  function headline() {
    var v = view, s = strings
    if (v.done !== null) {
      let h = v.done.interrupted ? s.t("test.stopped") : v.done.passed ? s.t("test.passed") : s.t("test.failed")
      if (v.done.errors > 0) h += " · " + s.tn("test.errors", v.done.errors).arg(v.done.errors)
      if (!v.done.recorded) h += " · " + s.t("test.unrecorded")
      return h
    }
    if (!v.running) return ""
    var parts = [s.t("test.running")]
    if (run && run.seconds === null) parts.push(s.t("test.cli"))
    else if (v.phase === "") parts.push(s.t("test.starting"))
    else if (v.phase === "reference") parts.push(s.t("test.phase.reference"))
    else if (v.phase === "all cores") parts.push(s.t("test.phase.all"))
    else if (v.core !== null) parts.push(s.t("test.phase.core").arg(v.core))
    else parts.push(v.phase)
    if (v.rate !== "") parts.push(v.rate)
    if (v.left !== null) parts.push(s.t("test.left").arg(T.fmtClock(v.left)))
    return parts.join(" · ")
  }

  function detail() {
    var v = view
    if (v.done !== null) return v.done.detail
    var parts = []
    if (v.gpu !== null) parts.push(strings.tn("test.iterations", v.gpu.iterations).arg(v.gpu.iterations))
    if (v.message !== "") parts.push(v.message)
    return parts.join(" · ")
  }

  IpcHandler {
    target: "qqq.oc.test"
    function start(kind: string, minutes: string): void {
      page.seconds = String(Math.round(Number(minutes) * 60) || 600)
      page.start(kind)
    }
    function stop(): void { page.stop() }
    function set(): void { page.applySuggestion() }
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

  component Caption: Text {
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
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
        text: page.strings.t("test.title")
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.title
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        width: parent.width
        elide: Text.ElideRight
        text: (page.st
          ? page.strings.t("test.profile").arg(Model.plain(page.running && page.run ? page.run.profile : page.root.active))
          : page.strings.t("overview.offline")).toUpperCase()
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
      text: page.running ? T.fmtClock(page.elapsed) : ""
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

  // ---- start buttons and length ----
  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(6)

    Repeater {
      model: T.KINDS

      Button {
        required property string modelData
        readonly property bool mine: page.running && page.run !== null && page.run.kind === modelData
        text: page.strings.t("test.kind." + modelData)
        fontSize: Style.font.bodySmall
        foreground: page.fg
        accent: page.accent
        fontFamily: page.fontFamily
        bordered: true
        selected: mine
        enabled: page.alive && !page.running
        opacity: enabled || mine ? 1 : 0.45
        onClicked: page.start(modelData)
      }
    }
  }

  Row {
    visible: page.alive
    width: parent.width
    spacing: Style.space(8)

    Text {
      width: Style.space(52)
      anchors.verticalCenter: parent.verticalCenter
      elide: Text.ElideRight
      textFormat: Text.PlainText
      text: page.strings.t("test.length")
      color: page.dim
      font.family: page.fontFamily
      font.pixelSize: Style.font.bodySmall
    }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: T.LENGTHS
      value: page.seconds
      enabled: page.alive && !page.running
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.seconds = v }
    }
  }

  // test-start refusals (pending settings, a test already running) and a
  // tester that could not be started.
  Text {
    visible: page.alive && page.startError !== ""
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.startError
    color: page.urgent
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }

  Caption {
    visible: page.alive && !page.showRun
    width: parent.width
    text: page.strings.t("test.idle")
  }

  // ---- progress ----
  Item {
    visible: page.alive && page.showRun
    width: parent.width
    implicitHeight: Math.max(runHead.implicitHeight, stopButton.implicitHeight)

    Section {
      id: runHead
      anchors.left: parent.left
      anchors.right: stopButton.visible ? stopButton.left : parent.right
      anchors.rightMargin: Style.space(8)
      anchors.verticalCenter: parent.verticalCenter
      topPadding: 0
      text: page.headline().toUpperCase()
      color: page.view.done !== null && !page.view.done.passed && !page.view.done.interrupted ? page.urgent : page.dim
    }
    Button {
      id: stopButton
      visible: page.running
      anchors.right: parent.right
      anchors.verticalCenter: parent.verticalCenter
      text: page.strings.t("test.stop")
      fontSize: Style.font.caption
      foreground: page.fg
      accent: page.accent
      fontFamily: page.fontFamily
      bordered: true
      onClicked: page.stop()
    }
  }

  Rectangle {
    visible: page.alive && page.showRun
    width: parent.width
    height: Math.max(4, Style.space(6))
    radius: height / 2
    color: Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.1)

    Rectangle {
      width: parent.width * page.view.pct / 100
      height: parent.height
      radius: parent.radius
      color: page.view.errors > 0 ? page.urgent : page.accent
      Behavior on width {
        NumberAnimation { duration: Style.duration(200); easing.type: Easing.Bezier; easing.bezierCurve: Style.easeOut }
      }
    }
  }

  CoreGrid {
    visible: page.alive && page.showRun && page.view.cores.length > 0 && page.tiles.length > 0
    width: parent.width
    tiles: page.tiles
    hasDense: page.hasDense
    okColor: page.okColor
    foreground: page.fg; accent: page.accent; urgent: page.urgent; fontFamily: page.fontFamily
  }

  Caption {
    visible: page.alive && page.showRun && text !== ""
    width: parent.width
    text: page.detail()
  }

  // ---- suggestion ----
  Rectangle {
    visible: page.alive && page.suggestion !== null
    width: parent.width
    implicitHeight: suggestRow.implicitHeight + Style.space(16)
    radius: Style.cornerRadius
    color: Qt.rgba(page.urgent.r, page.urgent.g, page.urgent.b, 0.14)
    border.width: 1
    border.color: Qt.rgba(page.urgent.r, page.urgent.g, page.urgent.b, 0.5)

    Row {
      id: suggestRow
      anchors.verticalCenter: parent.verticalCenter
      anchors.left: parent.left
      anchors.right: parent.right
      anchors.margins: Style.space(8)
      spacing: Style.space(8)

      Text {
        width: parent.width - setButton.width - parent.spacing
        anchors.verticalCenter: parent.verticalCenter
        wrapMode: Text.Wrap
        textFormat: Text.PlainText
        text: page.suggestion
          ? page.strings.t("test.suggest").arg(page.suggestion.core).arg(page.suggestion.errors).arg(T.fmtCo(page.suggestion.from)).arg(T.fmtCo(page.suggestion.co))
          : ""
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Button {
        id: setButton
        anchors.verticalCenter: parent.verticalCenter
        text: page.suggestionStaged ? page.strings.t("test.staged") : page.strings.t("test.set").arg(page.suggestion ? T.fmtCo(page.suggestion.co) : "")
        fontSize: Style.font.caption
        foreground: page.fg
        accent: page.accent
        fontFamily: page.fontFamily
        bordered: true
        selected: !page.suggestionStaged
        enabled: !page.suggestionStaged
        opacity: enabled ? 1 : 0.6
        onClicked: page.applySuggestion()
      }
    }
  }

  // ---- history ----
  Section {
    visible: page.alive
    width: parent.width
    text: page.strings.t("test.history").toUpperCase()
  }

  Caption {
    visible: page.alive && page.history.length === 0
    width: parent.width
    text: page.strings.t("test.history.none")
  }

  Repeater {
    model: page.alive ? page.history : []

    Item {
      id: historyRow
      required property var modelData
      width: parent.width
      implicitHeight: Math.max(historyLabel.implicitHeight, historyVerdict.implicitHeight)

      Text {
        id: historyLabel
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        text: historyRow.modelData.label
        color: page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Text {
        anchors.left: historyLabel.right
        anchors.leftMargin: Style.space(8)
        anchors.right: historyVerdict.left
        anchors.rightMargin: Style.space(8)
        anchors.verticalCenter: parent.verticalCenter
        elide: Text.ElideRight
        textFormat: Text.PlainText
        text: historyRow.modelData.detail
        color: page.dim
        font.family: page.fontFamily
        font.pixelSize: Style.font.caption
      }
      Text {
        id: historyVerdict
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        textFormat: Text.PlainText
        text: historyRow.modelData.passed
          ? page.strings.t("test.result.pass")
          : page.strings.t("test.result.fail") + (historyRow.modelData.errors > 0 ? " · " + page.strings.tn("test.errors", historyRow.modelData.errors).arg(historyRow.modelData.errors) : "")
        color: historyRow.modelData.passed ? page.okColor : page.urgent
        font.family: page.fontFamily
        font.pixelSize: Style.font.bodySmall
      }
    }
  }

  // The daemon's last message ("cpu test stopped on "stock"") or a failed
  // request of the panel's.
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
