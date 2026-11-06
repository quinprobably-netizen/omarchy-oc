const test = require("node:test")
const assert = require("node:assert/strict")
const M = require("../Model.js")

// A trimmed copy of one daemon status event (see src/daemon.rs Core::status).
const status = {
  event: "status", time: 1000, active: "performance", base: "balanced",
  auto_profile: null, auto_switch: false, boot_apply: true,
  pending: null, message: "", alert: null,
  caps: { smu: true, gpu: true, fans: true },
  cpu: {
    model: "AMD Ryzen 5 8500G w/ Radeon 740M Graphics",
    reading: { mhz_avg: 4563.4, mhz_max: 5100, temp: 71.125, util: 12.5, package_w: 38.2 },
    knobs: { epp: "performance" },
    cores: [{ core: 0, tier: "fast", cpu: 0 }, { core: 1, tier: "dense", cpu: 2 }],
  },
  smu: { limits: { fast_w: 120 }, stock: null, applied: {}, needs_reboot: [] },
  gpu: {
    name: "Arc A580",
    reading: { act_mhz: 2400, temp: 52.4, power_w: 141.2, vram_used_mib: 7000, busy: 90 },
    knobs: { power_w: 190, max_mhz: 2400, min_mhz: 600 },
    procs: [], stock_power_w: 150,
  },
  mem: { reading: { used_pct: 57.3, used_mib: 9011, total_mib: 15591 } },
  fans: {
    available: true, profile: "performance",
    fans: [
      { id: "pwm1", label: "CPU fan", role: "cpu", rpm: 1662, percent: 60 },
      { id: "pwm2", label: "System fan 1", role: "case", rpm: 1599, percent: 60 },
      { id: "gpu", label: "GPU fan", role: "gpu", rpm: 3059, percent: null },
    ],
  },
  profiles: [
    { name: "stock", builtin: true, risky: false, confirmed: false, crashes: 0 },
    { name: "balanced", builtin: true, risky: false, confirmed: false, crashes: 0 },
    { name: "performance", builtin: true, risky: false, confirmed: false, crashes: 0 },
    { name: "max", builtin: true, risky: true, confirmed: false, crashes: 1 },
    { name: "custom", builtin: false, risky: true, confirmed: true, crashes: 0 },
  ],
  rules: [{ match: "process:Cyberpunk2077.exe", profile: "max" }],
  watchdog: { enabled: true, fallback: "stock" },
  confirm_seconds: 20,
  history: { t: [1, 2, 3], series: { cpu_mhz: [4000, null, 4500], cpu_temp: [70, 71, 72] } },
}

test("isRisky mirrors the daemon's key_is_risky", () => {
  assert.equal(M.isRisky("smu.co_all", -10, 150), true)
  assert.equal(M.isRisky("smu.fast_w", 120, 150), true)
  assert.equal(M.isRisky("cpu.epp", "power", 150), false)
  assert.equal(M.isRisky("gpu.power_w", 160, 150), true)
  assert.equal(M.isRisky("gpu.power_w", 140, 150), false)
  assert.equal(M.isRisky("gpu.power_w", 150.4, 150), false, "within half a watt of stock")
  assert.equal(M.isRisky("gpu.power_w", 140, null), true, "stock unknown")
  assert.equal(M.isRisky("gpu.power_w", "160", 150), true, "numeric string")
  assert.equal(M.isRisky("gpu.max_mhz", 2400, 150), false)
  assert.equal(M.isRisky("fans.profile", "silent", 150), false)
})

test("isRisky: Curve Optimizer at 0 is stock, so it is safe", () => {
  assert.equal(M.isRisky("smu.co_all", 0, 150), false)
  assert.equal(M.isRisky("smu.co_all", "0", 150), false)
  assert.equal(M.isRisky("smu.co_cores.3", 0, 150), false)
  assert.equal(M.isRisky("smu.co_cores.3", -5, 150), true)
  assert.equal(M.isRisky("smu.co_all", -1, 150), true)
})

test("isRisky: clearing a key is never risky", () => {
  for (const v of [null, undefined, "", "stock", "null"]) {
    assert.equal(M.isRisky("smu.fast_w", v, 150), false, `clear with ${JSON.stringify(v)}`)
    assert.equal(M.isRisky("gpu.power_w", v, null), false, `clear with ${JSON.stringify(v)}`)
  }
})

