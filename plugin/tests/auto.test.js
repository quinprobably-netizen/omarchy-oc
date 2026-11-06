const test = require("node:test")
const assert = require("node:assert/strict")
const A = require("../lib/auto.js")
const M = require("../Model.js")

const status = {
  active: "stock", base: "balanced", auto_switch: true, auto_profile: "performance", boot_apply: true,
  rules: [
    { matcher: "steam-game", profile: "max" },
    { matcher: "process:blender*", profile: "performance" },
    { matcher: "gamemode", profile: "performance" },
  ],
  watchdog: { enabled: true, cpu_max_c: 98, gpu_max_c: 100, seconds: 5, on_errors: true, fallback: "stock" },
  confirm_seconds: 20,
}

test("ruleLabel names the three matcher kinds", () => {
  assert.equal(A.ruleLabel("steam-game"), "Any Steam game")
  assert.equal(A.ruleLabel("gamemode"), "GameMode games")
  assert.equal(A.ruleLabel("process:blender*"), "blender*")
  assert.equal(A.ruleLabel("process:llama-server"), "llama-server")
  assert.equal(A.ruleLabel("weird"), "weird")
  assert.equal(A.ruleLabel(null), "")
})

test("ruleRows keeps the daemon's order and index", () => {
  const rows = A.ruleRows(status)
  assert.deepEqual(rows.map(r => r.index), [0, 1, 2])
  assert.deepEqual(rows.map(r => r.label), ["Any Steam game", "blender*", "GameMode games"])
  assert.deepEqual(rows.map(r => r.profile), ["max", "performance", "performance"])
  assert.equal(rows[1].matcher, "process:blender*")
  assert.deepEqual(A.ruleRows(null), [])
  assert.deepEqual(A.ruleRows({ rules: null }), [])
})

test("matcherOptions: the two built-ins, then running programs, minus existing rules", () => {
  const procs = [{ name: "zsh" }, { name: "blender" }, { name: "bash" }, { name: "steam" }]
  const opts = A.matcherOptions(procs, status.rules)
  assert.deepEqual(opts.map(o => o.matcher), ["process:bash", "process:blender", "process:steam", "process:zsh"],
    "steam-game and gamemode already have rules; programs sorted")
  assert.deepEqual(opts.map(o => o.label), ["bash", "blender", "steam", "zsh"])
  const fresh = A.matcherOptions(procs, [])
  assert.deepEqual(fresh.slice(0, 2).map(o => o.matcher), ["steam-game", "gamemode"])
  assert.deepEqual(fresh.slice(0, 2).map(o => o.label), ["Any Steam game", "GameMode games"])
  assert.equal(fresh.length, 6)
})

test("matcherOptions degrades to the built-ins when the daemon has no processes request", () => {
  assert.deepEqual(A.matcherOptions(null, []).map(o => o.matcher), ["steam-game", "gamemode"])
  assert.deepEqual(A.matcherOptions("not a list", []).map(o => o.matcher), ["steam-game", "gamemode"])
})

test("matcherOptions drops junk, duplicates and over-long names", () => {
  const procs = [{ name: "bash" }, { name: "bash" }, {}, { name: 42 }, { name: "" }, { name: "x".repeat(300) }, { name: "a b" }]
  const opts = A.matcherOptions(procs, [])
  assert.deepEqual(opts.slice(2).map(o => o.label), ["a b", "bash", "x".repeat(A.NAME_MAX)])
  assert.equal(opts[2].matcher, "process:a b")
})

test("confirm window: 10, 20 and 60 s; anything else selects nothing", () => {
  assert.deepEqual(A.CONFIRM_OPTIONS.map(o => o.value), ["10", "20", "60"])
  assert.deepEqual(A.CONFIRM_OPTIONS.map(o => o.label), ["10 s", "20 s", "60 s"])
  assert.equal(A.confirmValue(status), "20")
  assert.equal(A.confirmValue({ confirm_seconds: 30 }), "")
  assert.equal(A.confirmValue(null), "")
  assert.equal(A.confirmSeconds("60"), 60)
  assert.equal(A.confirmSeconds("nope"), null)
})

