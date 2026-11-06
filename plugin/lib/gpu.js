// Pure helpers for the GPU page. No QML types here so tests/gpu.test.js
// can load this file under node. Self-contained (no .import) so the same
// file works from QML and node.

var CLIENTS_MAX = 6
var POWER_MIN_W = 30
var POWER_MAX_W = 250
var POWER_MAX_RATIO = 1.3
var CLOCK_BOUNDS = [100, 4000]  // tune::Bounds for gpu.min_mhz / gpu.max_mhz
var TEMP_MAX_C = 100

function num(value) {
  if (typeof value === "number") return isFinite(value) ? value : null
  if (typeof value === "string") {
    var s = value.trim()
    if (s === "") return null
    var n = Number(s)
    return isFinite(n) ? n : null
  }
  return null
}

function plain(s) {
  if (s === null || s === undefined) return ""
  return String(s).replace(/</g, "‹").replace(/>/g, "›")
}

function fmt(v, digits, unit) {
  var n = num(v)
  if (n === null) return "—" + (unit || "")
  return n.toFixed(digits || 0) + (unit || "")
}

function gpu(st) { return st && st.gpu ? st.gpu : null }
function knobs(st) { var g = gpu(st); return g && g.knobs ? g.knobs : null }
function reading(st) { var g = gpu(st); return g && g.reading ? g.reading : null }

// ---- clients --------------------------------------------------------------

// The programs on the GPU: busiest first, then by VRAM; idle entries with
// nothing resident are dropped; at most CLIENTS_MAX. Names come from
// /proc and go through plain().
function gpuClients(st) {
  var g = gpu(st)
  var procs = g && g.procs ? g.procs : []
  var out = []
  for (var i = 0; i < procs.length; i++) {
    var p = procs[i]
    var busy = num(p.busy) || 0
    var vram = num(p.vram_mib) || 0
    if (busy <= 0 && vram <= 0) continue
    out.push({ name: plain(p.name), vram: vram, busy: busy })
  }
  out.sort(function(a, b) { return b.busy - a.busy || b.vram - a.vram })
  return out.slice(0, CLIENTS_MAX)
}

function fmtVram(mib) {
  var n = num(mib)
  if (n === null) return "—"
  return n >= 1024 ? (n / 1024).toFixed(1) + " GiB" : Math.round(n) + " MiB"
}

// ---- sliders --------------------------------------------------------------

// PL1: the limit in force, stock as the gold tick, live draw as the use
// bar. Range 30 W to max(250, 1.3 × stock).
function powerRow(st) {
  var g = gpu(st), k = knobs(st), r = reading(st)
  var stock = g ? num(g.stock_power_w) : null
  return {
    value: k ? num(k.power_w) : null,
    stock: stock,
    use: r ? num(r.power_w) : null,
    min: POWER_MIN_W,
    max: stock !== null ? Math.max(POWER_MAX_W, Math.round(stock * POWER_MAX_RATIO)) : POWER_MAX_W
  }
}

// Min or max clock over rpn..rp0. The stock tick is rp0 for the max slider
// and rpe (the efficient frequency, the driver's default floor) for min.
function clockRow(st, which) {
  var k = knobs(st)
  var lo = k ? num(k.rpn_mhz) : null, hi = k ? num(k.rp0_mhz) : null
  return {
    value: k ? num(which === "max" ? k.max_mhz : k.min_mhz) : null,
    stock: k ? num(which === "max" ? k.rp0_mhz : k.rpe_mhz) : null,
    min: lo !== null ? lo : CLOCK_BOUNDS[0],
    max: hi !== null ? hi : CLOCK_BOUNDS[1]
  }
}

// The daemon refuses min above max, so the page never sends it: the min
// slider stops at the shown max and the max slider at the shown min.
function clampClock(which, v, minMhz, maxMhz) {
  var n = Math.round(num(v) || 0)
  var lo = num(minMhz), hi = num(maxMhz)
  if (which === "min" && hi !== null) return Math.min(n, hi)
  if (which === "max" && lo !== null) return Math.max(n, lo)
  return n
}

function pendingValue(map, key) {
  if (!map || !Object.prototype.hasOwnProperty.call(map, key)) return null
  return num(map[key])
}

// The clock the daemon will validate against, which is what the slider
// shows: a live edit still in the 250 ms batch, then one sent but not yet
// in a status (`inflight`, see Model.addInflight), then the profile's
// tune, then the hardware knob (the value the tune falls back to when
// unset).
function shownClock(which, liveChanges, tune, k, inflight) {
  var key = which === "max" ? "gpu.max_mhz" : "gpu.min_mhz"
  var field = which === "max" ? "max_mhz" : "min_mhz"
  var l = pendingValue(liveChanges, key)
  if (l !== null) return l
  var f = pendingValue(inflight, key)
  if (f !== null) return f
  var t = tune ? num(tune[field]) : null
  if (t !== null) return t
  return k ? num(k[field]) : null
}

