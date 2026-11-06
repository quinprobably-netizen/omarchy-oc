// Pure helpers for the Fans page, ported from the qqq.fans widget's
// Model.js. No QML types here so tests/fans.test.js can load this file
// under node.
//
// Two statuses meet here: `st` is omarchy-fand's own status (the `fans`
// proxy's `status` reply, polled by the page), `oc` is omarchy-ocd's pushed
// status, whose `fans.fans` carries the live rpm/percent per fan.
//
// Nerd Font glyphs are surrogate-pair escapes (see Model.js).

var ROLE_LABELS = { cpu: "CPU fan", case: "Case", gpu: "GPU fan" }
var ROLE_SHORT = { cpu: "CPU", case: "Case", gpu: "GPU" }
var PROFILE_ICONS = { silent: "\uDB82\uDD04", balanced: "\uDB83\uDF85", performance: "\uDB81\uDCC5", full: "\uDB80\uDE10", custom: "\uDB81\uDC93" }
// Five profiles share one segmented row: "Performance" does not fit.
var PROFILE_SHORT = { performance: "Perf" }

// Chart range. Below 20°C nothing on a desktop is interesting; above 100°C
// everything is already at full speed.
var T_MIN = 20
var T_MAX = 100

// Same as Model.plain: angle brackets to single-angle quotation marks, for
// text that may reach a sink whose format the page cannot set.
function plain(s) {
  if (s === null || s === undefined) return ""
  return String(s).replace(/</g, "‹").replace(/>/g, "›")
}

function sensor(st, id) {
  if (!st || !st.sensors) return null
  for (var i = 0; i < st.sensors.length; i++) if (st.sensors[i].id === id) return st.sensors[i]
  return null
}

function sensorTemp(st, id) {
  var s = sensor(st, id)
  return s && s.temp !== null && s.temp !== undefined ? Number(s.temp) : NaN
}

function formatTemp(value) {
  return isFinite(value) ? Math.round(value) + "°" : "—"
}

function connectedFans(st) {
  if (!st || !st.fans) return []
  // Uncalibrated headers that have never spun are almost always empty.
  // The GPU fan is always present; it rests at 0 rpm when the card is cool.
  return st.fans.filter(function(f) {
    return f.connected && (f.kind === "gpu" || f.calibrated || f.seen_spinning || f.rpm > 0)
  })
}

// One editor tab per (role, sensor) curve in the active profile, limited to
// roles that actually have a connected fan and sensors present on this box.
function curveTabs(st) {
  var tabs = []
  if (!st || !st.curves) return tabs
  var fans = connectedFans(st)
  var roles = ["cpu", "case", "gpu"]
  for (var r = 0; r < roles.length; r++) {
    var role = roles[r]
    if (!fans.some(function(f) { return f.role === role })) continue
    var curves = st.curves[role] || {}
    var sensorIds = Object.keys(curves).sort(function(a, b) { return a === "cpu" ? -1 : b === "cpu" ? 1 : a < b ? -1 : 1 })
    for (var s = 0; s < sensorIds.length; s++) {
      var sid = sensorIds[s]
      var sen = sensor(st, sid)
      if (!sen) continue
      tabs.push({
        key: role + ":" + sid,
        role: role,
        sensor: sid,
        // Compact so five tabs fit: "CPU fan", "Case ← GPU", "GPU ← VRAM".
        label: sid === role ? ROLE_LABELS[role] : ROLE_SHORT[role] + " ← " + plain(sen.label),
        points: curves[sid]
      })
    }
  }
  return tabs
}

function findTab(tabs, key) {
  for (var i = 0; i < tabs.length; i++) if (tabs[i].key === key) return tabs[i]
  return tabs.length > 0 ? tabs[0] : null
}

function interp(points, x) {
  if (!points || points.length === 0) return 0
  if (x <= points[0][0]) return points[0][1]
  for (var i = 1; i < points.length; i++) {
    var a = points[i - 1], b = points[i]
    if (x <= b[0]) return b[0] === a[0] ? b[1] : a[1] + (b[1] - a[1]) * (x - a[0]) / (b[0] - a[0])
  }
  return points[points.length - 1][1]
}

