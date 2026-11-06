// Pure helpers for the Memory page. No QML types here so
// tests/memory.test.js can load this file under node. Self-contained (no
// .import) so the same file works from QML and node.

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

function plain(s) {
  if (s === null || s === undefined) return ""
  return String(s).replace(/</g, "‹").replace(/>/g, "›")
}

function gib(mib) {
  var n = num(mib)
  return n === null ? "—" : (n / 1024).toFixed(1)
}

// "16 GiB" for a whole number of GiB, "7.5 GiB" otherwise, "512 MiB" below one.
function fmtSize(mib) {
  var n = num(mib)
  if (n === null) return "—"
  if (n < 1024) return Math.round(n) + " MiB"
  var g = n / 1024
  return (g % 1 === 0 ? g : g.toFixed(1)) + " GiB"
}

function mem(info) { return info && info.mem ? info.mem : null }

// ---- DIMM table (SMBIOS type 17, root only) --------------------------------

// One row per slot. Empty slots keep their locator and nothing else;
// populated ones show size, type, "running / rated MT/s", part and vendor.
// Every string is firmware text and goes through plain().
function dimmRows(info) {
  var m = mem(info)
  var dimms = m && m.dimms ? m.dimms : []
  return dimms.map(function(d) {
    if (!d.populated) return { slot: plain(d.locator), populated: false, size: "—", type: "", speed: "", part: "", vendor: "" }
    var rated = num(d.speed_mts), running = num(d.configured_speed_mts)
    var speed = ""
    if (running !== null && rated !== null) speed = running + " / " + rated + " MT/s"
    else if (rated !== null) speed = rated + " MT/s"
    else if (running !== null) speed = running + " MT/s"
    return {
      slot: plain(d.locator),
      populated: true,
      size: fmtSize(d.size_mib),
      type: plain(d.memory_type),
      speed: speed,
      part: plain(d.part_number),
      vendor: plain(d.manufacturer)
    }
  })
}

// The i18n key of the line under the (missing) DIMM table: the root hint
// without SMBIOS, "no modules" with SMBIOS but no entries, nothing otherwise.
function dimmHint(info) {
  var m = mem(info)
  if (!m || !m.smbios_available) return "memory.dimms.root"
  if (!m.dimms || m.dimms.length === 0) return "memory.dimms.none"
  return ""
}

// ---- SPD (EXPO / XMP) -----------------------------------------------------

function profileSpeeds(block) {
  var list = block && block.profiles ? block.profiles : []
  return list.map(function(p) { return num(p.speed_mts) }).filter(function(n) { return n !== null })
}

function spdRows(info) {
  var m = mem(info)
  var spd = m && m.spd ? m.spd : []
  return spd.map(function(s) {
    var expo = profileSpeeds(s.expo), xmp = profileSpeeds(s.xmp)
    var profiles = []
    if (expo.length > 0) profiles.push("EXPO " + expo.join(" / ") + " MT/s")
    if (xmp.length > 0) profiles.push("XMP " + xmp.join(" / ") + " MT/s")
    var jedec = num(s.jedec_speed_mts)
    return {
      slot: s.slot === null || s.slot === undefined ? "" : String(s.slot),
      part: plain(s.part_number),
      type: plain(s.dram_type),
      vendor: plain(s.module_manufacturer),
      jedec: jedec !== null ? "JEDEC " + jedec + " MT/s" : "",
      profiles: profiles.join(" · ")
    }
  })
}

// ---- text -----------------------------------------------------------------

// "32 GiB · 2/4 slots · DDR5 4800 / 6000 MT/s": the header caption. The
// daemon's `summary` says the same at a length the header cannot hold.
function memCaption(info) {
  var m = mem(info)
  if (!m) return ""
  var parts = []
  var total = num(m.total_installed_mib)
  if (total === null) total = num(m.meminfo_total_mib)
  if (total !== null) parts.push(fmtSize(total))
  var slots = num(m.slots_total), populated = num(m.slots_populated)
  if (slots !== null && slots > 0) parts.push((populated !== null ? populated + "/" : "") + slots + " slots")
  var first = null
  for (var i = 0; i < (m.dimms || []).length; i++) if (m.dimms[i].populated) { first = m.dimms[i]; break }
  if (first) {
    var kind = plain(first.memory_type)
    var row = dimmRows({ mem: { dimms: [first] } })[0]
    if (row.speed !== "") kind = (kind + " " + row.speed).trim()
    if (kind !== "") parts.push(kind)
  }
  return parts.join(" · ")
}

function memNote(info) { var m = mem(info); return m ? plain(m.note) : "" }
function memSummary(info) { var m = mem(info); return m ? plain(m.summary) : "" }

// ---- usage ----------------------------------------------------------------

function pct(frac) { return frac === null ? "—" : Math.round(frac * 100) + " %" }

// RAM, swap (when there is any) and VRAM (its size from info.json's GPU
// block; without it the bar is empty and only the used figure shows).
function usageRows(st, info) {
  var r = st && st.mem && st.mem.reading ? st.mem.reading : null
  var g = st && st.gpu && st.gpu.reading ? st.gpu.reading : null
  var rows = []

  var used = r ? num(r.used_mib) : null, total = r ? num(r.total_mib) : null
  var ramFrac = r && num(r.used_pct) !== null ? num(r.used_pct) / 100 : null
  rows.push({
    key: "ram",
    frac: ramFrac,
    value: pct(ramFrac),
    text: used !== null && total !== null ? gib(used) + " / " + gib(total) + " GiB" : "—"
  })

  var swapTotal = r ? num(r.swap_total_mib) : null, swapUsed = r ? num(r.swap_used_mib) : null
  if (swapTotal !== null && swapTotal > 0) {
    var swapFrac = swapUsed !== null ? swapUsed / swapTotal : null
    rows.push({ key: "swap", frac: swapFrac, value: pct(swapFrac), text: gib(swapUsed) + " / " + gib(swapTotal) + " GiB" })
  }

  var vramUsed = g ? num(g.vram_used_mib) : null
  if (vramUsed !== null) {
    var barGib = info && info.gpu ? num(info.gpu.vram_bar_gib) : null
    var vramFrac = barGib !== null && barGib > 0 ? vramUsed / (barGib * 1024) : null
    rows.push({
      key: "vram",
      frac: vramFrac,
      value: pct(vramFrac),
      text: gib(vramUsed) + (barGib !== null ? " / " + barGib.toFixed(1) : "") + " GiB"
    })
  }
  return rows
}

if (typeof module !== "undefined") {
  module.exports = {
    dimmRows: dimmRows, dimmHint: dimmHint, spdRows: spdRows, memNote: memNote, memSummary: memSummary, memCaption: memCaption,
    usageRows: usageRows, fmtSize: fmtSize
  }
}
