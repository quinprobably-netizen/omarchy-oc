import QtQuick

// A filled line over the last N samples. Gaps (NaN) break the line.
Canvas {
  id: canvas

  property var values: []
  property color color: "white"

  onValuesChanged: requestPaint()
  onColorChanged: requestPaint()
  onWidthChanged: requestPaint()
  onHeightChanged: requestPaint()

  onPaint: {
    var ctx = getContext("2d")
    ctx.reset()
    var vals = values || []
    var n = vals.length
    if (n < 2 || width <= 0 || height <= 0) return
    var lo = Infinity, hi = -Infinity, finite = 0
    for (var i = 0; i < n; i++) {
      var v = Number(vals[i])
      if (!isFinite(v)) continue
      finite++
      if (v < lo) lo = v
      if (v > hi) hi = v
    }
    if (finite < 2) return
    if (hi - lo < 1e-9) { lo -= 1; hi += 1 }
    var pad = 1.5
    var span = hi - lo
    var xOf = function(i) { return i / (n - 1) * width }
    var yOf = function(v) { return height - pad - (v - lo) / span * (height - pad * 2) }

    // Fill each unbroken run down to the baseline, then stroke the line.
    ctx.fillStyle = Qt.rgba(color.r, color.g, color.b, 0.18)
    ctx.strokeStyle = color
    ctx.lineWidth = 1.5
    ctx.lineJoin = "round"
    var runStart = -1
    for (var j = 0; j <= n; j++) {
      var ok = j < n && isFinite(Number(vals[j]))
      if (ok && runStart < 0) runStart = j
      if (!ok && runStart >= 0) {
        var end = j - 1
        if (end > runStart) {
          ctx.beginPath()
          ctx.moveTo(xOf(runStart), height)
          for (var k = runStart; k <= end; k++) ctx.lineTo(xOf(k), yOf(Number(vals[k])))
          ctx.lineTo(xOf(end), height)
          ctx.closePath()
          ctx.fill()
          ctx.beginPath()
          ctx.moveTo(xOf(runStart), yOf(Number(vals[runStart])))
          for (var m = runStart + 1; m <= end; m++) ctx.lineTo(xOf(m), yOf(Number(vals[m])))
          ctx.stroke()
        }
        runStart = -1
      }
    }
  }
}
