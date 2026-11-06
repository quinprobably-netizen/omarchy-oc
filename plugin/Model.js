// Pure helpers for the Overclock panel. No QML types here so
// tests/model.test.js can load this file under node.
//
// Nerd Font glyphs are written as surrogate-pair escapes: raw private-use
// characters do not survive every editor and QML's JS engine reads \uXXXX
// pairs everywhere.

var RISKY_PREFIXES = ["smu."]
var RETRY_MS = 2000

var PROFILE_ICONS = {
  stock: "󰓦",        // 󰓦
  eco: "󰌪",          // 󰌪
  balanced: "󰾅",     // 󰾅
  performance: "󰓅",  // 󰓅
  max: "󱐋"           // 󱐋
}
var PROFILE_ICON_OTHER = "󰒓"  // 󰒓

var PROFILE_LABELS = { stock: "Stock", eco: "Eco", balanced: "Balanced", performance: "Perf", max: "Max" }

// The rail, top to bottom. `file` is set once a page exists; the rest load
// pages/Placeholder.qml until their task lands.
var PAGES = [
  { key: "overview", icon: "󰕮", label: "Overview", file: "Overview.qml" },  // 󰕮
  { key: "cpu",      icon: "󰍛", label: "CPU", file: "Cpu.qml" },            // 󰍛
  { key: "gpu",      icon: "󰢮", label: "GPU", file: "Gpu.qml" },            // 󰢮
  { key: "memory",   icon: "󰘚", label: "Memory", file: "Memory.qml" },      // 󰘚
  { key: "fans",     icon: "󰈐", label: "Fans", file: "Fans.qml" },          // 󰈐
  { key: "auto",     icon: "󰁪", label: "Auto", file: "Auto.qml" },          // 󰁪
  { key: "test",     icon: "󰙨", label: "Test", file: "Test.qml" }          // 󰙨
]

function pageIndex(key) {
  for (var i = 0; i < PAGES.length; i++) if (PAGES[i].key === key) return i
  return 0
}

function pageSource(key) {
  var p = PAGES[pageIndex(key)]
  return "pages/" + (p.file || "Placeholder.qml")
}

// ---- risk classification (mirrors tune::key_is_risky in the daemon) -------

// null, "", "stock" and "null" clear a key back to stock.
function clears(value) {
  if (value === null || value === undefined) return true
  if (typeof value === "string") {
    var s = value.trim()
    return s === "" || s === "stock" || s === "null"
  }
  return false
}

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

// smu.* is risky except Curve Optimizer at 0 (that is stock); gpu.power_w
// is risky above stock (or when stock is unknown); clearing is never risky.
function isRisky(key, value, stockGpuW) {
  if (clears(value)) return false
  var parts = String(key).split(".")
  if (parts[0] === "smu") {
    if ((parts.length === 2 && parts[1] === "co_all") || (parts.length === 3 && parts[1] === "co_cores")) {
      return num(value) !== 0
    }
    return true
  }
  if (parts.length === 2 && parts[0] === "gpu" && parts[1] === "power_w") {
    var w = num(value), stock = num(stockGpuW)
    if (w === null || stock === null) return true
    return w > stock + 0.5
  }
  return false
}

// ---- staging --------------------------------------------------------------

function emptyStaged() {
  return { profile: "", changes: {} }
}

// Returns a new staged set with `key` added, aimed at `profile`. The staged
// set follows the active profile (R22): a live edit that forks a built-in
// into custom must not drop what was staged, so nothing here starts over.
function stage(staged, profile, key, value) {
  var out = retarget(staged, profile)
  out.changes[key] = value
  return out
}

// Returns a copy of the staged set aimed at `profile` (the newly active
// one, after a fork or a switch); an empty `profile` (offline) keeps the
// old target.
function retarget(staged, profile) {
  var out = emptyStaged()
  out.profile = staged && staged.profile ? String(staged.profile) : ""
  if (profile) out.profile = String(profile)
  if (staged && staged.changes) for (var k in staged.changes) out.changes[k] = staged.changes[k]
  return out
}

// The `edit` request Apply sends: the staged keys go to the active profile,
// the one now holding the edits, not the profile they were staged on.
function applyRequest(staged, active) {
  var s = retarget(staged, active)
  return { profile: s.profile, changes: s.changes, apply: true }
}

function stagedCount(staged) {
  if (!staged || !staged.changes) return 0
  return Object.keys(staged.changes).length
}