function clonePoints(points) {
  return (points || []).map(function(p) { return [Number(p[0]), Number(p[1])] })
}

// Move point `index` to (t, p), clamped so temps stay strictly increasing
// (1°C apart) and percent never decreases — the same rules the daemon
// enforces, so a drag can never produce a curve it would reject.
function movePoint(points, index, t, p) {
  var pts = clonePoints(points)
  if (index < 0 || index >= pts.length) return pts
  var tLo = index > 0 ? pts[index - 1][0] + 1 : 0
  var tHi = index < pts.length - 1 ? pts[index + 1][0] - 1 : 110
  var pLo = index > 0 ? pts[index - 1][1] : 0
  var pHi = index < pts.length - 1 ? pts[index + 1][1] : 100
  pts[index] = [
    Math.round(Math.max(tLo, Math.min(tHi, t))),
    Math.round(Math.max(pLo, Math.min(pHi, p)))
  ]
  return pts
}

function addPoint(points, t, p) {
  var pts = clonePoints(points)
  if (pts.length >= 8) return pts
  t = Math.round(t)
  for (var i = 0; i < pts.length; i++) if (pts[i][0] === t) return pts
  var idx = 0
  while (idx < pts.length && pts[idx][0] < t) idx++
  var lo = idx > 0 ? pts[idx - 1][1] : 0
  var hi = idx < pts.length ? pts[idx][1] : 100
  pts.splice(idx, 0, [t, Math.round(Math.max(lo, Math.min(hi, p)))])
  return pts
}

function removePoint(points, index) {
  var pts = clonePoints(points)
  if (pts.length <= 2 || index < 0 || index >= pts.length) return pts
  pts.splice(index, 1)
  return pts
}

// "40:10,60:30", the omarchy-fans CLI's spelling of a curve.
function pointsArg(points) {
  return points.map(function(p) { return p[0] + ":" + p[1] }).join(",")
}

// The inner request the `fans` proxy forwards: omarchy-fand takes the
// points as JSON pairs, and integers are what the editor produces.
function curveRequest(tab, points) {
  if (!tab) return null
  return {
    cmd: "curve",
    role: tab.role,
    sensor: tab.sensor,
    points: clonePoints(points).map(function(p) { return [Math.round(p[0]), Math.round(p[1])] })
  }
}

// A connected fan whose speed ignored PWM at calibration: the BIOS drives it.
function ignoresPwm(f) {
  return !!f && f.connected === true && f.responsive === false
}

function fanTag(f) {
  if (ignoresPwm(f)) return "ignores PWM · BIOS"
  if (f && f.kind === "gpu") return ""
  return f && f.connected && !f.calibrated ? "uncalibrated" : ""
}

// How to make a PWM-deaf fan controllable. A 3-pin fan has no PWM wire, so
// only the header's Voltage (DC) mode, a BIOS setting, can slow it down.
function fanHint(f) {
  if (!ignoresPwm(f)) return ""
  return "3-pin fan: set this header to Voltage mode in the BIOS (Smart Fan → Fan Control Mode), then reboot. " +
    "It is re-tested at every start and goes onto its curve once it responds."
}

function retestRunning(st, fanId) {
  var c = st && st.calibration
  return !!c && c.kind === "retest" && (!fanId || (c.fans || []).indexOf(fanId) >= 0)
}

function modeLabel(st, alive) {
  if (!st || !alive) return "Offline"
  if (st.calibration) return (st.calibration.kind === "retest" ? "Re-testing " : "Calibrating ") + Math.round(Number(st.calibration.progress || 0) * 100) + "%"
  if (st.mode === "auto") return "BIOS"
  return "Curve"
}

function profileLabel(st) {
  if (!st || !st.profiles) return ""
  for (var i = 0; i < st.profiles.length; i++) if (st.profiles[i].id === st.profile) return st.profiles[i].label
  return st.profile || ""
}