// A clock edit clamped against the other clock as the daemon will see it,
// so two drags inside one batch (min up to 2000, then max down to 1800),
// or one right after the other batch went out, still send a valid pair.
function clampClockEdit(which, v, liveChanges, tune, k, inflight) {
  var other = shownClock(which === "max" ? "min" : "max", liveChanges, tune, k, inflight)
  return which === "max" ? clampClock("max", v, other, null) : clampClock("min", v, null, other)
}

// ---- header ---------------------------------------------------------------

// "16.0 GT/s PCIe" -> "4.0"; unknown rates are shown as the rate itself.
function pcieGen(speed) {
  var s = String(speed || "")
  var m = s.match(/([\d.]+)\s*GT\/s/)
  if (!m) return ""
  var gts = Number(m[1])
  var gens = { 2.5: "1.0", 5: "2.0", 8: "3.0", 16: "4.0", 32: "5.0", 64: "6.0" }
  return gens[gts] || (m[1] + " GT/s")
}

// "ASRock Arc A580 · PCIe 4.0 ×4 · ReBAR 8 GiB" and "38° · 42 W". The
// caption needs info.json (vendor, link, ReBAR); the status alone gives the
// name.
function gpuHeadline(st, info) {
  var g = gpu(st)
  var ig = info && info.gpu ? info.gpu : null
  var parts = []
  var name = plain(ig && ig.name ? ig.name : g && g.name ? g.name : "")
  var vendor = plain(ig ? ig.vendor : "")
  if (vendor !== "" && name.indexOf(vendor) !== 0) name = (vendor + " " + name).trim()
  if (name !== "") parts.push(name)
  if (ig && ig.pcie_link) {
    var gen = pcieGen(ig.pcie_link.speed)
    var width = num(ig.pcie_link.width)
    if (gen !== "") parts.push("PCIe " + gen + (width !== null ? " ×" + width : ""))
  }
  if (ig && ig.resizable_bar) {
    var bar = num(ig.vram_bar_gib)
    parts.push("ReBAR" + (bar !== null ? " " + (bar % 1 === 0 ? bar : bar.toFixed(1)) + " GiB" : ""))
  }
  var r = reading(st)
  var right = r ? fmt(r.temp, 0, "°") : "—"
  if (r && num(r.power_w) !== null) right += " · " + fmt(r.power_w, 0, " W")
  return { caption: parts.join(" · "), right: right }
}

// ---- chart ----------------------------------------------------------------

function series(st, name) {
  var h = st && st.history && st.history.series ? st.history.series[name] : null
  if (!h) return []
  return h.map(function(v) { var n = num(v); return n === null ? NaN : n })
}

function last(vals) {
  for (var i = vals.length - 1; i >= 0; i--) if (isFinite(vals[i])) return vals[i]
  return null
}

// Clock, temperature and power, each on its own scale (0..rp0, 0..100 °C,
// 0..the PL1 slider's top) so the three lines share one box. xe reports
// act_mhz 0 while the GT sleeps in rc6: those samples are gaps, and the
// legend reads `idleText` when the latest one is such a sample.
function chartSeries(st, colors, idleText) {
  var k = knobs(st)
  var c = colors || {}
  var raw = series(st, "gpu_mhz"), temp = series(st, "gpu_temp"), w = series(st, "gpu_w")
  var mhz = raw.map(function(v) { return v === 0 ? NaN : v })
  var idle = raw.length > 0 && raw[raw.length - 1] === 0
  var rp0 = k ? num(k.rp0_mhz) : null
  return [
    { values: mhz, color: c.clock || "#b57bff", label: idle ? (idleText || "idle") : fmt(last(mhz), 0, " MHz"), min: 0, max: rp0 !== null ? rp0 : CLOCK_BOUNDS[1] },
    { values: temp, color: c.temp || "#e8c26a", label: fmt(last(temp), 0, " °C"), min: 0, max: TEMP_MAX_C },
    { values: w, color: c.power || "#ff6b8b", label: fmt(last(w), 0, " W"), min: 0, max: powerRow(st).max }
  ]
}

// ---- profile and throttling ----------------------------------------------

function profileOptions(k) {
  var list = k && k.power_profiles ? k.power_profiles : []
  return list.map(function(p) { return { value: String(p), label: plain(String(p).replace(/_/g, " ")) } })
}

// Empty when the GPU is not throttling; otherwise the reasons the driver
// gives, or "throttled" when it gives none.
function throttleText(st) {
  var r = reading(st)
  if (!r || !r.throttled) return ""
  var reasons = (r.throttle_reasons || []).map(function(x) { return plain(x) }).filter(function(x) { return x !== "" })
  return reasons.length > 0 ? reasons.join(", ") : "throttled"
}

if (typeof module !== "undefined") {
  module.exports = {
    CLIENTS_MAX: CLIENTS_MAX, gpuClients: gpuClients, fmtVram: fmtVram, powerRow: powerRow, clockRow: clockRow,
    clampClock: clampClock, shownClock: shownClock, clampClockEdit: clampClockEdit, pcieGen: pcieGen, gpuHeadline: gpuHeadline, chartSeries: chartSeries,
    profileOptions: profileOptions, throttleText: throttleText
  }
}