// Returns a new staged set without `key` (a key that went live instead,
// such as Curve Optimizer back to 0, must not be applied twice).
function unstage(staged, key) {
  var out = emptyStaged()
  if (!staged) return out
  out.profile = staged.profile || ""
  for (var k in staged.changes || {}) if (k !== key) out.changes[k] = staged.changes[k]
  return out
}

function splitEdit(changes, stockGpuW) {
  var out = { safe: {}, risky: {} }
  if (!changes) return out
  for (var k in changes) (isRisky(k, changes[k], stockGpuW) ? out.risky : out.safe)[k] = changes[k]
  return out
}

// ---- text -----------------------------------------------------------------

// For a sink whose text format the plugin cannot set (bar tooltips): the
// angle brackets that make Qt treat a string as rich text become their
// single-angle quotation marks.
function plain(s) {
  if (s === null || s === undefined) return ""
  return String(s).replace(/</g, "‹").replace(/>/g, "›")
}

function fmt(v, digits, unit) {
  var n = num(v)
  if (n === null) return "—"
  return n.toFixed(digits || 0) + (unit || "")
}

function ghz(mhz) {
  var n = num(mhz)
  return n === null ? "—" : (n / 1000).toFixed(1) + "G"
}

function gib(mib) {
  var n = num(mib)
  return n === null ? "—" : (n / 1024).toFixed(1)
}

// A core that fails under test gets a Curve Optimizer value 5 closer to 0.
function suggestCo(v) {
  var n = num(v)
  if (n === null) return 0
  return Math.min(0, n + 5)
}

function profileIcon(name) {
  return PROFILE_ICONS[name] || PROFILE_ICON_OTHER
}

function profileLabel(name) {
  return PROFILE_LABELS[name] || String(name || "")
}

// ---- status readers -------------------------------------------------------

function reading(st, block) {
  return st && st[block] && st[block].reading ? st[block].reading : null
}

function cpuTemp(st) { var r = reading(st, "cpu"); return r ? r.temp : null }
function gpuTemp(st) { var r = reading(st, "gpu"); return r ? r.temp : null }
function cpuMhz(st) { var r = reading(st, "cpu"); return r ? r.mhz_avg : null }
function gpuMhz(st) { var r = reading(st, "gpu"); return r ? r.act_mhz : null }

function pendingRemaining(st) {
  return st && st.pending ? Math.max(0, Math.round(Number(st.pending.remaining) || 0)) : -1
}

// What the Keep/Revert bar under every page shows, or null while nothing
// is pending. The label is neutralised: it names a user profile.
function pendingInfo(st) {
  if (!st || !st.pending) return null
  var name = String(st.pending.profile || "")
  return { profile: name, label: plain(profileLabel(name)), remaining: pendingRemaining(st) }
}

function profileEntry(st, name) {
  if (!st || !st.profiles) return null
  for (var i = 0; i < st.profiles.length; i++) if (st.profiles[i].name === name) return st.profiles[i]
  return null
}

// One chip per profile. A risky profile that has never been kept through
// its Keep/Revert window is drawn as a warning.
function profileChips(st) {
  if (!st || !st.profiles) return []
  return st.profiles.map(function(p) {
    return {
      name: p.name,
      label: profileLabel(p.name),
      icon: profileIcon(p.name),
      on: p.name === st.active,
      warn: !!p.risky && !p.confirmed,
      builtin: !!p.builtin
    }
  })
}

function automationLine(st) {
  if (!st) return ""
  var rules = st.rules ? st.rules.length : 0
  var ruleText = rules === 0 ? "no rules" : rules === 1 ? "1 rule" : rules + " rules"
  var wd = st.watchdog && st.watchdog.enabled ? "watchdog on" : "watchdog off"
  var head
  if (st.auto_switch && st.auto_profile) head = "Auto → " + profileLabel(st.auto_profile)
  else head = (st.auto_switch ? "Auto on" : "Auto off") + " · base " + profileLabel(st.base)
  return head + " · " + ruleText + " · " + wd
}

function series(st, name) {
  var h = st && st.history && st.history.series ? st.history.series[name] : null
  if (!h) return []
  return h.map(function(v) { var n = num(v); return n === null ? NaN : n })
}

