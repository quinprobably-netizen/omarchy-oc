import QtQuick
import qs.Commons
import qs.Commons as Commons

// Several series in one box, each line on its own scale so a clock in MHz,
// a temperature and a power draw can share the chart. A legend swatch per
// series sits in the top-left corner.
//
//   series   [{values, color, label, min, max}]: `min`/`max` fix the
//            series' scale; a missing one falls back to minY/maxY, then to
//            the data. Gaps (NaN) break the line.
//   minY     the chart's default scale (null: from the data)
//   maxY
Rectangle {
  id: chart

  property var series: []
  property var minY: null
  property var maxY: null
  property color foreground: Commons.Color.foreground
  property string fontFamily: Style.font.family
  property int gridLines: 3

  readonly property color dim: Qt.darker(foreground, 1.4)

  implicitHeight: Style.space(96)
  radius: Style.cornerRadius
  color: Qt.rgba(foreground.r, foreground.g, foreground.b, 0.06)
  clip: true

  Canvas {
    id: canvas
    anchors.fill: parent
    anchors.margins: Style.space(1)

    readonly property var list: chart.series || []
    readonly property real padY: Style.space(4)

    onListChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()
    Connections {
      target: chart
      function onForegroundChanged() { canvas.requestPaint() }
      function onMinYChanged() { canvas.requestPaint() }
      function onMaxYChanged() { canvas.requestPaint() }
    }

    function scaleOf(s) {
      var lo = finiteOr(s.min, finiteOr(chart.minY, Infinity))
      var hi = finiteOr(s.max, finiteOr(chart.maxY, -Infinity))
      var vals = s.values || []
      if (lo === Infinity || hi === -Infinity) {
        for (var i = 0; i < vals.length; i++) {
          var v = Number(vals[i])
          if (!isFinite(v)) continue
          if (lo === Infinity && v < lo) lo = v
          if (hi === -Infinity && v > hi) hi = v
        }
        // Only one bound came from the data: re-run against the other.
        if (lo === Infinity) lo = hi
        if (hi === -Infinity) hi = lo
      }
      if (!isFinite(lo) || !isFinite(hi)) return null
      if (hi - lo < 1e-9) { lo -= 1; hi += 1 }
      return { lo: lo, hi: hi }
    }

    function finiteOr(v, fallback) {
      var n = Number(v)
      return v !== null && v !== undefined && v !== "" && isFinite(n) ? n : fallback
    }

    onPaint: {
      var ctx = getContext("2d")
      ctx.reset()
      if (width <= 0 || height <= 0) return
      var fg = chart.foreground

      // Grid.
      ctx.strokeStyle = Qt.rgba(fg.r, fg.g, fg.b, 0.08)
      ctx.lineWidth = 1
      for (var g = 1; g <= chart.gridLines; g++) {
        var gy = Math.round(height * g / (chart.gridLines + 1)) + 0.5
        ctx.beginPath()
        ctx.moveTo(0, gy)
        ctx.lineTo(width, gy)
        ctx.stroke()
      }

      var list = canvas.list
      var n = 0
      for (var a = 0; a < list.length; a++) n = Math.max(n, (list[a].values || []).length)
      if (n < 2) return

      for (var si = 0; si < list.length; si++) {
        var s = list[si]
        var vals = s.values || []
        var sc = scaleOf(s)
        if (sc === null) continue
        var span = sc.hi - sc.lo
        var color = s.color || fg
        // Every series spans the same time axis: index i is sample i of n.
        var xOf = function(i) { return i / (n - 1) * width }
        var yOf = function(v) {
          var f = Math.max(0, Math.min(1, (v - sc.lo) / span))
          return height - padY - f * (height - padY * 2)
        }

        ctx.strokeStyle = color
        ctx.lineWidth = si === 0 ? 2 : 1.5
        ctx.lineJoin = "round"
        if (si === 0) ctx.fillStyle = Qt.rgba(Qt.color(color).r, Qt.color(color).g, Qt.color(color).b, 0.12)

        var runStart = -1
        for (var j = 0; j <= vals.length; j++) {
          var ok = j < vals.length && isFinite(Number(vals[j]))
          if (ok && runStart < 0) runStart = j
          if (!ok && runStart >= 0) {
            var end = j - 1
            if (end > runStart) {
              if (si === 0) {
                ctx.beginPath()
                ctx.moveTo(xOf(runStart), height)
                for (var k = runStart; k <= end; k++) ctx.lineTo(xOf(k), yOf(Number(vals[k])))
                ctx.lineTo(xOf(end), height)
                ctx.closePath()
                ctx.fill()
              }
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
  }

  // Legend: one swatch and label per series, on a pill so it stays
  // readable where a line runs under it.
  Rectangle {
    anchors.left: parent.left
    anchors.top: parent.top
    anchors.margins: Style.space(4)
    width: legend.implicitWidth + Style.space(8)
    height: legend.implicitHeight + Style.space(4)
    radius: height / 2
    color: Qt.rgba(Commons.Color.popups.background.r, Commons.Color.popups.background.g, Commons.Color.popups.background.b, 0.75)
  }

  Row {
    id: legend
    anchors.left: parent.left
    anchors.top: parent.top
    anchors.leftMargin: Style.space(8)
    anchors.topMargin: Style.space(6)
    spacing: Style.space(8)

    Repeater {
      model: chart.series || []

      Row {
        required property var modelData
        spacing: Style.space(4)

        Rectangle {
          anchors.verticalCenter: parent.verticalCenter
          width: Style.space(7)
          height: width
          radius: Math.max(1, Style.space(1))
          color: parent.modelData.color || chart.foreground
        }
        Text {
          anchors.verticalCenter: parent.verticalCenter
          textFormat: Text.PlainText
          text: String(parent.modelData.label || "")
          color: chart.dim
          font.family: chart.fontFamily
          font.pixelSize: Style.font.caption
        }
      }
    }
  }
}
