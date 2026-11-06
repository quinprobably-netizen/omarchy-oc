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

// ---- CPU page (Task 5) -----------------------------------------------------

// The 6-core status fixture: 2 Zen 4 cores (fast) and 4 Zen 4c (dense).
const status6 = {
  ...status,
  active: "custom",
  caps: { smu: false, smu_error: "host bridge config space: Permission denied (os error 13)", pm_table: false, pm_error: null },
  cpu: {
    model: "AMD Ryzen 5 8500G w/ Radeon 740M Graphics",
    reading: { mhz_avg: 3506, mhz_max: 5249, temp: 71.125, package_w: 38.2 },
    knobs: {
      pstate_mode: "active", pstate_modes: ["active", "passive", "guided"],
      governor: "performance", governors: ["performance", "powersave"],
      epp: "performance", epps: ["performance"],
      boost: true, smt: "on", has_dense: true,
      idle_states: [{ name: "POLL", disabled: false }, { name: "C1", disabled: false }, { name: "C2", disabled: false }, { name: "C3", disabled: false }],
    },
    cores: [
      { core: 0, tier: "fast", cpu: 0 }, { core: 1, tier: "dense", cpu: 1 }, { core: 2, tier: "dense", cpu: 2 },
      { core: 3, tier: "dense", cpu: 3 }, { core: 4, tier: "fast", cpu: 4 }, { core: 5, tier: "dense", cpu: 5 },
    ],
  },
  smu: { limits: null, stock: null, applied: {}, needs_reboot: [] },
}

const smuLive = {
  limits: { table_version: 1, stapm_limit: 65, stapm_value: 30, fast_limit: 88, fast_value: 38, slow_limit: 75, slow_value: 36, tctl_limit: 95, tctl_value: 71, tdc_limit: 60, tdc_value: 20, edc_limit: 90, edc_value: 25 },
  stock: { table_version: 1, stapm_limit: 65, fast_limit: 88, slow_limit: 75, tctl_limit: 95, tdc_limit: 60, edc_limit: 90 },
  applied: {},
}

test("limitRow maps a PM-table row to value/stock/use with the daemon's bounds", () => {
  assert.deepEqual(M.limitRow(smuLive, "fast"), { value: 88, stock: 88, use: 38, min: 10, max: 200 })
  assert.deepEqual(M.limitRow(smuLive, "tctl"), { value: 95, stock: 95, use: 71, min: 60, max: 100 })
  assert.deepEqual(M.limitRow(smuLive, "tdc"), { value: 60, stock: 60, use: 20, min: 10, max: 250 })
  assert.deepEqual(M.limitRow(smuLive, "edc"), { value: 90, stock: 90, use: 25, min: 10, max: 250 })
})

test("limitRow returns nulls when the PM table is unavailable", () => {
  assert.deepEqual(M.limitRow({ limits: null, stock: null }, "fast"), { value: null, stock: null, use: null, min: 10, max: 200 })
  assert.deepEqual(M.limitRow(null, "slow"), { value: null, stock: null, use: null, min: 10, max: 200 })
  // Stock is read even when the live table is gone (lockdown after boot).
  const r = M.limitRow({ limits: null, stock: smuLive.stock }, "fast")
  assert.deepEqual(r, { value: null, stock: 88, use: null, min: 10, max: 200 })
})

test("shownLimit: staged wins, then the profile, then the hardware", () => {
  assert.equal(M.shownLimit({ "smu.fast_w": 110 }, "smu.fast_w", 100, 88), 110)
  assert.equal(M.shownLimit({}, "smu.fast_w", 100, 88), 100)
  assert.equal(M.shownLimit({}, "smu.fast_w", null, 88), 88)
  assert.equal(M.shownLimit(null, "smu.fast_w", null, null), null)
  // A staged clear shows the hardware value again, not the profile's.
  assert.equal(M.shownLimit({ "smu.fast_w": null }, "smu.fast_w", 100, 88), 88)
})

test("coreTiles: 6 tiles, tiers, CO from co_cores falling back to co_all", () => {
  const tune = { cpu: {}, smu: { co_all: -10, co_cores: { "2": -20 } } }
  const tiles = M.coreTiles(status6, tune)
  assert.equal(tiles.length, 6)
  assert.deepEqual(tiles.map(t => t.core), [0, 1, 2, 3, 4, 5])
  assert.deepEqual(tiles.map(t => t.tier), ["fast", "dense", "dense", "dense", "fast", "dense"])
  assert.deepEqual(tiles.map(t => t.co), [-10, -10, -20, -10, -10, -10])
  assert.deepEqual(tiles.map(t => t.override), [false, false, true, false, false, false])
  assert.deepEqual(tiles.map(t => t.test), [null, null, null, null, null, null])
})