function cpuTile(st) {
  var r = reading(st, "cpu")
  var cores = st && st.cpu && st.cpu.cores ? st.cpu.cores.length : 0
  var sub = fmt(r ? r.temp : null, 0, "°")
  var w = r ? num(r.package_w) : null
  if (w !== null) {
    var lim = st.smu && st.smu.limits ? num(st.smu.limits.fast_limit) : null
    sub += " · " + fmt(w, 0, " W") + (lim !== null ? " of " + fmt(lim, 0, " W") : "")
  }
  return {
    title: "CPU" + (cores ? " · " + cores + "c" : ""),
    value: r && num(r.mhz_avg) !== null ? fmt(r.mhz_avg, 0, " MHz") : "—",
    sub: sub,
    seriesName: "cpu_mhz"
  }
}

// xe reports act_mhz 0 while the GT sleeps in rc6: the tile reads
// `idleText` then, and its sparkline draws such samples as gaps (gapZero).
function gpuTile(st, idleText) {
  var g = st ? st.gpu : null
  var r = g ? g.reading : null
  var sub = ""
  if (r) {
    sub = fmt(r.temp, 0, "°")
    var w = num(r.power_w)
    if (w !== null) {
      var lim = g.knobs ? num(g.knobs.power_w) : null
      sub += " · " + fmt(w, 0) + (lim !== null ? " of " + fmt(lim, 0) : "") + " W"
    }
  }
  var mhz = r ? num(r.act_mhz) : null
  var idle = mhz === 0
  return {
    title: "GPU" + (g && g.name ? " · " + g.name : ""),
    value: idle ? (idleText || "idle") : mhz !== null ? fmt(mhz, 0, " MHz") : "—",
    sub: sub,
    idle: idle,
    seriesName: "gpu_mhz",
    gapZero: true
  }
}

// A tile's sparkline samples; zeros become gaps for a tile that asks (the
// GPU clock, where 0 means asleep, not slow).
function tileSeries(st, tile) {
  var vals = series(st, tile ? tile.seriesName : "")
  if (!tile || !tile.gapZero) return vals
  return vals.map(function(v) { return v === 0 ? NaN : v })
}

function memTile(st) {
  var r = reading(st, "mem")
  var sub = ""
  if (r) {
    sub = gib(r.used_mib) + " / " + gib(r.total_mib) + " GiB"
    var g = reading(st, "gpu")
    if (g && num(g.vram_used_mib) !== null) sub += " · VRAM " + gib(g.vram_used_mib) + " GiB"
  }
  return {
    title: "Memory",
    value: r && num(r.used_pct) !== null ? fmt(r.used_pct, 0, " %") : "—",
    sub: sub,
    seriesName: "mem_used_pct"
  }
}

function fansTile(st) {
  var f = st ? st.fans : null
  var list = f && f.available && f.fans ? f.fans : []
  var cpu = null, rest = []
  for (var i = 0; i < list.length; i++) {
    var x = list[i]
    if (num(x.rpm) === null) continue
    if (x.role === "cpu" && cpu === null) cpu = x
    else rest.push((x.role === "gpu" ? "GPU " : x.role === "case" ? "case " : "") + fmt(x.rpm, 0))
  }
  return {
    title: "Fans" + (f && f.profile ? " · " + profileLabel(f.profile) : ""),
    value: cpu ? fmt(cpu.rpm, 0, " rpm") : "—",
    sub: rest.join(" · "),
    seriesName: "fan_cpu"
  }
}

function offlineHint(socketPath) {
  return "omarchy-ocd is not running (no daemon at " + socketPath + ").\n" +
    "Start it with:  sudo systemctl enable --now omarchy-ocd"
}

// ---- CPU page -------------------------------------------------------------

// Slider ranges, mirroring tune::Bounds in the daemon.
var LIMIT_BOUNDS = {
  stapm: [10, 200], fast: [10, 200], slow: [10, 200],
  tctl: [60, 100],
  tdc: [10, 250], edc: [10, 250], soc_tdc: [10, 250], soc_edc: [10, 250]
}

// One SMU limit as the PM table reports it: `value` is the limit in force,
// `use` the live draw against it, `stock` the limit read at first start.
// `key` is the PM-table stem: fast, slow, stapm, tctl, tdc, edc.
function limitRow(smu, key) {
  var b = LIMIT_BOUNDS[key] || [0, 100]
  var lim = smu && smu.limits ? smu.limits : null
  var stock = smu && smu.stock ? smu.stock : null
  return {
    value: lim ? num(lim[key + "_limit"]) : null,
    stock: stock ? num(stock[key + "_limit"]) : null,
    use: lim ? num(lim[key + "_value"]) : null,
    min: b[0],
    max: b[1]
  }
}

