// Pure helpers for the Test page: the live view of a daemon-run stability
// test from its `test-progress` / `test-done` events, and the results kept
// per profile. No QML types and no imports, so tests/test.test.js can load
// this file under node and pages/Test.qml under QML.
//
// A CPU line is stress::Progress: {t, phase, blocks, errors, blocks_per_s,
// cores:[{core, cpus, blocks, errors, seconds}], done, passed, isa,
// message}, phase "reference" | "all cores" | "core N" | "done". A GPU line:
// {t, mode, phase, iterations, errors, gflops, vram_gbps, adapter,
// message?}. The daemon adds `kind` and `event`.

var KINDS = ["cpu", "cpu-per-core", "gpu"]

// Segmented values are strings; the request takes the integer.
var LENGTHS = [
  { value: "120", label: "2 min" },
  { value: "600", label: "10 min" },
  { value: "1800", label: "30 min" },
  { value: "3600", label: "1 h" }
]

// Shortest per-core phase, as testrun::MIN_CORE_SECONDS.
var MIN_CORE_SECONDS = 10

function num(v) {
  if (typeof v === "number") return isFinite(v) ? v : null
  if (typeof v === "string" && v.trim() !== "") { var n = Number(v); return isFinite(n) ? n : null }
  return null
}

function plain(s) {
  if (s === null || s === undefined) return ""
  return String(s).replace(/</g, "‹").replace(/>/g, "›")
}

function fmtCo(v) {
  var n = num(v)
  if (n === null) return "—"
  if (n < 0) return "−" + Math.abs(n)
  return n > 0 ? "+" + n : "0"
}

// "3:12", "1:00:00"; null when unknown.
function fmtClock(seconds) {
  var s = num(seconds)
  if (s === null) return "—"
  s = Math.max(0, Math.round(s))
  var h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60
  var pad = function(n) { return (n < 10 ? "0" : "") + n }
  return h > 0 ? h + ":" + pad(m) + ":" + pad(sec) : m + ":" + pad(sec)
}

// A test length for a history line: "2 min", "1 h", "42 s".
function fmtLength(seconds) {
  var s = num(seconds)
  if (s === null) return ""
  if (s >= 3600 && s % 3600 === 0) return (s / 3600) + " h"
  if (s >= 60) return Math.round(s / 60) + " min"
  return Math.round(s) + " s"
}

// The whole run in seconds, as testrun::command plans it: `cpu-per-core`
// is one all-core phase of `seconds` and then each of `cores` alone for
// seconds / cores, never under MIN_CORE_SECONDS. Null when the length is
// unknown (a CLI run that reported in with stress-begin).
function totalSeconds(kind, seconds, cores) {
  var s = num(seconds)
  if (s === null) return null
  if (kind !== "cpu-per-core") return s
  var n = Math.max(1, num(cores) || 0)
  return s + n * Math.max(MIN_CORE_SECONDS, Math.floor(s / n))
}

// The core a "core N" phase runs, or null.
function phaseCore(phase) {
  var m = /^core (\d+)$/.exec(String(phase || ""))
  return m ? Number(m[1]) : null
}

function isProgress(ev) { return !!ev && ev.event === "test-progress" }
function isDone(ev) { return !!ev && ev.event === "test-done" }

// The page's event list: one line per phase (a later line of the same
// phase replaces the earlier one, so the list stays a few entries long), the
// done event last. A line after the done event, or of another kind, starts
// a new test. Returns a new array; other events leave the list as it is.
function pushEvent(events, ev) {
  var list = events || []
  if (!isProgress(ev) && !isDone(ev)) return list
  var last = list.length > 0 ? list[list.length - 1] : null
  var fresh = last === null || isDone(last) || String(last.kind) !== String(ev.kind)
  var out = fresh ? [] : list.slice()
  if (isProgress(ev) && out.length > 0 && String(out[out.length - 1].phase) === String(ev.phase)) out.pop()
  out.push(ev)
  return out
}