// {value, label} per omarchy-fand profile, for the Segmented control.
function profileOptions(st) {
  if (!st || !st.profiles) return []
  return st.profiles.map(function(p) {
    return { value: String(p.id), label: PROFILE_SHORT[p.id] || plain(p.label || p.id) }
  })
}

// Under the title: "Balanced · Curve · omarchy-fand".
function caption(st, alive) {
  if (!st || !alive) return "Offline"
  return profileLabel(st) + " · " + modeLabel(st, alive) + " · omarchy-fand"
}

// What the calibrate button says while omarchy-fand works: its step, with
// the progress, or the mode label's generic text.
function calibrationText(st) {
  var c = st && st.calibration
  if (!c) return ""
  var pct = Math.round(Number(c.progress || 0) * 100) + "%"
  return c.step ? plain(c.step) + " · " + pct : modeLabel(st, true)
}

// The listed fans (omarchy-fand's connected ones) with the live reading
// from omarchy-ocd's pushed status on top: the page polls omarchy-fand
// only every 2 s, the push comes every tick.
function fanRows(st, oc) {
  var live = {}
  var pushed = oc && oc.fans && oc.fans.fans ? oc.fans.fans : []
  for (var i = 0; i < pushed.length; i++) live[pushed[i].id] = pushed[i]
  return connectedFans(st).map(function(f) {
    var l = live[f.id]
    var out = {}
    for (var k in f) out[k] = f[k]
    out.label = plain(f.label || f.id)
    if (l) {
      if (l.rpm !== undefined) out.rpm = l.rpm
      if (l.percent !== undefined) out.percent = l.percent
    }
    return out
  })
}

function rpmText(f) {
  if (f && f.stalled) return "STALLED"
  return f && f.rpm !== null && f.rpm !== undefined ? Math.round(Number(f.rpm)) + " rpm" : "—"
}

function percentText(f) {
  return f && f.percent !== null && f.percent !== undefined ? Math.round(Number(f.percent)) + " %" : ""
}

// Fill of the speed bar. In BIOS mode there is no commanded percent, so
// the rpm share of the calibrated maximum stands in.
function speedShare(f) {
  if (!f) return 0
  var s
  if (f.percent !== null && f.percent !== undefined) s = Number(f.percent) / 100
  else if (f.max_rpm) s = (Number(f.rpm) || 0) / Number(f.max_rpm)
  else s = 0
  return Math.max(0, Math.min(1, isFinite(s) ? s : 0))
}

function hasStall(st) {
  return connectedFans(st).some(function(f) { return f.stalled })
}

function hasCritical(st) {
  if (!st || !st.sensors) return false
  return st.sensors.some(function(s) { return s.temp !== null && s.temp >= s.critical })
}

function cpuFanRpm(st) {
  var fans = connectedFans(st)
  for (var i = 0; i < fans.length; i++) if (fans[i].role === "cpu" && fans[i].rpm !== null) return fans[i].rpm
  return fans.length > 0 && fans[0].rpm !== null ? fans[0].rpm : NaN
}

if (typeof module !== "undefined") {
  module.exports = {
    ROLE_LABELS: ROLE_LABELS, ROLE_SHORT: ROLE_SHORT, PROFILE_ICONS: PROFILE_ICONS, T_MIN: T_MIN, T_MAX: T_MAX,
    plain: plain, sensor: sensor, sensorTemp: sensorTemp, formatTemp: formatTemp, connectedFans: connectedFans,
    curveTabs: curveTabs, findTab: findTab, interp: interp, clonePoints: clonePoints, movePoint: movePoint,
    addPoint: addPoint, removePoint: removePoint, pointsArg: pointsArg, curveRequest: curveRequest,
    ignoresPwm: ignoresPwm, fanTag: fanTag, fanHint: fanHint, retestRunning: retestRunning, modeLabel: modeLabel,
    profileLabel: profileLabel, profileOptions: profileOptions, caption: caption, calibrationText: calibrationText,
    fanRows: fanRows, rpmText: rpmText, percentText: percentText, speedShare: speedShare, hasStall: hasStall,
    hasCritical: hasCritical, cpuFanRpm: cpuFanRpm
  }
}