// The limit a slider shows: a staged value first (a staged clear shows the
// hardware value), then the profile's, then what the PM table reports.
function shownLimit(stagedChanges, key, tuneValue, liveValue) {
  if (stagedChanges && Object.prototype.hasOwnProperty.call(stagedChanges, key)) {
    var s = num(stagedChanges[key])
    return s !== null ? s : num(liveValue)
  }
  var t = num(tuneValue)
  return t !== null ? t : num(liveValue)
}

function stagedHas(stagedChanges, key) {
  return !!stagedChanges && Object.prototype.hasOwnProperty.call(stagedChanges, key)
}

// Curve Optimizer for all cores, with a staged change on top. Unset is 0.
function coAll(tune, stagedChanges) {
  if (stagedHas(stagedChanges, "smu.co_all")) return num(stagedChanges["smu.co_all"]) || 0
  var v = tune && tune.smu ? num(tune.smu.co_all) : null
  return v === null ? 0 : v
}

// One tile per physical core: its Curve Optimizer value (per-core override
// over the all-core value, staged changes over the profile), tier and the
// profile's last CPU test verdict. The daemon keeps one verdict per profile,
// so every core carries it.
function coreTiles(st, tune, cpuTest, stagedChanges) {
  var cores = st && st.cpu && st.cpu.cores ? st.cpu.cores : []
  var all = coAll(tune, stagedChanges)
  var perCore = tune && tune.smu && tune.smu.co_cores ? tune.smu.co_cores : {}
  var verdict = cpuTest ? (cpuTest.passed ? "pass" : "fail") : null
  return cores.map(function(c) {
    var key = "smu.co_cores." + c.core
    var own = stagedHas(stagedChanges, key) ? num(stagedChanges[key]) : num(perCore[String(c.core)])
    return {
      core: c.core,
      tier: String(c.tier || ""),
      co: own !== null ? own : all,
      override: own !== null,
      test: verdict
    }
  })
}

// Zen 4 / Zen 4c tiers; a part without dense cores has no tier to show.
function tierLabel(tier, hasDense) {
  if (!hasDense) return ""
  if (tier === "fast") return "Zen 4"
  if (tier === "dense") return "4c"
  return String(tier || "")
}

// The deepest C-state still enabled, or null when none is disabled.
function idleMax(knobs) {
  var states = knobs && knobs.idle_states ? knobs.idle_states : []
  if (states.length === 0) return null
  var deepest = null
  for (var i = 0; i < states.length; i++) if (!states[i].disabled) deepest = states[i].name
  var last = states[states.length - 1]
  return deepest === null || deepest === last.name ? null : String(deepest)
}

// "Ryzen 5 8500G · 2× Zen 4 + 4× Zen 4c" and "71° · 38 W".
function cpuHeadline(st) {
  var c = st ? st.cpu : null
  if (!c) return { caption: "", right: "—" }
  var model = plain(c.model || "").replace(/^AMD\s+/, "").replace(/\s+w\/.*$/, "").replace(/\s+\d+-Core Processor$/, "").trim()
  var cores = c.cores || []
  var fast = 0, dense = 0
  for (var i = 0; i < cores.length; i++) {
    if (cores[i].tier === "dense") dense++
    else fast++
  }
  var topo = ""
  if (c.knobs && c.knobs.has_dense && dense > 0) topo = fast + "× Zen 4 + " + dense + "× Zen 4c"
  else if (cores.length > 0) topo = cores.length + " cores"
  var caption = [model, topo].filter(function(s) { return s !== "" }).join(" · ")
  var r = c.reading
  var right = r ? fmt(r.temp, 0, "°") : "—"
  if (r && num(r.package_w) !== null) right += " · " + fmt(r.package_w, 0, " W")
  return { caption: caption, right: right }
}

// Curve Optimizer value with a real minus sign.
function fmtCo(v) {
  var n = num(v)
  if (n === null) return "—"
  if (n < 0) return "−" + Math.abs(n)
  return n > 0 ? "+" + n : "0"
}