test("coreTiles: no CO at all is 0 (stock); status without cores gives []", () => {
  const tiles = M.coreTiles(status6, { smu: {} })
  assert.deepEqual(tiles.map(t => t.co), [0, 0, 0, 0, 0, 0])
  assert.deepEqual(M.coreTiles(status6, null).map(t => t.co), [0, 0, 0, 0, 0, 0])
  assert.deepEqual(M.coreTiles(null, null), [])
  assert.deepEqual(M.coreTiles({ cpu: {} }, null), [])
})

test("coreTiles: the profile's last CPU test marks every core", () => {
  const tune = { smu: { co_all: -10 } }
  const pass = M.coreTiles(status6, tune, { time: 1, seconds: 120, passed: true, errors: 0, detail: "" })
  assert.deepEqual(pass.map(t => t.test), ["pass", "pass", "pass", "pass", "pass", "pass"])
  const fail = M.coreTiles(status6, tune, { passed: false, errors: 3 })
  assert.equal(fail[2].test, "fail")
})

test("coreTiles: staged changes overlay the profile", () => {
  const tune = { smu: { co_all: -10, co_cores: { "2": -20 } } }
  const t1 = M.coreTiles(status6, tune, null, { "smu.co_cores.2": -15, "smu.co_cores.4": -5 })
  assert.deepEqual(t1.map(t => t.co), [-10, -10, -15, -10, -5, -10])
  assert.deepEqual(t1.map(t => t.override), [false, false, true, false, true, false])
  const t2 = M.coreTiles(status6, tune, null, { "smu.co_all": -25 })
  assert.deepEqual(t2.map(t => t.co), [-25, -25, -20, -25, -25, -25])
  // A staged clear of the per-core value falls back to co_all.
  const t3 = M.coreTiles(status6, tune, null, { "smu.co_cores.2": null })
  assert.equal(t3[2].co, -10)
  assert.equal(t3[2].override, false)
})

test("coAll reads the all-core value with staged overlay", () => {
  const tune = { smu: { co_all: -10 } }
  assert.equal(M.coAll(tune, null), -10)
  assert.equal(M.coAll(tune, { "smu.co_all": -25 }), -25)
  assert.equal(M.coAll(tune, { "smu.co_all": null }), 0)
  assert.equal(M.coAll(null, null), 0)
})

test("unstage drops one key and leaves the rest", () => {
  let s = M.stage(M.emptyStaged(), "custom", "smu.co_all", -10)
  s = M.stage(s, "custom", "smu.fast_w", 120)
  const u = M.unstage(s, "smu.co_all")
  assert.deepEqual(u.changes, { "smu.fast_w": 120 })
  assert.equal(u.profile, "custom")
  assert.equal(M.stagedCount(s), 2, "input untouched")
  assert.deepEqual(M.unstage(null, "x"), M.emptyStaged())
  assert.deepEqual(M.unstage(u, "nope").changes, { "smu.fast_w": 120 })
})

test("idleMax: the deepest enabled C-state, null when all are", () => {
  assert.equal(M.idleMax(status6.cpu.knobs), null)
  const c2 = { idle_states: [{ name: "POLL" }, { name: "C1" }, { name: "C2" }, { name: "C3", disabled: true }] }
  assert.equal(M.idleMax(c2), "C2")
  const c1 = { idle_states: [{ name: "POLL" }, { name: "C1" }, { name: "C2", disabled: true }, { name: "C3", disabled: true }] }
  assert.equal(M.idleMax(c1), "C1")
  assert.equal(M.idleMax({}), null)
  assert.equal(M.idleMax(null), null)
})

test("cpuHeadline", () => {
  const h = M.cpuHeadline(status6)
  assert.equal(h.caption, "Ryzen 5 8500G · 2× Zen 4 + 4× Zen 4c")
  assert.equal(h.right, "71° · 38 W")
  const noPower = { ...status6, cpu: { ...status6.cpu, reading: { temp: 40.2, package_w: null } } }
  assert.equal(M.cpuHeadline(noPower).right, "40°")
  const plain = { ...status6, cpu: { ...status6.cpu, model: "AMD Ryzen 7 7800X3D 8-Core Processor", knobs: { has_dense: false },
    cores: [{ core: 0, tier: "fast" }, { core: 1, tier: "fast" }] } }
  assert.equal(M.cpuHeadline(plain).caption, "Ryzen 7 7800X3D · 2 cores")
  assert.deepEqual(M.cpuHeadline(null), { caption: "", right: "—" })
  assert.equal(M.cpuHeadline({ cpu: { model: "<b>x</b>" } }).caption.indexOf("<"), -1, "model is neutralised")
})