// The daemon's verdict event, trimmed to what the page shows.
function doneView(ev) {
  if (!isDone(ev)) return null
  return {
    passed: ev.passed === true,
    errors: num(ev.errors) || 0,
    interrupted: ev.interrupted === true,
    recorded: ev.recorded === true,
    detail: plain(ev.detail)
  }
}

// Curve Optimizer in force for `core` under `tune`: its override, else the
// all-core value, else 0 (stock).
function coFor(tune, core) {
  var smu = tune && tune.smu ? tune.smu : null
  if (!smu) return 0
  var own = smu.co_cores ? num(smu.co_cores[String(core)]) : null
  if (own !== null) return own
  var all = num(smu.co_all)
  return all === null ? 0 : all
}

// A failing core gets a value 5 closer to 0 (Model.suggestCo); at 0 there
// is nothing safer to set.
function suggestion(bad, tune) {
  if (bad.length === 0) return null
  var c = bad[0]
  var from = coFor(tune, c.core)
  var to = Math.min(0, from + 5)
  if (to === from) return null
  return { core: c.core, errors: c.errors, from: from, co: to }
}

// "9877 GFLOPS" / "412 GB/s" for the GPU, "40 blocks/s" for the CPU.
function rateText(ev) {
  if (!ev) return ""
  var g = num(ev.gflops), v = num(ev.vram_gbps)
  if (g !== null && g > 0) return Math.round(g) + " GFLOPS"
  if (v !== null && v > 0) return Math.round(v) + " GB/s"
  var b = num(ev.blocks_per_s)
  if (b !== null && b > 0) return Math.round(b) + " blocks/s"
  return ""
}

// What the page draws, from the events of the current test (`pushEvent`
// order) and `ctx`: {kind, seconds, cores (status.cpu.cores), tune (the
// profile the test ran on), elapsed (seconds since status.stress.started,
// for a page that loads mid-test before its first line)}.
//
// Returns {running, phase, core, pct, left, errors, rate, message,
// cores:[{core, state:"ok"|"bad"|"run"|"idle", errors}], suggestion:{core,
// errors, from, co}|null, done:{passed, errors, interrupted, recorded,
// detail}|null, gpu:{iterations, gflops, vram_gbps}|null}.
//
// A core is "bad" once it made an error, "run" while its phase runs (every
// core in the all-core phase), "ok" once its last phase ended clean, else
// "idle". In a per-core run a core's own phase is over when it appeared in
// an earlier line, or (page reloaded mid-test, one line only) when its
// total seconds exceed the all-core phase.
function testView(events, ctx) {
  var c = ctx || {}
  var list = events || []
  var kind = String(c.kind || "")
  var last = null, done = null, seenPhases = {}
  for (var i = 0; i < list.length; i++) {
    var ev = list[i]
    if (isDone(ev)) { done = doneView(ev); if (kind === "") kind = String(ev.kind || "") }
    else if (isProgress(ev)) {
      if (last !== null) seenPhases[String(last.phase)] = true
      last = ev
      if (kind === "") kind = String(ev.kind || "")
    }
  }
  // The caller knows from status.stress whether a test runs; without that,
  // a line or an elapsed time means one does, until its done event.
  var running = done === null && (c.running !== undefined ? c.running === true : last !== null || c.elapsed !== undefined)
  var coreList = c.cores || []
  var total = totalSeconds(kind, c.seconds, coreList.length)
  var t = last !== null ? num(last.t) : (c.elapsed !== undefined ? num(c.elapsed) : null)
  var pct = 0, left = null
  if (done !== null && !done.interrupted && done.passed) { pct = 100; left = 0 }
  else if (total !== null && t !== null) { pct = Math.max(0, Math.min(100, Math.round(100 * t / total))); left = Math.max(0, Math.round(total - t)) }
  if (done !== null) left = 0

  var phase = last !== null ? plain(last.phase) : ""
  var runningCore = running && last !== null ? phaseCore(last.phase) : null
  var allPhase = running && (last === null ? kind === "cpu" || kind === "cpu-per-core" : String(last.phase) === "all cores")
  var perCore = kind === "cpu-per-core"
  var allSeconds = num(c.seconds)

  var byCore = {}
  if (last !== null && last.cores) for (var j = 0; j < last.cores.length; j++) byCore[String(last.cores[j].core)] = last.cores[j]
  var cores = [], bad = []
  if (kind === "cpu" || kind === "cpu-per-core") {
    for (var k = 0; k < coreList.length; k++) {
      var id = coreList[k].core
      var r = byCore[String(id)] || null
      var errors = r ? num(r.errors) || 0 : 0
      var tested = r !== null && (num(r.blocks) || 0) > 0
      var ownDone = perCore && (seenPhases["core " + id] === true || (r !== null && allSeconds !== null && (num(r.seconds) || 0) > allSeconds + 1))
      var state
      if (errors > 0) state = "bad"
      else if (allPhase || runningCore === id) state = "run"
      else if (perCore ? ownDone : tested) state = "ok"
      else state = "idle"
      if (last === null && !running) state = "idle"
      cores.push({ core: id, state: state, errors: errors })
      if (errors > 0) bad.push({ core: id, errors: errors })
    }
    if (last === null && !running) cores = []
  }

  var gpu = null
  if (kind === "gpu" && last !== null) gpu = { iterations: num(last.iterations) || 0, gflops: num(last.gflops), vram_gbps: num(last.vram_gbps) }

  return {
    running: running,
    phase: phase,
    core: runningCore,
    pct: pct,
    left: left,
    errors: done !== null ? done.errors : (last !== null ? num(last.errors) || 0 : 0),
    rate: rateText(last),
    message: last !== null ? plain(last.message) : "",
    cores: cores,
    suggestion: suggestion(bad, c.tune),
    done: done,
    gpu: gpu
  }
}