// The TDC/EDC row moves both limits: EDC follows TDC at the stock ratio
// (1.5 when the stock table is unknown), within the daemon's bounds.
function edcFor(tdc, stockTdc, stockEdc) {
  var t = num(stockTdc), e = num(stockEdc)
  var ratio = t !== null && e !== null && t > 0 ? e / t : 1.5
  var b = LIMIT_BOUNDS.edc
  return Math.max(b[0], Math.min(b[1], Math.round((num(tdc) || 0) * ratio)))
}

// A value typed on the omarchy-shell command line: numbers and booleans as
// themselves, a clear as null, anything else as the string.
function ipcValue(s) {
  if (clears(s)) return null
  var t = String(s).trim()
  if (t === "true") return true
  if (t === "false") return false
  var n = num(t)
  return n !== null ? n : t
}

var EPP_OPTIONS = [
  { value: "power", label: "power" },
  { value: "balance_power", label: "bal-pwr" },
  { value: "balance_performance", label: "bal-perf" },
  { value: "performance", label: "perf" }
]

// Empty when the SMU can be driven; otherwise why not (shown next to the
// disabled limit sliders).
function smuDisabledReason(st) {
  var caps = st ? st.caps : null
  if (caps && caps.smu) return ""
  var e = caps ? caps.smu_error : null
  return e ? plain(e) : "SMU unavailable"
}

// The warning above the limit sliders. `noStock`: the SMU answers but the
// PM table does not read (CONFIG_STRICT_DEVMEM blocks /dev/mem), so the
// daemon has no stock STAPM/PPT/Tctl/TDC/EDC values and a changed limit
// stays until a reboot (the Curve Optimizer is unaffected: stock is 0).
// `reboot`: the fields the daemon could not put back, comma-joined, or "".
function smuStockNotice(st) {
  var caps = st && st.caps ? st.caps : null
  var noStock = !!(caps && caps.smu && !caps.pm_table)
  var list = st && st.smu && st.smu.needs_reboot ? st.smu.needs_reboot : []
  var reboot = list.map(function(f) { return plain(f) }).join(", ")
  return { noStock: noStock, reboot: reboot }
}

// ---- bar ------------------------------------------------------------------

function barLabel(st, alive, mode) {
  if (!st || !alive) return "OC offline"
  var remaining = pendingRemaining(st)
  if (remaining >= 0) return "KEEP " + remaining + "s"
  var icon = profileIcon(st.active)
  if (mode === "Icon") return icon
  if (mode === "Clocks") return icon + " " + ghz(cpuMhz(st)) + " · " + ghz(gpuMhz(st))
  return icon + " " + fmt(cpuTemp(st), 0, "°") + " · " + fmt(gpuTemp(st), 0, "°")
}

// Shown by the bar's shared tooltip, whose text format the plugin cannot
// set, so every daemon-supplied string goes through plain().
function tooltip(st, alive) {
  if (!st || !alive) return "omarchy-ocd is not running"
  var lines = []
  var head = profileLabel(st.active)
  if (st.auto_switch && st.auto_profile) head += " · auto"
  var remaining = pendingRemaining(st)
  if (remaining >= 0) head += " · keep within " + remaining + " s"
  lines.push(plain(head))
  var c = reading(st, "cpu")
  if (c) lines.push("CPU " + fmt(c.mhz_avg, 0, " MHz") + " · " + fmt(c.temp, 0, "°") + (num(c.package_w) !== null ? " · " + fmt(c.package_w, 0, " W") : ""))
  var g = st.gpu
  if (g && g.reading) lines.push(plain(g.name || "GPU") + " " + fmt(g.reading.act_mhz, 0, " MHz") + " · " + fmt(g.reading.temp, 0, "°") + " · " + fmt(g.reading.power_w, 0, " W"))
  if (st.alert) lines.push("Alert: " + plain(st.alert))
  return lines.join("\n")
}

function nextFavourite(csv, active) {
  var names = String(csv || "").split(",").map(function(s) { return s.trim() }).filter(function(s) { return s !== "" })
  if (names.length === 0) return ""
  var i = names.indexOf(active)
  return names[(i + 1) % names.length]
}

// The profile a live edit goes to: the active one in a fresh status.
function activeProfile(st) {
  return st && st.active ? String(st.active) : ""
}

// A live batch goes out only once there is a profile to send it to (a
// fresh status from a live daemon); otherwise it is held, so a slider move
// during a daemon restart lands once the daemon is back.
function takeLive(changes, profile) {
  var p = profile ? String(profile) : ""
  var has = !!changes && Object.keys(changes).length > 0
  if (has && p !== "") return { send: changes, keep: {}, profile: p }
  return { send: null, keep: has ? changes : {}, profile: p }
}