test("tierLabel", () => {
  assert.equal(M.tierLabel("fast", true), "Zen 4")
  assert.equal(M.tierLabel("dense", true), "4c")
  assert.equal(M.tierLabel("fast", false), "")
  assert.equal(M.tierLabel("weird", true), "weird")
})

test("smuDisabledReason", () => {
  assert.equal(M.smuDisabledReason(status6), "host bridge config space: Permission denied (os error 13)")
  assert.equal(M.smuDisabledReason({ ...status6, caps: { smu: true } }), "")
  assert.equal(M.smuDisabledReason({ caps: { smu: false, smu_error: null } }), "SMU unavailable")
  assert.equal(M.smuDisabledReason(null), "SMU unavailable")
  assert.equal(M.smuDisabledReason({ caps: { smu: false, smu_error: "<img src=x>" } }).indexOf("<"), -1)
})

test("fmtCo writes a real minus and a plus", () => {
  assert.equal(M.fmtCo(-15), "−15")
  assert.equal(M.fmtCo(0), "0")
  assert.equal(M.fmtCo(5), "+5")
  assert.equal(M.fmtCo(null), "—")
})

test("edcFor keeps EDC at the stock ratio to TDC, 1.5 when unknown", () => {
  assert.equal(M.edcFor(60, 60, 90), 90)
  assert.equal(M.edcFor(80, 60, 90), 120)
  assert.equal(M.edcFor(100, null, null), 150)
  assert.equal(M.edcFor(200, 60, 90), 250, "clamped to the daemon's bound")
  assert.equal(M.edcFor(5, 60, 90), 10)
})

test("EPP options are the four the daemon accepts", () => {
  assert.deepEqual(M.EPP_OPTIONS.map(o => o.value), ["power", "balance_power", "balance_performance", "performance"])
  assert.deepEqual(M.EPP_OPTIONS.map(o => o.label), ["power", "bal-pwr", "bal-perf", "perf"])
})

test("ipcValue turns a shell argument into the JSON the daemon expects", () => {
  assert.equal(M.ipcValue("-5"), -5)
  assert.equal(M.ipcValue("120"), 120)
  assert.equal(M.ipcValue("true"), true)
  assert.equal(M.ipcValue("false"), false)
  assert.equal(M.ipcValue("null"), null)
  assert.equal(M.ipcValue(""), null)
  assert.equal(M.ipcValue("stock"), null)
  assert.equal(M.ipcValue("balance_power"), "balance_power")
  assert.equal(M.ipcValue("C2"), "C2")
})

test("the CPU page is wired into the rail", () => {
  assert.equal(M.pageSource("cpu"), "pages/Cpu.qml")
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
  assert.equal(M.pageSource("gpu"), "pages/Placeholder.qml")
})

test("takeLive holds a live batch until there is a profile to send it to", () => {
  const changes = { "cpu.epp": "power", "gpu.max_mhz": 2000 }
  assert.deepEqual(M.takeLive(changes, "stock"), { send: changes, keep: {}, profile: "stock" })
  assert.deepEqual(M.takeLive(changes, ""), { send: null, keep: changes, profile: "" }, "offline: hold")
  assert.deepEqual(M.takeLive(changes, null), { send: null, keep: changes, profile: "" }, "no status yet: hold")
  assert.deepEqual(M.takeLive({}, "stock"), { send: null, keep: {}, profile: "stock" }, "nothing to send")
  assert.deepEqual(M.takeLive(null, "stock"), { send: null, keep: {}, profile: "stock" })
})

test("activeProfile reads the live status, empty when there is none", () => {
  assert.equal(M.activeProfile(status), "performance")
  assert.equal(M.activeProfile(null), "")
  assert.equal(M.activeProfile({ ...status, active: null }), "")
})

test("requestError ignores success and the disconnect sentinel", () => {
  assert.equal(M.DISCONNECTED, "disconnected")
  assert.equal(M.requestError(true, { profile: "custom" }), "")
  assert.equal(M.requestError(false, M.DISCONNECTED), "")
  assert.equal(M.requestError(false, "risky changes need Apply: smu.co_all"), "risky changes need Apply: smu.co_all")
  assert.equal(M.requestError(false, null), "request failed")
})
