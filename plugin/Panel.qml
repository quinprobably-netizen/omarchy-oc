import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "Model.js" as Model
import "components"

// Overclock: bar widget + panel for the omarchy-ocd tuning daemon.
//
// Talks to the daemon over its control socket (Client.qml), never through
// a CLI. The panel is an icon rail on the left and one page on the right;
// pages live in pages/ and get this item as `root`.
//
//   left click    open the panel, or keep pending settings while they count down
//   right click   cycle the favourite profiles
Panel {
  id: root

  moduleName: "qqq.oc"
  ipcTarget: "qqq.oc"
  manageIpc: false

  readonly property string socketPath: String(setting("socketPath", "/run/omarchy-oc/control.sock"))
  readonly property string barMode: String(setting("barMode", "Temps"))
  readonly property string favourites: String(setting("favourites", "eco,balanced,max"))

  Commons.Translations { id: catalog; directory: Qt.resolvedUrl("i18n") }
  readonly property var strings: catalog

  Client { id: oc; path: root.socketPath }
  property alias client: oc

  // A non-null status is always from the live connection (the client drops
  // it on disconnect). Derived values guard on `status` itself, never on
  // `alive`: two bindings on the same change are not updated in a fixed
  // order, so `alive` can still read true while `status` is already null.
  readonly property var status: oc.status
  readonly property bool alive: oc.connected && oc.status !== null
  readonly property var pending: status && status.pending ? status.pending : null
  readonly property string alert: status && status.alert ? String(status.alert) : ""
  readonly property string active: Model.activeProfile(status)
  readonly property var stockGpuW: status && status.gpu ? status.gpu.stock_power_w : null

  // ---- edit plumbing (safe keys live, risky keys staged) --------------------

  property string page: "overview"
  property int railCursor: 0
  property bool cursorActive: false
  property var staged: Model.emptyStaged()
  property var liveChanges: ({})
  // Live batches sent but not yet reflected in a status (Model.addInflight):
  // a page that must not contradict a value the daemon is about to hold
  // (the GPU clock pair) reads these between liveChanges and the tune.
  property var inflightChanges: ({})
  property string error: ""
  property var lastReport: null

  readonly property color fg: bar ? bar.barForeground : Commons.Color.foreground
  readonly property color accent: Commons.Color.accent
  readonly property color urgent: bar ? bar.urgent : Commons.Color.urgent
  readonly property color dim: Qt.darker(fg, 1.4)
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family

  // A dropped connection fails in-flight requests with Model.DISCONNECTED;
  // the offline hint covers that, so it never becomes an error line.
  function requestDone(ok, r) {
    root.error = Model.requestError(ok, r)
    if (ok && r && typeof r === "object" && r.report !== undefined) root.lastReport = r.report
  }

  // Safe keys: coalesce everything changed within 250 ms into one edit.
  function liveEdit(key, value) {
    var c = liveChanges
    c[key] = value
    liveChanges = c
    liveTimer.restart()
  }

  Timer { id: liveTimer; interval: 250; onTriggered: root.flushLive() }

  // Sends the batch against the profile in the client's current status, or
  // holds it while the daemon is away; onAliveChanged releases it once the
  // first status of the new connection is in. The profile is read from
  // oc.status directly: the derived `active` binding may not have been
  // re-evaluated yet when onAliveChanged runs.
  function flushLive() {
    var t = Model.takeLive(liveChanges, Model.activeProfile(oc.status))
    liveChanges = t.keep
    if (t.send === null) return
    var sent = t.send
    inflightChanges = Model.addInflight(inflightChanges, sent)
    oc.request("edit", { profile: t.profile, changes: sent }, function(ok, r) {
      // A refused batch never reaches the tune: forget it now.
      if (!ok) root.inflightChanges = Model.clearInflight(root.inflightChanges, Object.keys(sent))
      root.requestDone(ok, r)
    })
  }

  // Each status settles the in-flight keys the active profile's tune now
  // carries; a drop forgets them all (nothing sent can be trusted after it).
  onStatusChanged: if (status) inflightChanges = Model.settleInflight(inflightChanges, status)

  onAliveChanged: {
    if (!alive) { inflightChanges = {}; return }
    root.error = ""
    flushLive()
  }

  // Risky keys wait in `staged` until applyStaged() sends them with apply:true.
  function stageEdit(key, value) {
    staged = Model.stage(staged, active, key, value)
  }

  // A key that goes live instead (Curve Optimizer back to 0, a clear) must
  // not also be applied from the stage.
  function unstageEdit(key) {
    if (Model.stagedHas(staged.changes, key)) staged = Model.unstage(staged, key)
  }

  // What a page control calls: safe values go live, risky ones to the
  // stage. The risk is decided per value (Model.isRisky), so the same key
  // can take either route.
  function edit(key, value) {
    if (Model.isRisky(key, value, stockGpuW)) {
      stageEdit(key, value)
    } else {
      unstageEdit(key)
      liveEdit(key, value)
    }
  }

  function applyStaged() {
    if (Model.stagedCount(staged) === 0) return
    var s = staged
    oc.request("edit", { profile: s.profile, changes: s.changes, apply: true }, function(ok, r) {
      if (ok) root.discardStaged()
      root.requestDone(ok, r)
    })
  }

  function discardStaged() { staged = Model.emptyStaged() }

  function keep() { oc.request("confirm", null, requestDone) }
  function revert() { oc.request("revert", null, requestDone) }
  function clearAlert() { oc.request("clear-alert", null, requestDone) }

  function applyProfile(name) {
    if (!name) return
    oc.request("apply", { profile: String(name) }, requestDone)
  }

  function showPage(name) {
    page = String(name)
    railCursor = Model.pageIndex(page)
  }

  function moveRail(delta) {
    var n = Model.PAGES.length
    railCursor = (railCursor + delta + n) % n
    page = Model.PAGES[railCursor].key
  }

  IpcHandler {
    target: "qqq.oc"
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.toggle() }
    function page(name: string): void { root.showPage(name); root.open() }
    function keep(): void { root.keep() }
    function revert(): void { root.revert() }
    // A safe live edit from a script: omarchy-shell qqq.oc live cpu.epp power
    function live(key: string, value: string): void { root.liveEdit(key, value) }
    // The route a page control takes: omarchy-shell qqq.oc edit smu.co_all -5
    function edit(key: string, value: string): void { root.edit(key, Model.ipcValue(value)) }
  }

  onOpenedChanged: {
    if (!opened) return
    cursorActive = false
    railCursor = Model.pageIndex(page)
  }

  // ---- bar ---------------------------------------------------------------

  readonly property bool iconOnly: barMode === "Icon" && pending === null && alive
  readonly property bool urgentState: !alive || alert !== ""

  implicitWidth: iconOnly ? iconButton.implicitWidth : textButton.implicitWidth
  implicitHeight: iconOnly ? iconButton.implicitHeight : textButton.implicitHeight

  function barPressed(b) {
    if (b === Qt.RightButton) root.applyProfile(Model.nextFavourite(root.favourites, root.active))
    else if (root.pending) root.keep()
    else root.toggle()
  }

  WidgetButton {
    id: textButton
    anchors.fill: parent
    visible: !root.iconOnly
    bar: root.bar
    text: Model.barLabel(root.status, root.alive, root.barMode)
    fontSize: Style.font.caption
    horizontalMargin: 6
    active: root.urgentState
    activeColor: root.urgent
    tooltipText: Model.tooltip(root.status, root.alive)
    onPressed: function(b) { root.barPressed(b) }
  }

  BarIconButton {
    id: iconButton
    anchors.fill: parent
    visible: root.iconOnly
    bar: root.bar
    text: Model.profileIcon(root.active)
    active: root.urgentState
    activeColor: root.urgent
    tooltipText: Model.tooltip(root.status, root.alive)
    onPressed: function(b) { root.barPressed(b) }
  }

  // ---- panel -------------------------------------------------------------

  KeyboardPanel {
    id: panel
    anchorItem: root.iconOnly ? iconButton : textButton
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(540))
    contentHeight: panel.fittedContentHeight(body.implicitHeight)

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      // Arrows walk the rail; Enter keeps pending settings; Tab is the page's.
      onMoveRequested: function(dx, dy) {
        root.cursorActive = true
        root.moveRail(dy !== 0 ? dy : dx)
      }
      onActivateRequested: {
        if (root.pending) root.keep()
        else if (pageLoader.item && typeof pageLoader.item.activate === "function") pageLoader.item.activate()
      }
      onCloseRequested: root.close()
      onTabRequested: function(direction) {
        if (pageLoader.item && typeof pageLoader.item.tabRequested === "function") pageLoader.item.tabRequested(direction)
      }

      Row {
        id: body
        anchors.left: parent.left
        anchors.right: parent.right
        anchors.top: parent.top
        spacing: Style.space(12)

        Rail {
          id: rail
          pages: Model.PAGES
          current: root.page
          cursor: root.cursorActive ? root.railCursor : -1
          foreground: root.fg
          accent: root.accent
          fontFamily: root.fontFamily
          onSelected: function(name) { root.cursorActive = false; root.showPage(name) }
        }

        // The page, with the Keep/Revert bar and the stage bar under it on
        // every page.
        Column {
          width: body.width - rail.width - body.spacing
          spacing: Style.space(10)

          Loader {
            id: pageLoader
            width: parent.width
            readonly property string src: Model.pageSource(root.page)
            onSrcChanged: load()
            Component.onCompleted: load()
            function load() { setSource(Qt.resolvedUrl(src), { root: root }) }
          }

          PendingBar {
            width: parent.width
            root: root
            onKeep: root.keep()
            onRevert: root.revert()
          }

          StageBar {
            width: parent.width
            root: root
            onApply: root.applyStaged()
            onDiscard: root.discardStaged()
          }
        }
      }
    }
  }
}
