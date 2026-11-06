import QtQuick
import Quickshell.Io
import "Model.js" as Model

// One persistent connection to omarchy-ocd's control socket.
//
// Newline-delimited JSON both ways. Every request carries an id and gets
// its reply through the callback given to request(); the daemon pushes
// {"event": ...} lines once we have subscribed, which it does again after
// every reconnect. A connection that drops (daemon restart, or the daemon
// dropping us as a stalled subscriber) is retried every 2 s.
QtObject {
  id: client

  property string path: ""
  readonly property bool connected: socket.connected
  // Latest {"event":"status",...} line of the current connection; null
  // while disconnected, so a non-null status is always fresh.
  property var status: null
  property var lastEvent: null
  property string lastError: ""
  property int attempts: 0

  signal event(var ev)

  property int nextId: 1
  property var callbacks: ({})
  property var queue: []

  // cb(ok, resultOrError). While disconnected the line waits in `queue`
  // and goes out right after the subscribe on the next connection.
  function request(cmd, args, cb) {
    var msg = { id: nextId, cmd: cmd }
    nextId += 1
    if (args) for (var k in args) if (k !== "id" && k !== "cmd") msg[k] = args[k]
    if (typeof cb === "function") callbacks[msg.id] = cb
    var line = JSON.stringify(msg) + "\n"
    if (socket.connected) send(line)
    else queue.push(line)
    return msg.id
  }

  function send(line) {
    socket.write(line)
    socket.flush()
  }

  function handleLine(data) {
    var text = String(data).trim()
    if (text === "") return
    var msg
    try { msg = JSON.parse(text) } catch (e) { lastError = "bad line from daemon"; return }
    if (!msg || typeof msg !== "object") return
    if (msg.event !== undefined) {
      lastEvent = msg
      if (msg.event === "status") status = msg
      event(msg)
      return
    }
    if (msg.id !== undefined) {
      var cb = callbacks[msg.id]
      if (cb) {
        delete callbacks[msg.id]
        cb(msg.ok === true, msg.ok === true ? msg.result : String(msg.error || "request failed"))
      }
    }
  }

  function onConnected() {
    lastError = ""
    attempts = 0
    request("subscribe", null, function(ok, r) { if (!ok) client.lastError = "subscribe: " + r })
    var waiting = queue
    queue = []
    for (var i = 0; i < waiting.length; i++) send(waiting[i])
  }

  // In-flight requests will never be answered: fail them now. Lines that
  // never left the queue stay for the next connection.
  function onDisconnected() {
    status = null
    var cbs = callbacks
    callbacks = {}
    for (var id in cbs) cbs[id](false, Model.DISCONNECTED)
  }

  function connect() {
    if (socket.connected || path === "") return
    attempts += 1
    socket.connected = true
  }

  function disconnect() {
    socket.connected = false
  }

  property Socket socket: Socket {
    path: client.path
    parser: SplitParser {
      splitMarker: "\n"
      onRead: function(data) { client.handleLine(data) }
    }
    onConnectionStateChanged: {
      if (connected) client.onConnected()
      else client.onDisconnected()
    }
    onError: function(err) { client.lastError = "socket error " + err }
  }

  // A failed attempt frees the underlying socket, so setting `connected`
  // again starts a fresh one; a lost connection never reconnects by itself.
  property Timer retry: Timer {
    interval: Model.RETRY_MS
    repeat: true
    triggeredOnStart: true
    running: client.path !== "" && !client.socket.connected
    onTriggered: client.connect()
  }

  onPathChanged: {
    if (socket.connected) socket.connected = false
  }
}
