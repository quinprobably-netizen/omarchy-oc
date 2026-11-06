const test = require("node:test")
const assert = require("node:assert/strict")
const T = require("../lib/test.js")

// Six physical cores (the 8500G: 2× Zen 4 + 4× Zen 4c), as status.cpu.cores.
const cores = [0, 1, 2, 3, 4, 5].map(c => ({ core: c, tier: c === 0 || c === 4 ? "fast" : "dense", cpu: c }))

// A progress line from `omarchy-oc stress cpu --json --per-core`, as the
// daemon relays it (kind added, event tagged). `secs` are the per-core
// totals, which the tester only updates at the end of each phase.
function progress(t, phase, opts) {
  const o = opts || {}
  const secs = o.secs || [30, 30, 30, 30, 30, 30]
  return {
    event: "test-progress", kind: o.kind || "cpu-per-core",
    t, phase, blocks: 1200, errors: o.errors ? o.errors[2] || 0 : 0, blocks_per_s: 40.5,
    cores: cores.map(c => ({ core: c.core, cpus: [c.cpu], blocks: 200, errors: o.errors ? o.errors[c.core] || 0 : 0, seconds: secs[c.core] })),
    done: false, passed: null, isa: "avx512", message: null,
  }
}

// A 30 s per-core run on 6 cores: 30 s all cores, then 10 s each core
// (MIN_CORE_SECONDS), 90 s in all. Core 2 made 3 errors in the all-core
// phase; core 0 passed its own phase; core 1 is in its phase at t=41.4
// (46 % of 90 s).
const recorded = [
  progress(0, "reference"),
  progress(12, "all cores", { errors: { 2: 3 } }),
  progress(30, "all cores", { errors: { 2: 3 } }),
  progress(35, "core 0", { errors: { 2: 3 } }),
  progress(40, "core 0", { errors: { 2: 3 } }),
  progress(41.4, "core 1", { errors: { 2: 3 }, secs: [40, 30, 30, 30, 30, 30] }),
]
const ctx = { kind: "cpu-per-core", seconds: 30, cores, tune: { smu: { co_all: null, co_cores: { "2": -20 } } } }

test("totalSeconds mirrors the daemon's per-core plan", () => {
  assert.equal(T.totalSeconds("cpu", 120, 6), 120)
  assert.equal(T.totalSeconds("gpu", 600, 6), 600)
  assert.equal(T.totalSeconds("cpu-per-core", 120, 6), 120 + 6 * 20)
  assert.equal(T.totalSeconds("cpu-per-core", 30, 6), 30 + 6 * 10, "never under 10 s per core")
  assert.equal(T.totalSeconds("cpu-per-core", 120, 0), 120 + 120, "no core list yet: one 120 s phase")
  assert.equal(T.totalSeconds("cpu", null, 6), null, "a CLI run has no known length")
})

test("testView over the recorded sequence: 46 %, core 2 bad with 3 errors, suggestion −15", () => {
  const v = T.testView(recorded, ctx)
  assert.equal(v.pct, 46)
  assert.equal(v.running, true)
  assert.equal(v.phase, "core 1")
  assert.equal(v.core, 1)
  assert.equal(v.left, 49)
  assert.equal(v.errors, 3)
  assert.deepEqual(v.cores.map(c => c.state), ["ok", "run", "bad", "idle", "idle", "idle"])
  assert.deepEqual(v.cores[2], { core: 2, state: "bad", errors: 3 })
  assert.deepEqual(v.suggestion, { core: 2, errors: 3, from: -20, co: -15 })
  assert.equal(v.done, null)
})

test("testView from the last line alone (page reloaded mid-test) gives the same per-core states", () => {
  const v = T.testView([recorded[recorded.length - 1]], ctx)
  assert.deepEqual(v.cores.map(c => c.state), ["ok", "run", "bad", "idle", "idle", "idle"], "core 0's own 10 s are in its total")
  assert.equal(v.pct, 46)
})