// Model.coreTiles output with the live states on top: `test` "pass" |
// "fail" | "run" | null and the error count, for CoreGrid.
function tileStates(tiles, view) {
  var by = {}
  var list = view && view.cores ? view.cores : []
  for (var i = 0; i < list.length; i++) by[String(list[i].core)] = list[i]
  return (tiles || []).map(function(t) {
    var r = by[String(t.core)] || null
    var out = {}
    for (var k in t) out[k] = t[k]
    out.test = r === null ? null : r.state === "ok" ? "pass" : r.state === "bad" ? "fail" : r.state === "run" ? "run" : null
    out.errors = r === null ? 0 : r.errors
    return out
  })
}

// Every profile's recorded CPU and GPU result, newest first:
// [{profile, kind, time, seconds, passed, errors, detail, label}].
function testHistory(st) {
  var out = []
  var profiles = st && st.profiles ? st.profiles : []
  for (var i = 0; i < profiles.length; i++) {
    var p = profiles[i]
    var pair = [["cpu", p.cpu_test], ["gpu", p.gpu_test]]
    for (var j = 0; j < pair.length; j++) {
      var r = pair[j][1]
      if (!r || typeof r !== "object") continue
      var name = plain(p.name)
      out.push({
        profile: name,
        kind: pair[j][0],
        time: num(r.time) || 0,
        seconds: num(r.seconds),
        passed: r.passed === true,
        errors: num(r.errors) || 0,
        detail: plain(r.detail),
        label: name + " · " + pair[j][0].toUpperCase() + " " + fmtLength(r.seconds)
      })
    }
  }
  out.sort(function(a, b) { return b.time - a.time })
  return out
}

if (typeof module !== "undefined") {
  module.exports = {
    KINDS: KINDS, LENGTHS: LENGTHS, MIN_CORE_SECONDS: MIN_CORE_SECONDS,
    fmtClock: fmtClock, fmtLength: fmtLength, fmtCo: fmtCo, totalSeconds: totalSeconds, phaseCore: phaseCore,
    pushEvent: pushEvent, testView: testView, tileStates: tileStates, testHistory: testHistory, coFor: coFor
  }
}
