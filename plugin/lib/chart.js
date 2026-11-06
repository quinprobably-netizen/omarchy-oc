// Pure helpers for components/Chart.qml. No QML types here so
// tests/chart.test.js can load this file under node.

function finite(v) {
  if (v === null || v === undefined || v === "") return null
  var n = Number(v)
  return isFinite(n) ? n : null
}

// The y range a series is drawn on: its own `min`/`max`, else the chart's
// `minY`/`maxY`, else the real minimum and maximum of its finite samples.
// Flat data gets a one-unit band each way; nothing to scale by gives null.
function scaleOf(s, minY, maxY) {
  var lo = finite(s ? s.min : null)
  if (lo === null) lo = finite(minY)
  var hi = finite(s ? s.max : null)
  if (hi === null) hi = finite(maxY)
  if (lo === null || hi === null) {
    var vals = s && s.values ? s.values : []
    var dataLo = Infinity, dataHi = -Infinity
    for (var i = 0; i < vals.length; i++) {
      var v = finite(vals[i])
      if (v === null) continue
      if (v < dataLo) dataLo = v
      if (v > dataHi) dataHi = v
    }
    if (lo === null) lo = dataLo
    if (hi === null) hi = dataHi
  }
  if (!isFinite(lo) || !isFinite(hi)) return null
  if (hi - lo < 1e-9) { lo -= 1; hi += 1 }
  return { lo: lo, hi: hi }
}

// Every series spans the same time axis: index i of n samples.
function sampleCount(list) {
  var n = 0
  for (var i = 0; i < (list || []).length; i++) {
    var vals = list[i] && list[i].values ? list[i].values : []
    if (vals.length > n) n = vals.length
  }
  return n
}

if (typeof module !== "undefined") {
  module.exports = { scaleOf: scaleOf, sampleCount: sampleCount }
}