test("testView: the suggestion comes from the all-core value when the core has no override", () => {
  const v = T.testView(recorded, Object.assign({}, ctx, { tune: { smu: { co_all: -20, co_cores: {} } } }))
  assert.deepEqual(v.suggestion, { core: 2, errors: 3, from: -20, co: -15 })
  const stock = T.testView(recorded, Object.assign({}, ctx, { tune: null }))
  assert.equal(stock.suggestion, null, "already at 0: nothing safer to set")
  const near = T.testView(recorded, Object.assign({}, ctx, { tune: { smu: { co_cores: { "2": -3 } } } }))
  assert.deepEqual(near.suggestion, { core: 2, errors: 3, from: -3, co: 0 }, "never past 0")
  const clean = T.testView([progress(12, "all cores")], ctx)
  assert.equal(clean.suggestion, null, "no errors, no suggestion")
})

test("testView: an all-core run marks every core running, then ok when done", () => {
  const run = [progress(0, "reference", { kind: "cpu" }), progress(50, "all cores", { kind: "cpu" })]
  const c = { kind: "cpu", seconds: 120, cores, tune: null }
  const mid = T.testView(run, c)
  assert.deepEqual(mid.cores.map(x => x.state), ["run", "run", "run", "run", "run", "run"])
  assert.equal(mid.pct, 42)
  assert.equal(mid.left, 70)
  const done = Object.assign(progress(120.2, "done", { kind: "cpu" }), { done: true, passed: true })
  const fin = T.testView(run.concat([done, { event: "test-done", kind: "cpu", passed: true, errors: 0, interrupted: false, recorded: true, seconds: 120.2, detail: "avx512, 4800 blocks" }]), c)
  assert.deepEqual(fin.cores.map(x => x.state), ["ok", "ok", "ok", "ok", "ok", "ok"])
  assert.equal(fin.running, false)
  assert.equal(fin.pct, 100)
  assert.equal(fin.left, 0)
  assert.deepEqual(fin.done, { passed: true, errors: 0, interrupted: false, recorded: true, detail: "avx512, 4800 blocks" })
})

test("testView: a run stopped early keeps its progress and untested cores idle", () => {
  const stopped = recorded.concat([{ event: "test-done", kind: "cpu-per-core", passed: false, errors: 3, interrupted: true, recorded: true, seconds: 41.9, detail: "stopped before the end" }])
  const v = T.testView(stopped, ctx)
  assert.equal(v.running, false)
  assert.equal(v.pct, 46, "not rounded up to 100")
  assert.deepEqual(v.cores.map(c => c.state), ["ok", "idle", "bad", "idle", "idle", "idle"], "core 1 never finished its phase")
  assert.equal(v.done.interrupted, true)
  assert.deepEqual(v.suggestion, { core: 2, errors: 3, from: -20, co: -15 }, "the suggestion survives the stop")
})

test("testView: a reference-phase failure (no line but the done one) is a clean failure", () => {
  const ev = [{ event: "test-done", kind: "gpu", passed: false, errors: 0, interrupted: false, recorded: false, seconds: 0.1, detail: "tester exited with code 2" }]
  const v = T.testView(ev, { kind: "gpu", seconds: 120, cores, tune: null })
  assert.equal(v.running, false)
  assert.equal(v.pct, 0)
  assert.deepEqual(v.cores, [])
  assert.equal(v.done.detail, "tester exited with code 2")
  assert.equal(v.done.passed, false)
})

test("testView: GPU lines carry rates, no cores", () => {
  const ev = [{ event: "test-progress", kind: "gpu", t: 30, mode: "mixed", phase: "compute", iterations: 120, errors: 0, gflops: 9876.5, vram_gbps: null, adapter: "Intel Arc A580" }]
  const v = T.testView(ev, { kind: "gpu", seconds: 60, cores, tune: null })
  assert.deepEqual(v.cores, [])
  assert.equal(v.phase, "compute")
  assert.equal(v.core, null)
  assert.equal(v.pct, 50)
  assert.equal(v.left, 30)
  assert.equal(v.gpu.gflops, 9876.5)
  assert.equal(v.gpu.vram_gbps, null)
  assert.equal(v.rate, "9877 GFLOPS")
  const vram = T.testView([Object.assign({}, ev[0], { phase: "vram", gflops: null, vram_gbps: 412.3 })], { kind: "gpu", seconds: 60, cores, tune: null })
  assert.equal(vram.rate, "412 GB/s")
})

