const test = require("node:test")
const assert = require("node:assert/strict")
const G = require("../lib/gpu.js")
const M = require("../Model.js")

// The gpu block of one daemon status event (see src/gpu.rs), plus the
// gpu block of info.json for the caption.
const status = {
  active: "custom",
  gpu: {
    name: "Arc A580",
    stock_power_w: 150,
    knobs: {
      max_mhz: 2400, min_mhz: 600, power_w: 150, power_window_ms: 28000,
      power_profile: "base", power_profiles: ["base", "power_saving"],
      rp0_mhz: 2400, rpe_mhz: 600, rpn_mhz: 300,
    },
    reading: {
      act_mhz: 600, busy: 52, cur_mhz: 600, power_w: 41.9, temp: 38, vram_temp: 46,
      vram_used_mib: 7733, throttled: false, throttle_reasons: [],
    },
    procs: [
      { name: "Hyprland", pid: 1257, busy: 2.5, vram_mib: 351 },
      { name: "claude-desktop", pid: 619869, busy: 1.6, vram_mib: 126 },
      { name: "llama-server", pid: 600729, busy: 0, vram_mib: 6855 },
      { name: "quickshell", pid: 670768, busy: 0, vram_mib: 308 },
      { name: "idle-thing", pid: 1, busy: 0, vram_mib: 0 },
      { name: "chromium", pid: 601835, busy: 0, vram_mib: 91 },
      { name: "a", pid: 2, busy: 0.1, vram_mib: 1 },
      { name: "b", pid: 3, busy: 0.2, vram_mib: 2 },
      { name: "c", pid: 4, busy: 0.3, vram_mib: 3 },
      { name: "<img src=x>", pid: 5, busy: 9, vram_mib: 10 },
    ],
  },
  history: {
    series: {
      gpu_mhz: [600, 600, null, 2400],
      gpu_temp: [38, 39, 38, 40],
      gpu_w: [42, 42.1, 41.9, 120],
    },
  },
}

const info = {
  gpu: {
    name: "Arc A580", vendor: "ASRock", resizable_bar: true, vram_bar_gib: 8.0,
    pcie_link: { speed: "16.0 GT/s PCIe", width: 4, max_speed: "16.0 GT/s PCIe", max_width: 4 },
  },
}

test("gpuClients: top 6 by busy then VRAM, zero/zero dropped, names neutralised", () => {
  const c = G.gpuClients(status)
  assert.equal(c.length, 6)
  assert.deepEqual(c.map(x => x.name), ["‹img src=x›", "Hyprland", "claude-desktop", "c", "b", "a"])
  assert.deepEqual(c[1], { name: "Hyprland", vram: 351, busy: 2.5 })
  assert.ok(!c.some(x => x.name === "idle-thing"))
})

test("gpuClients: VRAM breaks ties at zero busy; empty without procs", () => {
  const st = { gpu: { procs: status.gpu.procs.slice(2, 6) } }
  assert.deepEqual(G.gpuClients(st).map(x => x.name), ["llama-server", "quickshell", "chromium"])
  assert.deepEqual(G.gpuClients(null), [])
  assert.deepEqual(G.gpuClients({ gpu: {} }), [])
})

test("fmtVram", () => {
  assert.equal(G.fmtVram(6855), "6.7 GiB")
  assert.equal(G.fmtVram(351), "351 MiB")
  assert.equal(G.fmtVram(null), "—")
})

test("powerRow: stock tick, live use, 30..max(250, 1.3×stock)", () => {
  assert.deepEqual(G.powerRow(status), { value: 150, stock: 150, use: 41.9, min: 30, max: 250 })
  const big = { gpu: { stock_power_w: 300, knobs: { power_w: 300 }, reading: { power_w: 10 } } }
  assert.equal(G.powerRow(big).max, 390)
  assert.deepEqual(G.powerRow(null), { value: null, stock: null, use: null, min: 30, max: 250 })
})

test("clockRow: rpn..rp0 range, stock ticks at rp0 (max) and rpe (min)", () => {
  assert.deepEqual(G.clockRow(status, "max"), { value: 2400, stock: 2400, min: 300, max: 2400 })
  assert.deepEqual(G.clockRow(status, "min"), { value: 600, stock: 600, min: 300, max: 2400 })
  assert.deepEqual(G.clockRow(null, "min"), { value: null, stock: null, min: 100, max: 4000 })
})

test("clampClock: min never above max, max never below min", () => {
  assert.equal(G.clampClock("min", 2600, 600, 2400), 2400)
  assert.equal(G.clampClock("min", 900, 600, 2400), 900)
  assert.equal(G.clampClock("max", 500, 600, 2400), 600)
  assert.equal(G.clampClock("max", 2000, 600, 2400), 2000)
  assert.equal(G.clampClock("max", 2000, null, null), 2000)
})

