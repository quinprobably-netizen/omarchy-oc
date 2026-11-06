const test = require("node:test")
const assert = require("node:assert/strict")
const Mem = require("../lib/memory.js")
const strings = require("../i18n/en.json")

// info.json's mem block as this board reports it as a user: no SMBIOS, no
// SPD, no DIMM sensors.
const userInfo = {
  mem: {
    dimms: [], spd: [], temp_sensors: [],
    meminfo_total_mib: 15591, slots_populated: null, slots_total: 4,
    smbios_available: false, total_installed_mib: null,
    note: "Timings and frequency are set by the BIOS (EXPO/XMP); the OS can only monitor them.",
    summary: "15591 MiB total (per /proc/meminfo), 4 slots (population unknown without root)",
  },
  gpu: { vram_bar_gib: 8.0 },
}

// As root: two of four slots populated (SMBIOS type 17), one SPD module.
const rootInfo = {
  mem: {
    smbios_available: true, slots_total: 4, slots_populated: 2, total_installed_mib: 32768,
    note: userInfo.mem.note,
    summary: "2 x 16 GiB DDR5, rated 6000 MT/s, running 4800 MT/s, 2/4 slots populated",
    dimms: [
      { handle: 1, locator: "DIMM_A1", bank_locator: "BANK 0", populated: false, size_mib: null,
        memory_type: "Unknown", speed_mts: null, configured_speed_mts: null, manufacturer: "", part_number: "" },
      { handle: 2, locator: "DIMM_A2", bank_locator: "BANK 1", populated: true, size_mib: 16384,
        memory_type: "DDR5", speed_mts: 6000, configured_speed_mts: 4800, manufacturer: "Kingston",
        part_number: "KF560C36-16", ranks: 1, configured_voltage_mv: 1100 },
      { handle: 3, locator: "DIMM_B1", bank_locator: "BANK 2", populated: false, size_mib: null,
        memory_type: "Unknown", speed_mts: null, configured_speed_mts: null, manufacturer: "", part_number: "" },
      { handle: 4, locator: "DIMM_B2", bank_locator: "BANK 3", populated: true, size_mib: 16384,
        memory_type: "DDR5", speed_mts: 6000, configured_speed_mts: 4800, manufacturer: "<b>Kingston</b>",
        part_number: "KF560C36-16", ranks: 1, configured_voltage_mv: 1100 },
    ],
    spd: [
      { slot: 1, dram_type: "DDR5 SDRAM", part_number: "KF560C36BBEK2-16", jedec_speed_mts: 4800, size_mib: 16384,
        module_manufacturer: "Kingston", cas_latencies: [40, 36],
        expo: { profiles: [{ name: "EXPO 1", speed_mts: 6000 }, { name: "EXPO 2", speed_mts: 5600 }] },
        xmp: { profiles: [] } },
    ],
    temp_sensors: [],
  },
}

const status = {
  mem: { reading: { used_pct: 76.7, used_mib: 11963, total_mib: 15591, swap_used_mib: 4204, swap_total_mib: 31182 } },
  gpu: { reading: { vram_used_mib: 7733 } },
}

test("dimmRows: empty without SMBIOS, one row per slot with SMBIOS", () => {
  assert.deepEqual(Mem.dimmRows(userInfo), [])
  assert.deepEqual(Mem.dimmRows(null), [])
  const rows = Mem.dimmRows(rootInfo)
  assert.equal(rows.length, 4)
  assert.deepEqual(rows[0], { slot: "DIMM_A1", populated: false, size: "—", type: "", speed: "", part: "", vendor: "" })
  assert.deepEqual(rows[1], {
    slot: "DIMM_A2", populated: true, size: "16 GiB", type: "DDR5",
    speed: "4800 / 6000 MT/s", part: "KF560C36-16", vendor: "Kingston",
  })
  assert.equal(rows[3].vendor, "‹b›Kingston‹/b›")
})

test("dimmHint: the Run-as-root note only without SMBIOS", () => {
  assert.equal(Mem.dimmHint(userInfo), "memory.dimms.root")
  assert.equal(Mem.dimmHint(null), "memory.dimms.root")
  assert.equal(Mem.dimmHint(rootInfo), "")
  assert.equal(Mem.dimmHint({ mem: { smbios_available: true, dimms: [] } }), "memory.dimms.none")
  assert.match(strings["memory.dimms.root"], /Run as root/)
  assert.ok(strings["memory.dimms.none"])
})

test("spdRows: part, type, JEDEC and EXPO/XMP speeds", () => {
  assert.deepEqual(Mem.spdRows(userInfo), [])
  const rows = Mem.spdRows(rootInfo)
  assert.equal(rows.length, 1)
  assert.deepEqual(rows[0], {
    slot: "1", part: "KF560C36BBEK2-16", type: "DDR5 SDRAM", vendor: "Kingston",
    jedec: "JEDEC 4800 MT/s", profiles: "EXPO 6000 / 5600 MT/s",
  })
})

test("memNote and memSummary pass the daemon's text through plain()", () => {
  assert.equal(Mem.memNote(userInfo), userInfo.mem.note)
  assert.equal(Mem.memNote({ mem: { note: "<x>" } }), "‹x›")
  assert.equal(Mem.memNote(null), "")
  assert.equal(Mem.memSummary(userInfo), userInfo.mem.summary)
  assert.equal(Mem.memSummary(null), "")
})

test("memCaption: short enough for the header, from SMBIOS when present", () => {
  assert.equal(Mem.memCaption(userInfo), "15.2 GiB · 4 slots")
  assert.equal(Mem.memCaption(rootInfo), "32 GiB · 2/4 slots · DDR5 4800 / 6000 MT/s")
  assert.equal(Mem.memCaption(null), "")
  assert.equal(Mem.memCaption({ mem: { meminfo_total_mib: 1024, slots_total: 0 } }), "1 GiB")
})

test("usageRows: RAM, swap and VRAM with fractions and text", () => {
  const u = Mem.usageRows(status, userInfo)
  assert.equal(u.length, 3)
  assert.deepEqual(u[0], { key: "ram", frac: 0.767, value: "77 %", text: "11.7 / 15.2 GiB" })
  assert.deepEqual(u[1], { key: "swap", frac: 4204 / 31182, value: "13 %", text: "4.1 / 30.5 GiB" })
  assert.deepEqual(u[2], { key: "vram", frac: 7733 / 8192, value: "94 %", text: "7.6 / 8.0 GiB" })
  // Without the VRAM size, the bar has no fraction but the text keeps the used figure.
  const noInfo = Mem.usageRows(status, null)
  assert.deepEqual(noInfo[2], { key: "vram", frac: null, value: "—", text: "7.6 GiB" })
  // No swap: the row is dropped. Nothing at all: RAM only, empty.
  const noSwap = Mem.usageRows({ mem: { reading: { used_pct: 10, used_mib: 1024, total_mib: 10240, swap_total_mib: 0 } } }, null)
  assert.deepEqual(noSwap.map(r => r.key), ["ram"])
  assert.deepEqual(Mem.usageRows(null, null).map(r => r.key), ["ram"])
  assert.deepEqual(Mem.usageRows(null, null)[0], { key: "ram", frac: null, value: "—", text: "—" })
})
