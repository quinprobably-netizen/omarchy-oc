import QtQuick
import qs.Commons
import qs.Ui as Ui
import "../Model.js" as Model
import "../lib/fans.js" as Fans
import "../components"

// Fans page: what the qqq.fans widget did, through omarchy-ocd's `fans`
// proxy to omarchy-fand. Profile strip and live RPM list, one curve editor
// tab per (fan role, sensor), calibrate / re-test and the BIOS hand-over.
//
// Two statuses: `st` is omarchy-ocd's pushed status (live rpm/percent in
// st.fans.fans every tick); `fst` is omarchy-fand's full status (curves,
// sensors, calibration), polled every 2 s while the panel is open.
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

  property var fst: null
  property bool polled: false
  property bool polling: false
  property bool refreshAgain: false
  // `statusError` belongs to the status poll, `fansError` to the last
  // command: a rejected command stays on screen until the next one succeeds.
  property string statusError: ""
  property string fansError: ""

  readonly property bool fansAlive: alive && fst !== null
  readonly property var rows: Fans.fanRows(fst, st)
  readonly property var tabs: Fans.curveTabs(fst)
  property string tabKey: ""
  readonly property var tab: Fans.findTab(tabs, tabKey)
  readonly property bool calibrating: fansAlive && !!fst.calibration
  readonly property bool biosMode: fansAlive && fst.mode === "auto"
  readonly property bool critical: fansAlive && Fans.hasCritical(fst)
  readonly property bool stalled: fansAlive && Fans.hasStall(fst)

  property bool confirmCalibrate: false

  spacing: Style.space(8)

  // ---- omarchy-fand status, through the proxy ----

  Timer {
    interval: 2000
    repeat: true
    triggeredOnStart: true
    running: page.alive && page.root.opened
    onTriggered: page.refresh()
  }

  onAliveChanged: {
    if (alive) return
    fst = null
    polled = false
    polling = false
  }

  function refresh() {
    if (polling) { refreshAgain = true; return }
    polling = true
    root.client.request("fans", { req: { cmd: "status" } }, function(ok, r) {
      if (!page) return
      page.polling = false
      page.polled = true
      if (ok && r && typeof r === "object" && Array.isArray(r.fans)) {
        page.fst = r
        page.statusError = ""
      } else {
        page.fst = null
        page.statusError = Model.requestError(ok, r)
      }
      if (page.refreshAgain) { page.refreshAgain = false; page.refresh() }
    })
  }

  // Commands go out one at a time, in order, so a fast series of curve
  // edits lands on omarchy-fand in the order it was made. Each reply is
  // followed by a status refresh, which also resolves the editor's draft.
  // Calibrate and re-test reply at once too (omarchy-fand runs them on a
  // thread and reports progress in its status), so they take the same queue.
  property var queue: []
  property bool busy: false

  function send(req) {
    queue = queue.concat([req])
    pump()
  }

  function pump() {
    if (busy || queue.length === 0) return
    var req = queue[0]
    queue = queue.slice(1)
    busy = true
    root.client.request("fans", { req: req }, function(ok, r) {
      if (!page) return
      page.busy = false
      page.fansError = Model.requestError(ok, r)
      page.refresh()
      page.pump()
    })
  }

  function setProfile(id) {
    if (!id) return
    send({ cmd: "profile", name: String(id) })
  }

  function saveCurve(points) {
    var req = Fans.curveRequest(tab, points)
    if (req) send(req)
  }

  function setBios(on) {
    send({ cmd: "mode", mode: on ? "auto" : "curve" })
  }

  function calibrate() {
    if (!confirmCalibrate) {
      confirmCalibrate = true
      confirmTimer.restart()
      return
    }
    confirmCalibrate = false
    send({ cmd: "calibrate" })
  }

  function retest(fanId) {
    send({ cmd: "retest", fans: [String(fanId)] })
  }

  Timer { id: confirmTimer; interval: 4000; onTriggered: page.confirmCalibrate = false }

  // Tab walks the curve tabs.
  function tabRequested(direction) {
    if (tabs.length === 0 || !tab) return
    var i = 0
    for (; i < tabs.length; i++) if (tabs[i].key === tab.key) break
    tabKey = tabs[(i + (direction < 0 ? -1 : 1) + tabs.length) % tabs.length].key
  }

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

  component Hint: Text {
    width: parent.width
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
        text: page.strings.t("page.fans")
        color: page.critical || page.stalled ? page.urgent : page.fg
        font.family: page.fontFamily
        font.pixelSize: Style.font.title
        font.bold: true
      }
      Text {
        textFormat: Text.PlainText
        width: parent.width
        elide: Text.ElideRight
        text: Fans.caption(page.fst, page.fansAlive).toUpperCase()
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
      text: Fans.formatTemp(Fans.sensorTemp(page.fst, "cpu"))
      color: page.critical ? page.urgent : page.fg
      font.family: page.fontFamily
      font.pixelSize: Style.font.display
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

  Text {
    visible: page.alive && page.polled && !page.fansAlive
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.strings.t("fans.offline") + (page.statusError !== "" ? "\n" + page.statusError : "")
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  // ---- sensors ----
  Flow {
    visible: page.fansAlive
    width: parent.width
    spacing: Style.space(14)
    Repeater {
      model: page.fansAlive && page.fst.sensors ? page.fst.sensors : []
      Row {
        required property var modelData
        spacing: Style.space(5)
        Text {
          textFormat: Text.PlainText
          text: Fans.plain(modelData.label)
          color: page.dim
          font.family: page.fontFamily
          font.pixelSize: Style.font.bodySmall
        }
        Text {
          textFormat: Text.PlainText
          text: modelData.ok === false ? "lost" : Fans.formatTemp(Number(modelData.temp))
          color: modelData.ok === false || Number(modelData.temp) >= Number(modelData.critical) ? page.urgent : page.fg
          font.family: page.fontFamily
          font.pixelSize: Style.font.bodySmall
          font.bold: true
        }
      }
    }
  }

  // ---- profile ----
  Row {
    visible: page.fansAlive
    width: parent.width
    spacing: Style.space(8)
    RowLabel { text: page.strings.t("fans.profile") }
    Segmented {
      anchors.verticalCenter: parent.verticalCenter
      options: Fans.profileOptions(page.fst)
      value: page.fansAlive && !page.biosMode ? String(page.fst.profile || "") : ""
      enabled: page.fansAlive && !page.calibrating
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onChanged: function(v) { page.setProfile(v) }
    }
  }

  // ---- fans ----
  Repeater {
    model: page.fansAlive ? page.rows : []

    Item {
      id: fanRow
      required property var modelData
      readonly property bool deaf: Fans.ignoresPwm(modelData)
      readonly property bool retesting: Fans.retestRunning(page.fst, modelData.id)
      readonly property string tag: Fans.fanTag(modelData)
      width: parent.width
      implicitHeight: line.implicitHeight + (fanHint.visible ? Style.space(2) + fanHint.implicitHeight : 0)

      Row {
        id: line
        width: parent.width
        spacing: Style.space(8)

        Text {
          width: line.width - track.width - pct.width - rpm.width - line.spacing * 3
            - (retestButton.visible ? retestButton.width + line.spacing : 0)
          anchors.verticalCenter: parent.verticalCenter
          elide: Text.ElideRight
          textFormat: Text.PlainText
          text: fanRow.modelData.label + (fanRow.tag !== "" ? " · " + fanRow.tag : "")
          color: fanRow.deaf ? page.dim : page.fg
          font.family: page.fontFamily
          font.pixelSize: Style.font.bodySmall
        }

        Rectangle {
          id: track
          width: Style.space(90)
          height: Style.space(5)
          anchors.verticalCenter: parent.verticalCenter
          radius: height / 2
          color: Qt.rgba(page.fg.r, page.fg.g, page.fg.b, 0.12)
          Rectangle {
            height: parent.height
            radius: parent.radius
            color: fanRow.modelData.stalled ? page.urgent : page.accent
            width: Math.max(height, parent.width * Fans.speedShare(fanRow.modelData))
            Behavior on width { NumberAnimation { duration: Style.duration(200); easing.type: Easing.OutCubic } }
          }
        }

        Text {
          id: pct
          width: Style.space(34)
          anchors.verticalCenter: parent.verticalCenter
          horizontalAlignment: Text.AlignRight
          textFormat: Text.PlainText
          text: Fans.percentText(fanRow.modelData)
          color: page.dim
          font.family: page.fontFamily
          font.pixelSize: Style.font.bodySmall
        }

        Text {
          id: rpm
          width: Style.space(64)
          anchors.verticalCenter: parent.verticalCenter
          horizontalAlignment: Text.AlignRight
          textFormat: Text.PlainText
          text: Fans.rpmText(fanRow.modelData)
          color: fanRow.modelData.stalled ? page.urgent : page.fg
          font.family: page.fontFamily
          font.pixelSize: Style.font.bodySmall
          font.bold: true
        }

        // Fans that ignored PWM: check again on demand (omarchy-fand also
        // does this at every start, e.g. after a BIOS change).
        Ui.Button {
          id: retestButton
          visible: fanRow.deaf
          anchors.verticalCenter: parent.verticalCenter
          iconText: "\uDB81\uDC53"
          iconSpinning: fanRow.retesting
          text: fanRow.retesting ? Fans.calibrationText(page.fst) : page.strings.t("fans.retest")
          tooltipText: page.strings.t("fans.retest.tip")
          fontSize: Style.font.caption
          foreground: page.fg
          fontFamily: page.fontFamily
          horizontalPadding: Style.space(6)
          verticalPadding: Style.space(2)
          bordered: true
          enabled: !page.calibrating
          onClicked: page.retest(fanRow.modelData.id)
        }
      }

      Hint {
        id: fanHint
        visible: fanRow.deaf
        anchors.top: line.bottom
        anchors.topMargin: Style.space(2)
        text: Fans.fanHint(fanRow.modelData)
      }
    }
  }

  Text {
    visible: page.fansAlive && page.rows.length === 0
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: page.strings.t("fans.none")
    color: page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.bodySmall
  }

  // ---- curve editor ----
  Section {
    visible: page.fansAlive && page.tab !== null
    width: parent.width
    text: (page.strings.t("fans.curve") + (page.tab ? " · " + page.tab.label : "")).toUpperCase()
  }

  Segmented {
    visible: page.fansAlive && page.tabs.length > 1
    options: page.tabs.map(function(t) { return { value: t.key, label: t.label } })
    value: page.tab ? page.tab.key : ""
    enabled: page.fansAlive
    foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
    onChanged: function(v) { page.tabKey = v }
  }

  CurveEditor {
    visible: page.fansAlive && page.tab !== null
    width: parent.width
    points: page.tab ? page.tab.points : []
    currentTemp: page.tab ? Number((Fans.sensor(page.fst, page.tab.sensor) || {}).held) : NaN
    foreground: page.fg
    accent: page.accent
    fontFamily: page.fontFamily
    editable: page.fansAlive && !page.calibrating
    onEdited: function(points) { page.saveCurve(points) }
  }

  Hint {
    visible: page.fansAlive && page.tab !== null
    text: page.fansAlive && page.fst.profile !== "custom"
      ? page.strings.t("fans.curve.hint.fork").arg(Fans.profileLabel(page.fst))
      : page.strings.t("fans.curve.hint")
  }

  // ---- actions ----
  Row {
    visible: page.fansAlive
    width: parent.width
    spacing: Style.space(14)

    Ui.Button {
      anchors.verticalCenter: parent.verticalCenter
      iconText: page.calibrating ? "\uDB81\uDC53" : "\uDB81\uDD1B"
      iconSpinning: page.calibrating
      text: page.calibrating
        ? Fans.calibrationText(page.fst)
        : page.strings.t(page.confirmCalibrate ? "fans.calibrate.confirm" : "fans.calibrate")
      tooltipText: page.strings.t("fans.calibrate.tip")
      fontSize: Style.font.caption
      foreground: page.confirmCalibrate ? page.urgent : page.fg
      fontFamily: page.fontFamily
      horizontalPadding: Style.space(8)
      verticalPadding: Style.space(3)
      bordered: true
      enabled: !page.calibrating
      onClicked: page.calibrate()
    }

    Toggle {
      anchors.verticalCenter: parent.verticalCenter
      label: page.strings.t("fans.bios")
      checked: page.biosMode
      enabled: page.fansAlive && !page.calibrating
      foreground: page.fg; accent: page.accent; fontFamily: page.fontFamily
      onToggled: function(on) { page.setBios(on) }
    }
  }

  Text {
    readonly property string msg: page.fansError !== "" ? page.fansError
      : page.root.error !== "" ? page.root.error
      : (page.fansAlive ? String(page.fst.message || "") : "")
    visible: msg !== ""
    width: parent.width
    wrapMode: Text.Wrap
    textFormat: Text.PlainText
    text: msg
    color: page.fansError !== "" || page.root.error !== "" || msg.indexOf("failed") >= 0 ? page.urgent : page.dim
    font.family: page.fontFamily
    font.pixelSize: Style.font.caption
  }
}