test("watchdogLine describes the trip conditions", () => {
  assert.equal(A.watchdogLine(status.watchdog, "Stock"), "→ Stock if CPU ≥ 98° / GPU ≥ 100° for 5 s, or hardware errors")
  assert.equal(A.watchdogLine({ ...status.watchdog, on_errors: false }, "Stock"), "→ Stock if CPU ≥ 98° / GPU ≥ 100° for 5 s")
  assert.equal(A.watchdogLine({ ...status.watchdog, cpu_max_c: 97.5 }, "Stock"), "→ Stock if CPU ≥ 98° / GPU ≥ 100° for 5 s, or hardware errors")
  assert.equal(A.watchdogLine(null, "Stock"), "")
  assert.equal(A.watchdogLine({ ...status.watchdog, fallback: "<b>x</b>" }, M.plain("<b>x</b>")), "→ ‹b›x‹/b› if CPU ≥ 98° / GPU ≥ 100° for 5 s, or hardware errors")
})

test("watchdog edits overlay the status until it catches up", () => {
  const wd = status.watchdog
  assert.equal(A.watchdogShown({}, wd, "cpu_max_c"), 98)
  assert.equal(A.watchdogShown({ cpu_max_c: 95 }, wd, "cpu_max_c"), 95)
  assert.equal(A.watchdogShown({ cpu_max_c: 95 }, wd, "gpu_max_c"), 100)
  assert.equal(A.watchdogShown({}, null, "cpu_max_c"), A.WATCHDOG_BOUNDS.cpu_max_c[0])
  assert.equal(A.watchdogShown(null, null, "seconds"), A.WATCHDOG_BOUNDS.seconds[0])
  // Pruning: a key the daemon now reports at the edited value is dropped.
  assert.deepEqual(A.watchdogSettled({ cpu_max_c: 95, seconds: 5 }, wd), { cpu_max_c: 95 })
  assert.deepEqual(A.watchdogSettled({ cpu_max_c: 95 }, null), { cpu_max_c: 95 })
  assert.deepEqual(A.watchdogSettled(null, wd), {})
})

test("watchdog steps are clamped to the daemon's bounds", () => {
  assert.deepEqual(A.WATCHDOG_BOUNDS, { cpu_max_c: [60, 105], gpu_max_c: [60, 110], seconds: [1, 120] })
  assert.equal(A.watchdogClamp("cpu_max_c", 200), 105)
  assert.equal(A.watchdogClamp("gpu_max_c", 10), 60)
  assert.equal(A.watchdogClamp("seconds", 7.6), 8)
  assert.equal(A.watchdogClamp("nope", 7), 7)
})

test("profileOptions lists the daemon's profiles with their short labels", () => {
  const st = { profiles: [{ name: "stock" }, { name: "performance" }, { name: "custom" }] }
  assert.deepEqual(A.profileOptions(st, M.profileLabel), [
    { value: "stock", label: "Stock" }, { value: "performance", label: "Perf" }, { value: "custom", label: "custom" },
  ])
  assert.deepEqual(A.profileOptions(null, M.profileLabel), [])
  assert.deepEqual(A.profileOptions({ profiles: [{ name: "<i>" }] }, M.profileLabel), [{ value: "<i>", label: "‹i›" }])
})

test("listChanged: a Repeater model is replaced only when its content differs", () => {
  const a = A.ruleRows(status)
  const b = A.ruleRows({ ...status, rules: status.rules.map(r => ({ ...r })) })
  assert.notEqual(a, b, "fresh arrays from a fresh status object")
  assert.equal(A.listChanged(a, b), false, "same content: keep the old array (and its delegates)")
  assert.equal(A.listChanged(a, A.ruleRows({ rules: status.rules.slice(0, 2) })), true)
  assert.equal(A.listChanged(a, A.ruleRows({ ...status, rules: [status.rules[0], { matcher: "gamemode", profile: "max" }, status.rules[2]] })), true)
  assert.equal(A.listChanged([], []), false)
  assert.equal(A.listChanged(null, []), true)
  assert.equal(A.listChanged(undefined, undefined), false)
  assert.equal(A.listChanged([{ a: 1 }], [{ a: "1" }]), true)
})

test("the Auto page is wired into the rail", () => {
  assert.equal(M.pageSource("auto"), "pages/Auto.qml")
})

test("fallbackOptions offers only profiles without overclock settings", () => {
  const st = { profiles: [
    { name: "stock", risky: false }, { name: "eco", risky: false }, { name: "max", risky: true },
    { name: "custom", risky: true }, { name: "quiet", risky: false },
  ] }
  assert.deepEqual(A.fallbackOptions(st, M.profileLabel).map(o => o.value), ["stock", "eco", "quiet"])
  assert.deepEqual(A.fallbackOptions(null, M.profileLabel), [])
})