test("testView: no events yet but a test is running: elapsed from the status", () => {
  const v = T.testView([], { kind: "cpu", seconds: 120, cores, tune: null, elapsed: 60 })
  assert.equal(v.running, true)
  assert.equal(v.pct, 50)
  assert.equal(v.left, 60)
  assert.equal(v.phase, "")
  assert.deepEqual(v.cores.map(c => c.state), ["run", "run", "run", "run", "run", "run"])
  const cli = T.testView([], { kind: "cpu", seconds: null, cores, tune: null, elapsed: 60 })
  assert.equal(cli.pct, 0, "a CLI run has no known length")
  assert.equal(cli.left, null)
  const idle = T.testView([], { kind: "", seconds: null, cores, tune: null })
  assert.equal(idle.running, false)
  assert.deepEqual(idle.cores, [], "nothing to show before a test")
})

test("testView: CPU rate and markup in messages", () => {
  const v = T.testView(recorded, ctx)
  assert.equal(v.rate, "41 blocks/s")
  const msg = T.testView([Object.assign(progress(5, "all cores"), { message: "<b>x</b>" })], ctx)
  assert.equal(msg.message, "‹b›x‹/b›")
})

test("pushEvent keeps one line per phase, starts over after a done line, ignores other events", () => {
  let ev = []
  ev = T.pushEvent(ev, { event: "status" })
  assert.deepEqual(ev, [])
  ev = T.pushEvent(ev, progress(0, "reference"))
  ev = T.pushEvent(ev, progress(1, "all cores"))
  ev = T.pushEvent(ev, progress(2, "all cores"))
  assert.equal(ev.length, 2)
  assert.equal(ev[1].t, 2, "the later line replaces the earlier of the same phase")
  ev = T.pushEvent(ev, progress(31, "core 0"))
  assert.equal(ev.length, 3)
  ev = T.pushEvent(ev, { event: "test-done", kind: "cpu-per-core", passed: true, errors: 0 })
  assert.equal(ev.length, 4)
  const next = T.pushEvent(ev, progress(0, "reference", { kind: "cpu" }))
  assert.equal(next.length, 1, "a line after the done line is a new test")
  const other = T.pushEvent([progress(5, "all cores")], progress(0, "reference", { kind: "cpu" }))
  assert.equal(other.length, 1, "a line of another kind is a new test")
})

test("pushEvent stays bounded over a long GPU run in mixed mode", () => {
  const gpuLine = (t, phase) => ({ event: "test-progress", kind: "gpu", t, mode: "mixed", phase, iterations: t * 4, errors: 0, gflops: phase === "compute" ? 9876.5 : null, vram_gbps: phase === "vram" ? 412.3 : null, adapter: "Intel Arc A580" })
  let ev = []
  for (let i = 0; i < 1000; i++) ev = T.pushEvent(ev, gpuLine(i * 5, i % 2 === 0 ? "compute" : "vram"))
  assert.equal(ev.length, 1, "a GPU run keeps only its latest line")
  assert.equal(ev[0].t, 4995)
  const c = { kind: "gpu", seconds: 6000, cores, tune: null }
  const v = T.testView(ev, c)
  assert.equal(v.phase, "vram")
  assert.equal(v.rate, "412 GB/s")
  assert.equal(v.pct, 83)
  assert.equal(v.left, 1005)
  assert.equal(v.running, true)
  ev = T.pushEvent(ev, { event: "test-done", kind: "gpu", passed: true, errors: 0, interrupted: false, recorded: true, seconds: 6000, detail: "9877 GFLOPS, 412 GB/s" })
  assert.equal(ev.length, 2)
  assert.equal(T.testView(ev, c).done.passed, true)
})