// ---- in-flight live edits -------------------------------------------------
//
// A live batch leaves `liveChanges` the moment it is sent, but the daemon
// only shows it in a status ~1 s later. Until then the sent values are the
// daemon's view (a clock clamp must see them), so Panel keeps them here.

// A new map: `inflight` with `sent` on top.
function addInflight(inflight, sent) {
  var out = {}
  for (var k in inflight || {}) out[k] = inflight[k]
  for (var s in sent || {}) out[s] = sent[s]
  return out
}

// A new map without `keys`; no keys means everything (an error reply, a
// disconnect: nothing in flight can be trusted any more).
function clearInflight(inflight, keys) {
  if (!keys) return {}
  var out = {}
  for (var k in inflight || {}) if (keys.indexOf(k) < 0) out[k] = inflight[k]
  return out
}

// The tune value a dotted key names ("gpu.min_mhz", "smu.co_cores.2").
function tuneValue(tune, key) {
  var node = tune
  var parts = String(key).split(".")
  for (var i = 0; i < parts.length; i++) {
    if (node === null || node === undefined || typeof node !== "object") return undefined
    node = node[parts[i]]
  }
  return node
}

function sameValue(a, b) {
  if (clears(a) && clears(b)) return true
  var na = num(a), nb = num(b)
  if (na !== null && nb !== null) return na === nb
  return String(a) === String(b)
}

// A new map without the keys the active profile's tune now reflects.
function settleInflight(inflight, st) {
  var p = profileEntry(st, st ? st.active : null)
  var tune = p && p.tune ? p.tune : null
  var out = {}
  for (var k in inflight || {}) {
    var have = tune ? tuneValue(tune, k) : undefined
    if (have === undefined || !sameValue(have, inflight[k])) out[k] = inflight[k]
  }
  return out
}

// ---- connection -----------------------------------------------------------

// What a callback gets as its error when the connection drops under an
// in-flight request. The offline hint already says it, so it is not an error
// to show.
var DISCONNECTED = "disconnected"

function requestError(ok, r) {
  if (ok) return ""
  if (r === DISCONNECTED) return ""
  return r === null || r === undefined || r === "" ? "request failed" : String(r)
}

function reconnect(state, nowMs) {
  return { retryAt: nowMs + RETRY_MS, attempts: (state && state.attempts ? state.attempts : 0) + 1 }
}

if (typeof module !== "undefined") {
  module.exports = {
    RISKY_PREFIXES: RISKY_PREFIXES, RETRY_MS: RETRY_MS, PAGES: PAGES, pageIndex: pageIndex, pageSource: pageSource,
    clears: clears, num: num, isRisky: isRisky, emptyStaged: emptyStaged, stage: stage, stagedCount: stagedCount,
    splitEdit: splitEdit, plain: plain, fmt: fmt, ghz: ghz, gib: gib, suggestCo: suggestCo,
    profileIcon: profileIcon, profileLabel: profileLabel, profileEntry: profileEntry, profileChips: profileChips,
    automationLine: automationLine, series: series, cpuTile: cpuTile, gpuTile: gpuTile, memTile: memTile,
    fansTile: fansTile, offlineHint: offlineHint, pendingRemaining: pendingRemaining, pendingInfo: pendingInfo,
    barLabel: barLabel, tooltip: tooltip, nextFavourite: nextFavourite, reconnect: reconnect,
    takeLive: takeLive, activeProfile: activeProfile, DISCONNECTED: DISCONNECTED, requestError: requestError,
    addInflight: addInflight, clearInflight: clearInflight, settleInflight: settleInflight, tuneValue: tuneValue,
    unstage: unstage, retarget: retarget, applyRequest: applyRequest, LIMIT_BOUNDS: LIMIT_BOUNDS, limitRow: limitRow, shownLimit: shownLimit, stagedHas: stagedHas,
    coAll: coAll, coreTiles: coreTiles, tierLabel: tierLabel, idleMax: idleMax, cpuHeadline: cpuHeadline,
    smuDisabledReason: smuDisabledReason, fmtCo: fmtCo, edcFor: edcFor, EPP_OPTIONS: EPP_OPTIONS,
    ipcValue: ipcValue, smuStockNotice: smuStockNotice, tileSeries: tileSeries
  }
}