test("shownClock: unflushed live batch, then the profile's tune, then the hardware knob", () => {
  const knobs = status.gpu.knobs
  assert.equal(G.shownClock("min", null, null, knobs), 600)
  assert.equal(G.shownClock("min", {}, { min_mhz: 1500, max_mhz: null }, knobs), 1500)
  assert.equal(G.shownClock("max", {}, { min_mhz: 1500, max_mhz: null }, knobs), 2400)
  assert.equal(G.shownClock("max", { "gpu.max_mhz": 1800 }, { max_mhz: 2000 }, knobs), 1800)
  assert.equal(G.shownClock("max", null, null, null), null)
})

test("clampClockEdit: the pair the daemon sees stays valid", () => {
  const knobs = status.gpu.knobs
  // Drag min to 2000, then max to 1800 inside one debounce window: the
  // min is still in the unflushed batch, so max goes out as 2000.
  const live = { "gpu.min_mhz": 2000 }
  assert.equal(G.clampClockEdit("max", 1800, live, null, knobs), 2000)
  assert.equal(G.clampClockEdit("max", 2200, live, null, knobs), 2200)
  // The profile says min 1500 while the hardware (failed write) still
  // reports 600: the max cannot go below the profile's 1500.
  const tune = { min_mhz: 1500, max_mhz: null }
  assert.equal(G.clampClockEdit("max", 1000, {}, tune, knobs), 1500)
  // And the min cannot pass the profile's max of 2000 even though the
  // hardware reports 2400.
  assert.equal(G.clampClockEdit("min", 2300, {}, { min_mhz: null, max_mhz: 2000 }, knobs), 2000)
  // Nothing known about the other side: the value goes through rounded.
  assert.equal(G.clampClockEdit("min", 700.4, null, null, null), 700)
})

test("gpuHeadline: vendor, PCIe generation, ReBAR; temp · W on the right", () => {
  const h = G.gpuHeadline(status, info)
  assert.equal(h.caption, "ASRock Arc A580 · PCIe 4.0 ×4 · ReBAR 8 GiB")
  assert.equal(h.right, "38° · 42 W")
  assert.equal(G.gpuHeadline(status, null).caption, "Arc A580")
  assert.equal(G.gpuHeadline(null, null).caption, "")
  assert.equal(G.gpuHeadline(null, null).right, "—")
  const evil = { gpu: { name: "<b>x</b>", pcie_link: { speed: "8.0 GT/s PCIe", width: 16 }, resizable_bar: false } }
  assert.equal(G.gpuHeadline(status, evil).caption, "‹b›x‹/b› · PCIe 3.0 ×16")
})

test("chartSeries: the three series with their own scales and latest-value labels", () => {
  const s = G.chartSeries(status, { clock: "#b57bff", temp: "#e8c26a", power: "#ff6b8b" })
  assert.equal(s.length, 3)
  assert.deepEqual(s[0].values, [600, 600, NaN, 2400])
  assert.equal(s[0].color, "#b57bff")
  assert.equal(s[0].label, "2400 MHz")
  // xe reports act_mhz 0 while the GT sleeps in rc6: a gap, not a drop to 0,
  // and the legend says so when the latest sample is one.
  const idle = { history: { series: { gpu_mhz: [600, 0, 600, 0] } } }
  const si = G.chartSeries(idle, {}, "idle")
  assert.deepEqual(si[0].values, [600, NaN, 600, NaN])
  assert.equal(si[0].label, "idle")
  assert.equal(G.chartSeries(idle, {})[0].label, "idle")
  assert.equal(G.chartSeries({ history: { series: { gpu_mhz: [0, 600] } } }, {})[0].label, "600 MHz")
  assert.deepEqual([s[0].min, s[0].max], [0, 2400])
  assert.equal(s[1].label, "40 °C")
  assert.deepEqual([s[1].min, s[1].max], [0, 100])
  assert.equal(s[2].label, "120 W")
  assert.deepEqual([s[2].min, s[2].max], [0, 250])
  const empty = G.chartSeries(null, {})
  assert.equal(empty.length, 3)
  assert.deepEqual(empty[0].values, [])
  assert.equal(empty[0].label, "— MHz")
})

test("profileOptions and throttleText", () => {
  assert.deepEqual(G.profileOptions(status.gpu.knobs), [
    { value: "base", label: "base" }, { value: "power_saving", label: "power saving" },
  ])
  assert.deepEqual(G.profileOptions(null), [])
  assert.equal(G.throttleText(status), "")
  const hot = { gpu: { reading: { throttled: true, throttle_reasons: ["thermal", "<x>"] } } }
  assert.equal(G.throttleText(hot), "thermal, ‹x›")
  assert.equal(G.throttleText({ gpu: { reading: { throttled: true, throttle_reasons: [] } } }), "throttled")
})

test("rail: gpu and memory are wired", () => {
  assert.equal(M.pageSource("gpu"), "pages/Gpu.qml")
  assert.equal(M.pageSource("memory"), "pages/Memory.qml")
})