test("stageKeepsProfile", () => {
  let s = M.stage({ profile: "", changes: {} }, "custom", "smu.co_all", -10)
  s = M.stage(s, "custom", "smu.fast_w", 120)
  assert.equal(s.profile, "custom")
  assert.equal(M.stagedCount(s), 2)
  assert.deepEqual(s.changes, { "smu.co_all": -10, "smu.fast_w": 120 })
  const other = M.stage(s, "other", "smu.tctl_c", 90)
  assert.equal(other.profile, "other")
  assert.equal(M.stagedCount(other), 1)
  assert.deepEqual(other.changes, { "smu.tctl_c": 90 })
  assert.equal(M.stagedCount(s), 2, "input untouched")
  assert.equal(M.stagedCount(null), 0)
})

test("stage overwrites the same key", () => {
  let s = M.stage({ profile: "custom", changes: {} }, "custom", "smu.co_all", -10)
  s = M.stage(s, "custom", "smu.co_all", -15)
  assert.deepEqual(s.changes, { "smu.co_all": -15 })
})

test("splitEdit separates safe from risky changes", () => {
  const r = M.splitEdit({ "cpu.epp": "power", "smu.co_all": -10, "gpu.power_w": 140, "smu.fast_w": 120, "smu.co_cores.2": 0 }, 150)
  assert.deepEqual(r.safe, { "cpu.epp": "power", "gpu.power_w": 140, "smu.co_cores.2": 0 })
  assert.deepEqual(r.risky, { "smu.co_all": -10, "smu.fast_w": 120 })
  assert.deepEqual(M.splitEdit({}, 150), { safe: {}, risky: {} })
  assert.deepEqual(M.splitEdit(null, 150), { safe: {}, risky: {} })
})

test("plainNeutralisesMarkup", () => {
  const p = M.plain("<img src=x>")
  assert.equal(p.indexOf("<"), -1)
  assert.equal(p.indexOf(">"), -1)
  assert.equal(p, "‹img src=x›")
  assert.equal(M.plain(null), "")
  assert.equal(M.plain(42), "42")
})

test("fmt", () => {
  assert.equal(M.fmt(null), "—")
  assert.equal(M.fmt(undefined), "—")
  assert.equal(M.fmt(NaN), "—")
  assert.equal(M.fmt("x"), "—")
  assert.equal(M.fmt(71.125, 0, "°"), "71°")
  assert.equal(M.fmt(71.125, 1, " W"), "71.1 W")
  assert.equal(M.fmt(4563), "4563")
})

test("suggestCo moves 5 toward zero and never past it", () => {
  assert.equal(M.suggestCo(-20), -15)
  assert.equal(M.suggestCo(-3), 0)
  assert.equal(M.suggestCo(0), 0)
  assert.equal(M.suggestCo(null), 0)
})

test("profileIcon", () => {
  assert.equal(M.profileIcon("stock"), "\u{F04E6}")
  assert.equal(M.profileIcon("eco"), "\u{F032A}")
  assert.equal(M.profileIcon("balanced"), "\u{F0F85}")
  assert.equal(M.profileIcon("performance"), "\u{F04C5}")
  assert.equal(M.profileIcon("max"), "\u{F140B}")
  assert.equal(M.profileIcon("custom"), "\u{F0493}")
  assert.equal(M.profileIcon("anything"), "\u{F0493}")
})

test("reconnect schedules the retry 2 s out", () => {
  const r = M.reconnect({ retryAt: 0, attempts: 3 }, 10000)
  assert.equal(r.retryAt, 12000)
  assert.equal(M.reconnect(null, 500).retryAt, 2500)
})

test("barLabel", () => {
  assert.equal(M.barLabel({ ...status, pending: { profile: "custom", previous: "stock", remaining: 14 } }, true, "Temps"), "KEEP 14s")
  assert.equal(M.barLabel(status, false, "Temps"), "OC offline")
  assert.equal(M.barLabel(null, false, "Temps"), "OC offline")
  assert.equal(M.barLabel(status, true, "Temps"), "\u{F04C5} 71° · 52°")
  assert.equal(M.barLabel(status, true, "Clocks"), "\u{F04C5} 4.6G · 2.4G")
  assert.equal(M.barLabel(status, true, "Icon"), "\u{F04C5}")
  const noGpu = { ...status, gpu: null }
  assert.equal(M.barLabel(noGpu, true, "Temps"), "\u{F04C5} 71° · —")
})