test("pushEvent replaces a repeated CPU phase wherever it was, and caps the list", () => {
  let ev = [progress(0, "reference"), progress(30, "all cores"), progress(40, "core 0"), progress(50, "core 1")]
  ev = T.pushEvent(ev, progress(60, "core 0"))
  assert.deepEqual(ev.map(e => e.phase), ["reference", "all cores", "core 1", "core 0"], "the latest line is last")
  assert.equal(ev[3].t, 60)
  const v = T.testView(ev, ctx)
  assert.equal(v.phase, "core 0")
  assert.equal(v.cores[1].state, "ok", "core 1's phase is over")
  for (let i = 0; i < 500; i++) ev = T.pushEvent(ev, progress(100 + i, "phase " + i))
  assert.equal(ev.length, T.MAX_EVENTS)
  assert.equal(ev[ev.length - 1].phase, "phase 499", "the oldest lines go, the latest stays")
})

test("testHistory lists every recorded result, newest first", () => {
  const st = {
    profiles: [
      { name: "stock", cpu_test: null, gpu_test: null },
      { name: "custom", cpu_test: { time: 100, seconds: 600.4, passed: true, errors: 0, detail: "avx512, 24000 blocks" },
        gpu_test: { time: 300, seconds: 120, passed: false, errors: 2, detail: "<b>7.2</b> TFLOPS" } },
      { name: "max", cpu_test: { time: 200, seconds: 41.9, passed: false, errors: 3, detail: "stopped before the end" }, gpu_test: null },
    ],
  }
  const h = T.testHistory(st)
  assert.deepEqual(h.map(r => [r.profile, r.kind, r.time]), [["custom", "gpu", 300], ["max", "cpu", 200], ["custom", "cpu", 100]])
  assert.equal(h[0].label, "custom · GPU 2 min")
  assert.equal(h[0].detail, "‹b›7.2‹/b› TFLOPS", "markup neutralised")
  assert.equal(h[0].passed, false)
  assert.equal(h[0].errors, 2)
  assert.equal(h[1].label, "max · CPU 42 s")
  assert.equal(h[2].label, "custom · CPU 10 min")
  assert.deepEqual(T.testHistory(null), [])
  assert.deepEqual(T.testHistory({ profiles: [{ name: "x" }] }), [])
})

test("formatting helpers", () => {
  assert.equal(T.fmtClock(48.6), "0:49")
  assert.equal(T.fmtClock(192), "3:12")
  assert.equal(T.fmtClock(3600), "1:00:00")
  assert.equal(T.fmtClock(null), "—")
  assert.equal(T.fmtLength(120), "2 min")
  assert.equal(T.fmtLength(3600), "1 h")
  assert.equal(T.fmtLength(41.9), "42 s")
  assert.equal(T.fmtLength(null), "")
  assert.equal(T.fmtCo(-15), "−15")
  assert.equal(T.fmtCo(0), "0")
  assert.deepEqual(T.LENGTHS.map(l => l.value), ["120", "600", "1800", "3600"])
  assert.deepEqual(T.KINDS, ["cpu", "cpu-per-core", "gpu"])
})

test("the Test page is wired into the rail", () => {
  const M = require("../Model.js")
  assert.equal(M.pageSource("test"), "pages/Test.qml")
})

test("tileStates overlays the live results on the core tiles", () => {
  const tiles = [{ core: 0, tier: "fast", co: -10, override: false, test: "pass" }, { core: 2, tier: "dense", co: -20, override: true, test: "pass" }]
  const v = { cores: [{ core: 0, state: "ok", errors: 0 }, { core: 2, state: "bad", errors: 3 }] }
  const out = T.tileStates(tiles, v)
  assert.deepEqual(out.map(t => [t.test, t.errors]), [["pass", 0], ["fail", 3]])
  assert.equal(out[1].co, -20, "the rest of the tile is kept")
  assert.deepEqual(T.tileStates(tiles, { cores: [{ core: 0, state: "run", errors: 0 }] }).map(t => t.test), ["run", null], "no result yet: no verdict")
  assert.deepEqual(T.tileStates(tiles, { cores: [] }).map(t => t.test), [null, null])
})
