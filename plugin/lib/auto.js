// Pure helpers for the Auto page (rules, boot apply, watchdog, Keep/Revert
// window). No QML types and no Model.js import: QML's `.import` directive
// would not parse under node, so what needs a profile label takes it as an
// argument. tests/auto.test.js loads this file under node.

// Longest process name the picker shows (a comm is 15 bytes; argv0 can be
// anything, so cap what reaches the layout).
var NAME_MAX = 64

var CONFIRM_OPTIONS = [
  { value: "10", label: "10 s" },
  { value: "20", label: "20 s" },
  { value: "60", label: "60 s" }
]

// Mirrors the clamps in the daemon's "watchdog" request.
var WATCHDOG_BOUNDS = { cpu_max_c: [60, 105], gpu_max_c: [60, 110], seconds: [1, 120] }

var BUILTIN_MATCHERS = [
  { matcher: "steam-game", label: "Any Steam game" },
  { matcher: "gamemode", label: "GameMode games" }
]

function ruleLabel(matcher) {
  if (matcher === null || matcher === undefined) return ""
  var m = String(matcher)
  for (var i = 0; i < BUILTIN_MATCHERS.length; i++) if (BUILTIN_MATCHERS[i].matcher === m) return BUILTIN_MATCHERS[i].label
  if (m.indexOf("process:") === 0) return m.substring(8)
  return m
}

// The rules as the daemon lists them (first match wins), with their index
// for rule-del / rule-set.
function ruleRows(st) {
  var rules = st && st.rules && st.rules.length !== undefined ? st.rules : []
  var out = []
  for (var i = 0; i < rules.length; i++) {
    var r = rules[i] || {}
    out.push({ index: i, matcher: String(r.matcher || ""), label: ruleLabel(r.matcher), profile: String(r.profile || "") })
  }
  return out
}

// What "Add rule" offers: the two built-in matchers, then one entry per
// running program (the daemon's `processes` reply, or null when the daemon
// does not have that request), minus what already has a rule.
function matcherOptions(procs, rules) {
  var taken = {}
  var list = rules && rules.length !== undefined ? rules : []
  for (var i = 0; i < list.length; i++) if (list[i] && list[i].matcher) taken[String(list[i].matcher)] = true
  var out = []
  for (var b = 0; b < BUILTIN_MATCHERS.length; b++) {
    if (!taken[BUILTIN_MATCHERS[b].matcher]) out.push({ matcher: BUILTIN_MATCHERS[b].matcher, label: BUILTIN_MATCHERS[b].label })
  }
  var names = []
  var seen = {}
  var src = procs && typeof procs === "object" && procs.length !== undefined ? procs : []
  for (var p = 0; p < src.length; p++) {
    var n = src[p] && typeof src[p].name === "string" ? src[p].name.trim().substring(0, NAME_MAX) : ""
    if (n === "" || seen[n]) continue
    seen[n] = true
    names.push(n)
  }
  names.sort()
  for (var k = 0; k < names.length; k++) {
    var m = "process:" + names[k]
    if (!taken[m]) out.push({ matcher: m, label: names[k] })
  }
  return out
}

// ---- Keep/Revert window ---------------------------------------------------

function confirmValue(st) {
  var s = st && st.confirm_seconds !== undefined && st.confirm_seconds !== null ? String(Math.round(Number(st.confirm_seconds))) : ""
  for (var i = 0; i < CONFIRM_OPTIONS.length; i++) if (CONFIRM_OPTIONS[i].value === s) return s
  return ""
}

function confirmSeconds(value) {
  for (var i = 0; i < CONFIRM_OPTIONS.length; i++) if (CONFIRM_OPTIONS[i].value === String(value)) return Number(value)
  return null
}

// ---- watchdog -------------------------------------------------------------

function degrees(v) {
  var n = Number(v)
  return isFinite(n) ? Math.round(n) + "°" : "—"
}

// "→ Stock if CPU ≥ 98° / GPU ≥ 100° for 5 s, or hardware errors". The
// fallback label comes in already neutralised by the caller.
function watchdogLine(wd, fallbackLabel) {
  if (!wd) return ""
  var s = "→ " + String(fallbackLabel || "") + " if CPU ≥ " + degrees(wd.cpu_max_c) + " / GPU ≥ " + degrees(wd.gpu_max_c) +
    " for " + (Number(wd.seconds) || 0) + " s"
  if (wd.on_errors) s += ", or hardware errors"
  return s
}

function watchdogClamp(key, v) {
  var b = WATCHDOG_BOUNDS[key]
  var n = Math.round(Number(v) || 0)
  return b ? Math.max(b[0], Math.min(b[1], n)) : n
}

// A stepper shows what the user asked for until the daemon's status agrees:
// the status arrives once a second, and a held button steps from the shown
// value.
function watchdogShown(edits, wd, key) {
  if (edits && Object.prototype.hasOwnProperty.call(edits, key)) return watchdogClamp(key, edits[key])
  var v = wd ? Number(wd[key]) : NaN
  if (isFinite(v)) return watchdogClamp(key, v)
  var b = WATCHDOG_BOUNDS[key]
  return b ? b[0] : 0
}

// The edits the daemon has not caught up with yet.
function watchdogSettled(edits, wd) {
  var out = {}
  if (!edits) return out
  for (var k in edits) {
    if (wd && isFinite(Number(wd[k])) && watchdogClamp(k, wd[k]) === watchdogClamp(k, edits[k])) continue
    out[k] = edits[k]
  }
  return out
}

// ---- profiles -------------------------------------------------------------

// Options for a profile picker, in the daemon's order; `label` is
// Model.profileLabel and the result is neutralised for any sink that is not
// PlainText.
function profileOptions(st, label) {
  if (!st || !st.profiles || st.profiles.length === undefined) return []
  var out = []
  for (var i = 0; i < st.profiles.length; i++) {
    var name = String(st.profiles[i].name || "")
    if (name === "") continue
    var l = label ? label(name) : name
    out.push({ value: name, label: String(l).replace(/</g, "‹").replace(/>/g, "›") })
  }
  return out
}

if (typeof module !== "undefined") {
  module.exports = {
    NAME_MAX: NAME_MAX, CONFIRM_OPTIONS: CONFIRM_OPTIONS, WATCHDOG_BOUNDS: WATCHDOG_BOUNDS, BUILTIN_MATCHERS: BUILTIN_MATCHERS,
    ruleLabel: ruleLabel, ruleRows: ruleRows, matcherOptions: matcherOptions,
    confirmValue: confirmValue, confirmSeconds: confirmSeconds,
    watchdogLine: watchdogLine, watchdogClamp: watchdogClamp, watchdogShown: watchdogShown, watchdogSettled: watchdogSettled,
    profileOptions: profileOptions
  }
}