test("tooltip", () => {
  assert.equal(M.tooltip(status, false), "omarchy-ocd is not running")
  const t = M.tooltip(status, true)
  assert.match(t, /^Perf/)
  assert.ok(t.indexOf("4563 MHz") >= 0)
  assert.ok(t.indexOf("Arc A580") >= 0)
  const p = M.tooltip({ ...status, pending: { profile: "custom", previous: "stock", remaining: 14 } }, true)
  assert.ok(p.indexOf("14 s") >= 0)
  const a = M.tooltip({ ...status, alert: "CPU 99 °C <b>hot</b>" }, true)
  assert.ok(a.indexOf("<b>") < 0, "alert text is neutralised for the shared tooltip sink")
})

test("nextFavourite cycles and wraps", () => {
  assert.equal(M.nextFavourite("eco,balanced,max", "balanced"), "max")
  assert.equal(M.nextFavourite("eco,balanced,max", "max"), "eco")
  assert.equal(M.nextFavourite("eco, balanced ,max", "eco"), "balanced")
  assert.equal(M.nextFavourite("eco,balanced,max", "custom"), "eco", "not a favourite: start over")
  assert.equal(M.nextFavourite("", "custom"), "")
})

test("profile labels and chips", () => {
  assert.equal(M.profileLabel("performance"), "Perf")
  assert.equal(M.profileLabel("stock"), "Stock")
  assert.equal(M.profileLabel("custom"), "custom")
  assert.equal(M.profileLabel("my-oc"), "my-oc")
  const chips = M.profileChips(status)
  assert.deepEqual(chips.map(c => c.name), ["stock", "balanced", "performance", "max", "custom"])
  assert.equal(chips[2].on, true)
  assert.equal(chips[3].warn, true, "risky and unconfirmed")
  assert.equal(chips[4].warn, false, "risky but confirmed")
  assert.deepEqual(M.profileChips(null), [])
})

test("automation line", () => {
  assert.equal(M.automationLine(status), "Auto off · base Balanced · 1 rule · watchdog on")
  const on = { ...status, auto_switch: true, auto_profile: "max", rules: [{}, {}, {}] }
  assert.equal(M.automationLine(on), "Auto → Max · 3 rules · watchdog on")
  const idle = { ...status, auto_switch: true, auto_profile: null, rules: [], watchdog: { enabled: false } }
  assert.equal(M.automationLine(idle), "Auto on · base Balanced · no rules · watchdog off")
})

test("series pulls a history series as numbers", () => {
  assert.deepEqual(M.series(status, "cpu_mhz"), [4000, NaN, 4500])
  assert.deepEqual(M.series(status, "nope"), [])
  assert.deepEqual(M.series(null, "cpu_mhz"), [])
})

test("tile summaries", () => {
  const cpu = M.cpuTile(status)
  assert.equal(cpu.title, "CPU · 2c")
  assert.equal(cpu.value, "4563 MHz")
  assert.equal(cpu.sub, "71° · 38 W of 120 W")
  const gpu = M.gpuTile(status)
  assert.equal(gpu.title, "GPU · Arc A580")
  assert.equal(gpu.value, "2400 MHz")
  assert.equal(gpu.sub, "52° · 141 of 190 W")
  const mem = M.memTile(status)
  assert.equal(mem.value, "57 %")
  assert.equal(mem.sub, "8.8 / 15.2 GiB · VRAM 6.8 GiB")
  const fans = M.fansTile(status)
  assert.equal(fans.title, "Fans · Perf")
  assert.equal(fans.value, "1662 rpm")
  assert.equal(fans.sub, "case 1599 · GPU 3059")
  assert.equal(M.gpuTile({ ...status, gpu: null }).value, "—")
  assert.equal(M.fansTile({ ...status, fans: { available: false, fans: [] } }).value, "—")
})

test("offline hint names the socket", () => {
  const h = M.offlineHint("/run/omarchy-oc/control.sock")
  assert.ok(h.indexOf("systemctl") >= 0)
  assert.ok(h.indexOf("/run/omarchy-oc/control.sock") >= 0)
})

test("pages", () => {
  assert.deepEqual(M.PAGES.map(p => p.key), ["overview", "cpu", "gpu", "memory", "fans", "auto", "test"])
  assert.equal(M.pageIndex("gpu"), 2)
  assert.equal(M.pageIndex("nope"), 0)
  assert.equal(M.pageSource("overview"), "pages/Overview.qml")
  assert.equal(M.pageSource("cpu"), "pages/Placeholder.qml")
})
