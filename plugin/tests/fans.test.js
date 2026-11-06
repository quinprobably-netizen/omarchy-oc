const test = require("node:test")
const assert = require("node:assert/strict")
const F = require("../lib/fans.js")
const M = require("../Model.js")

// A trimmed omarchy-fand status (what the `fans` proxy's `status` returns).
const status = {
  time: 1000, mode: "curve", profile: "balanced",
  profiles: [{ id: "silent", label: "Silent" }, { id: "balanced", label: "Balanced" }, { id: "performance", label: "Performance" }],
  curves: {
    cpu: { cpu: [[40, 10], [85, 100]] },
    case: { cpu: [[45, 10], [88, 100]], gpu: [[60, 10], [95, 100]], vram: [[70, 0], [100, 100]] },
  },
  sensors: [
    { id: "cpu", label: "CPU", temp: 45.2, held: 45, critical: 90, ok: true },
    { id: "gpu", label: "GPU", temp: 72, held: 72, critical: 97, ok: true },
  ],
  fans: [
    { id: "it8689/pwm1", label: "CPU fan", role: "cpu", connected: true, calibrated: true, rpm: 812, percent: 35, stalled: false },
    { id: "it8689/pwm2", label: "System fan 1", role: "case", connected: true, seen_spinning: true, rpm: 640, percent: 30, stalled: false },
    { id: "it8689/pwm4", label: "System fan 3", role: "case", connected: false, rpm: 0, stalled: false },
  ],
  calibration: null,
  message: "",
}

test("the fans page is wired into the rail", () => {
  assert.equal(M.pageSource("fans"), "pages/Fans.qml")
})

test("curve tabs skip sensors this machine lacks and roles without fans", () => {
  const tabs = F.curveTabs(status)
  assert.deepEqual(tabs.map(t => t.key), ["cpu:cpu", "case:cpu", "case:gpu"])
  assert.equal(tabs[2].label, "Case ← GPU")
  const noCase = { ...status, fans: [status.fans[0]] }
  assert.deepEqual(F.curveTabs(noCase).map(t => t.key), ["cpu:cpu"])
  assert.equal(F.findTab(tabs, "missing").key, "cpu:cpu")
  assert.equal(F.findTab(tabs, "case:gpu").key, "case:gpu")
  assert.equal(F.findTab([], "x"), null)
  assert.deepEqual(F.curveTabs(null), [])
})

test("movePoint keeps curves monotonic", () => {
  const pts = [[40, 10], [60, 30], [80, 100]]
  assert.deepEqual(F.movePoint(pts, 1, 90, 5), [[40, 10], [79, 10], [80, 100]])
  assert.deepEqual(F.movePoint(pts, 1, 20, 200), [[40, 10], [41, 100], [80, 100]])
  assert.deepEqual(F.movePoint(pts, 0, -10, -10), [[0, 0], [60, 30], [80, 100]])
  assert.deepEqual(pts, [[40, 10], [60, 30], [80, 100]], "input untouched")
})

test("add/remove points", () => {
  const pts = [[40, 10], [80, 100]]
  assert.deepEqual(F.addPoint(pts, 60.4, 5), [[40, 10], [60, 10], [80, 100]])
  assert.deepEqual(F.addPoint(pts, 40, 50), pts, "no duplicate temps")
  assert.deepEqual(F.removePoint(pts, 0), pts, "keeps two points")
  assert.deepEqual(F.removePoint([[1, 1], [2, 2], [3, 3]], 1), [[1, 1], [3, 3]])
  assert.equal(F.pointsArg(pts), "40:10,80:100")
})

test("curve request carries integer points for omarchy-fand", () => {
  const tab = F.findTab(F.curveTabs(status), "case:gpu")
  const req = F.curveRequest(tab, [[60.4, 10.2], [95, 100]])
  assert.deepEqual(req, { cmd: "curve", role: "case", sensor: "gpu", points: [[60, 10], [95, 100]] })
  assert.equal(F.curveRequest(null, [[1, 1]]), null)
})

test("profile options are short enough for one segmented row", () => {
  assert.deepEqual(F.profileOptions(status), [
    { value: "silent", label: "Silent" },
    { value: "balanced", label: "Balanced" },
    { value: "performance", label: "Perf" },
  ])
  assert.deepEqual(F.profileOptions(null), [])
})

test("fan rows take live rpm and percent from the pushed oc status", () => {
  const oc = { fans: { available: true, fans: [{ id: "it8689/pwm1", label: "CPU fan", role: "cpu", rpm: 900, percent: 42 }] } }
  const rows = F.fanRows(status, oc)
  assert.deepEqual(rows.map(r => r.id), ["it8689/pwm1", "it8689/pwm2"])
  assert.equal(rows[0].rpm, 900)
  assert.equal(rows[0].percent, 42)
  assert.equal(rows[1].rpm, 640, "a fan the oc status lacks keeps omarchy-fand's reading")
  assert.equal(rows[1].percent, 30)
  assert.deepEqual(F.fanRows(status, null).map(r => r.rpm), [812, 640])
  assert.deepEqual(F.fanRows(null, oc), [])
})

test("rpm and percent text", () => {
  assert.equal(F.rpmText({ rpm: 812 }), "812 rpm")
  assert.equal(F.rpmText({ rpm: null }), "—")
  assert.equal(F.rpmText({ rpm: 0, stalled: true }), "STALLED")
  assert.equal(F.percentText({ percent: 35.3 }), "35 %")
  assert.equal(F.percentText({ percent: null }), "")
  assert.equal(F.speedShare({ percent: 35 }), 0.35)
  assert.equal(F.speedShare({ percent: null, rpm: 500, max_rpm: 1000 }), 0.5)
  assert.equal(F.speedShare({ percent: null, rpm: 500 }), 0)
})

test("labels", () => {
  assert.equal(F.modeLabel(status, true), "Curve")
  assert.equal(F.modeLabel({ ...status, mode: "auto" }, true), "BIOS")
  assert.equal(F.modeLabel({ ...status, calibration: { progress: 0.42 } }, true), "Calibrating 42%")
  assert.equal(F.modeLabel(null, true), "Offline")
  assert.equal(F.profileLabel(status), "Balanced")
  assert.equal(F.profileLabel({ ...status, profile: "custom" }), "custom")
  assert.equal(F.formatTemp(45.2), "45°")
  assert.equal(F.formatTemp(NaN), "—")
  assert.equal(F.sensorTemp(status, "cpu"), 45.2)
  assert.ok(Number.isNaN(F.sensorTemp(status, "nvme")))
  assert.equal(F.hasCritical(status), false)
  assert.equal(F.hasCritical({ ...status, sensors: [{ ...status.sensors[0], temp: 91 }] }), true)
  assert.equal(F.hasStall(status), false)
  assert.equal(F.cpuFanRpm(status), 812)
})

test("header caption and calibration text", () => {
  assert.equal(F.caption(status, true), "Balanced · Curve · omarchy-fand")
  assert.equal(F.caption(null, true), "Offline")
  assert.equal(F.calibrationText(status), "")
  assert.equal(F.calibrationText({ ...status, calibration: { kind: "calibrate", step: "CPU fan: finding start", progress: 0.3 } }), "CPU fan: finding start · 30%")
  assert.equal(F.calibrationText({ ...status, calibration: { kind: "calibrate", progress: 0.3 } }), "Calibrating 30%")
  assert.equal(F.calibrationText({ ...status, calibration: { kind: "retest", progress: 0.5 } }), "Re-testing 50%")
})

test("never-spinning uncalibrated headers are hidden", () => {
  const st = { ...status, fans: [...status.fans, { id: "it8689/pwm5", label: "x", role: "case", connected: true, calibrated: false, seen_spinning: false, rpm: 0 }] }
  assert.deepEqual(F.connectedFans(st).map(f => f.id), ["it8689/pwm1", "it8689/pwm2"])
})

test("PWM-deaf fans stay listed (they still report RPM)", () => {
  const st = { ...status, fans: [{ id: "it8689/pwm2", label: "S", role: "case", connected: true, calibrated: true, responsive: false, rpm: 1149 }] }
  assert.equal(F.connectedFans(st).length, 1)
})

test("PWM-deaf fans get a tag, a fix hint and a re-test state", () => {
  const deaf = { id: "it/pwm2", label: "System fan 1", connected: true, responsive: false, calibrated: false }
  const ok = { id: "it/pwm3", connected: true, responsive: true, calibrated: true }
  const empty = { id: "it/pwm4", connected: false, responsive: true, calibrated: false }
  assert.equal(F.ignoresPwm(deaf), true)
  assert.equal(F.ignoresPwm(ok), false)
  assert.equal(F.ignoresPwm(empty), false)
  assert.equal(F.fanTag(deaf), "ignores PWM · BIOS")
  assert.equal(F.fanTag(ok), "")
  assert.equal(F.fanTag({ connected: true, calibrated: false }), "uncalibrated")
  assert.match(F.fanHint(deaf), /Voltage mode in the BIOS/)
  assert.equal(F.fanHint(ok), "")
  const st = { calibration: { kind: "retest", progress: 0.5, fans: ["it/pwm2"] } }
  assert.equal(F.retestRunning(st, "it/pwm2"), true)
  assert.equal(F.retestRunning(st, "it/pwm3"), false)
  assert.equal(F.retestRunning({ calibration: { kind: "calibrate", fans: ["it/pwm2"] } }, "it/pwm2"), false)
  assert.equal(F.modeLabel({ ...st, time: 1 }, true), "Re-testing 50%")
  assert.equal(F.modeLabel({ calibration: { progress: 0.25 } }, true), "Calibrating 25%")
})

test("GPU fan is listed at rest and gets its own curve tab", () => {
  const st = {
    curves: { cpu: { cpu: [[40, 10], [85, 100]] }, gpu: { gpu: [[55, 0], [93, 100]], vram: [[80, 0], [100, 100]] } },
    sensors: [{ id: "cpu", label: "CPU" }, { id: "gpu", label: "GPU" }, { id: "vram", label: "VRAM" }],
    fans: [
      { id: "it/pwm1", role: "cpu", connected: true, calibrated: true, rpm: 800 },
      { id: "xe/gpu", kind: "gpu", role: "gpu", label: "GPU fan", connected: true, calibrated: false, rpm: 0 },
    ],
  }
  assert.deepEqual(F.connectedFans(st).map(f => f.id), ["it/pwm1", "xe/gpu"])
  assert.deepEqual(F.curveTabs(st).map(t => t.label), ["CPU fan", "GPU fan", "GPU ← VRAM"])
  assert.equal(F.fanTag(st.fans[1]), "", "the name already says GPU")
})

test("fan labels from the daemon are shown as plain text, markup neutralised", () => {
  const st = { ...status, fans: [{ ...status.fans[0], label: "<img src=x>" }] }
  assert.equal(F.fanRows(st, null)[0].label, "‹img src=x›")
  assert.equal(F.curveTabs({ ...st, sensors: [{ id: "cpu", label: "<b>CPU</b>" }] })[0].label, "CPU fan")
})
