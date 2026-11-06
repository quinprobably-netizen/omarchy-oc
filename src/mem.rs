//! Memory (DRAM) monitoring and information.
//!
//! DDR5 timings and frequency cannot be changed from the OS on this platform
//! (BIOS / EXPO / XMP only), so this module is read-only:
//!
//! * live usage from `/proc/meminfo`;
//! * DIMM temperatures from `spd5118` (DDR5 SPD hub) and `jc42` (DDR4) hwmon
//!   devices, mapped to slots via the i2c client address;
//! * SMBIOS type 17 (Memory Device) records from
//!   `/sys/firmware/dmi/entries/17-*/raw` (root only; skipped if unreadable);
//! * the DDR5 SPD EEPROM when the `spd5118` driver exposes it through nvmem,
//!   including XMP 3.0 and AMD EXPO profile blocks.
//!
//! Every byte access is bounds-checked; malformed data yields missing fields,
//! never a panic. All filesystem access goes through [`Sys`] so the tests can
//! run against a fake tree.

use crate::util::{self, Sys};
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

/// Shown on the info page: the OS cannot tune memory on this platform.
pub const NOTE: &str =
    "Timings and frequency are set by the BIOS (EXPO/XMP); the OS can only monitor them.";

// ---------------------------------------------------------------------------
// Public type
// ---------------------------------------------------------------------------

pub struct Mem {
    sys: Sys,
    /// Number of SMBIOS type-17 entries (slots), known even when `raw` is unreadable.
    slots_total: usize,
    smbios_readable: bool,
    dimms: Vec<SmbiosDimm>,
    spd: Vec<SpdModule>,
    sensors: Vec<TempSensor>,
    live: MemInfo,
}

impl Mem {
    /// Gather static information. Reads a handful of small files; call once at start.
    pub fn detect(sys: &Sys) -> Mem {
        let entries: Vec<PathBuf> = sys
            .list("/sys/firmware/dmi/entries")
            .into_iter()
            .filter(|p| file_name(p).starts_with("17-"))
            .collect();
        let mut dimms = Vec::new();
        let mut smbios_readable = false;
        for e in &entries {
            if let Ok(raw) = fs::read(e.join("raw")) {
                smbios_readable = true;
                if let Some(d) = parse_type17(&raw) {
                    dimms.push(d);
                }
            }
        }

        let sensors = find_sensors(sys);
        let spd = sensors
            .iter()
            .filter(|s| s.driver == "spd5118")
            .filter_map(|s| {
                let bytes = read_spd_eeprom(sys, s.client.as_deref()?)?;
                parse_spd(&bytes, s.slot, s.i2c_addr)
            })
            .collect();

        let mut m = Mem {
            sys: sys.clone(),
            slots_total: entries.len(),
            smbios_readable,
            dimms,
            spd,
            sensors,
            live: MemInfo::default(),
        };
        m.sample();
        m
    }

    /// Refresh live readings: one read of /proc/meminfo plus one per DIMM sensor.
    pub fn sample(&mut self) {
        if let Some(s) = self.sys.read("/proc/meminfo") {
            self.live = parse_meminfo(&s);
        }
        for s in &mut self.sensors {
            s.temp = util::read_num::<i64>(&s.input).map(|m| util::round1(m as f64 / 1000.0));
        }
    }

    /// Static information for the UI / info page.
    pub fn info(&self) -> Value {
        let populated: Vec<&SmbiosDimm> = self.dimms.iter().filter(|d| d.populated).collect();
        // Without SMBIOS the best hint is one SPD sensor per DIMM; with
        // neither, population is unknown (null).
        let slots_populated = if self.smbios_readable {
            Some(populated.len())
        } else if !self.sensors.is_empty() {
            Some(self.sensors.len())
        } else {
            None
        };

        let smbios_total: u64 = populated.iter().filter_map(|d| d.size_mib).sum();
        let spd_total: u64 = self.spd.iter().filter_map(|m| m.size_mib).sum();
        let total_installed_mib = if smbios_total > 0 {
            Some(smbios_total)
        } else if spd_total > 0 {
            Some(spd_total)
        } else {
            None
        };

        json!({
            "note": NOTE,
            "smbios_available": self.smbios_readable,
            "slots_total": self.slots_total,
            "slots_populated": slots_populated,
            "total_installed_mib": total_installed_mib,
            "meminfo_total_mib": kib_to_mib(self.live.total_kib),
            "summary": self.summary(&populated, slots_populated, total_installed_mib),
            "dimms": self.dimms.iter().map(SmbiosDimm::to_json).collect::<Vec<_>>(),
            "spd": self.spd,
            "temp_sensors": self.sensors.iter().map(TempSensor::to_json).collect::<Vec<_>>(),
        })
    }

    /// Live readings.
    pub fn reading(&self) -> Value {
        let l = &self.live;
        let total = kib_to_mib(l.total_kib);
        let available = kib_to_mib(l.available_kib);
        let used = total.saturating_sub(available);
        let used_pct = if total > 0 { util::round1(used as f64 * 100.0 / total as f64) } else { 0.0 };
        json!({
            "total_mib": total,
            "available_mib": available,
            "used_mib": used,
            "used_pct": used_pct,
            "cached_mib": kib_to_mib(l.cached_kib),
            "swap_total_mib": kib_to_mib(l.swap_total_kib),
            "swap_used_mib": kib_to_mib(l.swap_total_kib.saturating_sub(l.swap_free_kib)),
            "dimm_temps": self.sensors.iter().map(|s| json!({
                "dimm": s.label,
                "slot": s.slot,
                "i2c_addr": s.i2c_addr,
                "driver": s.driver,
                "temp": s.temp,
            })).collect::<Vec<_>>(),
        })
    }

    fn summary(&self, populated: &[&SmbiosDimm], slots_populated: Option<usize>, total_mib: Option<u64>) -> String {
        let mut parts = Vec::new();
        if let Some(first) = populated.first() {
            let all_same = populated.iter().all(|d| d.size_mib == first.size_mib);
            let size = match (all_same, first.size_mib) {
                (true, Some(s)) => format!("{} x {}", populated.len(), fmt_mib(s)),
                _ => format!("{} modules, {}", populated.len(), total_mib.map(fmt_mib).unwrap_or_default()),
            };
            parts.push(format!("{size} {}", first.memory_type));
            if let Some(s) = first.speed_mts {
                let mut sp = format!("rated {s} MT/s");
                if let Some(c) = first.configured_speed_mts {
                    sp.push_str(&format!(", running {c} MT/s"));
                }
                parts.push(sp);
            }
        } else if let Some(m) = self.spd.first() {
            let mut s = format!("{} x {}", self.spd.len(), m.dram_type);
            if let Some(mib) = total_mib {
                s = format!("{} x {} {}", self.spd.len(), fmt_mib(mib / self.spd.len().max(1) as u64), m.dram_type);
            }
            if let Some(sp) = m.jedec_speed_mts {
                s.push_str(&format!(" (JEDEC {sp} MT/s)"));
            }
            parts.push(s);
        } else {
            parts.push(format!("{} total (per /proc/meminfo)", fmt_mib(kib_to_mib(self.live.total_kib))));
        }
        match (slots_populated, self.slots_total) {
            (Some(p), n) if n > 0 => parts.push(format!("{p}/{n} slots populated")),
            (None, n) if n > 0 => parts.push(format!("{n} slots (population unknown without root)")),
            _ => {}
        }
        parts.join(", ")
    }
}

// ---------------------------------------------------------------------------
// /proc/meminfo
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
struct MemInfo {
    total_kib: u64,
    available_kib: u64,
    cached_kib: u64,
    swap_total_kib: u64,
    swap_free_kib: u64,
}

fn parse_meminfo(s: &str) -> MemInfo {
    let mut m = MemInfo::default();
    for line in s.lines() {
        let Some((key, rest)) = line.split_once(':') else { continue };
        let Some(v) = rest.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()) else { continue };
        match key.trim() {
            "MemTotal" => m.total_kib = v,
            "MemAvailable" => m.available_kib = v,
            "Cached" => m.cached_kib = v,
            "SwapTotal" => m.swap_total_kib = v,
            "SwapFree" => m.swap_free_kib = v,
            _ => {}
        }
    }
    m
}

fn kib_to_mib(kib: u64) -> u64 {
    kib / 1024
}

fn fmt_mib(mib: u64) -> String {
    if mib >= 1024 && mib.is_multiple_of(1024) { format!("{} GiB", mib / 1024) } else { format!("{mib} MiB") }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// DIMM temperature sensors (spd5118 / jc42 hwmon)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct TempSensor {
    label: String,
    slot: Option<u8>,
    i2c_addr: Option<u8>,
    driver: String,
    /// Canonical i2c client directory (e.g. .../i2c-1/1-0051), if resolvable.
    client: Option<PathBuf>,
    input: PathBuf,
    temp: Option<f64>,
}

impl TempSensor {
    fn to_json(&self) -> Value {
        json!({
            "dimm": self.label,
            "slot": self.slot,
            "i2c_addr": self.i2c_addr,
            "driver": self.driver,
            "i2c_client": self.client.as_ref().map(|c| file_name(c)),
        })
    }
}

fn find_sensors(sys: &Sys) -> Vec<TempSensor> {
    let mut out = Vec::new();
    for h in sys.list("/sys/class/hwmon") {
        let Some(name) = util::read_trim(&h.join("name")) else { continue };
        if name != "spd5118" && name != "jc42" {
            continue;
        }
        let client = fs::canonicalize(h.join("device")).ok();
        let i2c_addr = client.as_deref().and_then(i2c_addr_of);
        let slot = i2c_addr.and_then(|a| slot_for_addr(&name, a));
        let label = match slot {
            Some(s) => format!("DIMM {s}"),
            None => format!("DIMM {}", out.len()),
        };
        out.push(TempSensor {
            label,
            slot,
            i2c_addr,
            driver: name,
            client,
            input: h.join("temp1_input"),
            temp: None,
        });
    }
    out
}

/// i2c client directory "1-0051" (bus-addr, "%d-%04x") -> 0x51. Adapter
/// directories ("i2c-1") and anything else yield None.
fn i2c_addr_of(client: &Path) -> Option<u8> {
    let name = file_name(client);
    let (bus, addr) = name.split_once('-')?;
    if bus.is_empty() || !bus.bytes().all(|b| b.is_ascii_digit()) || addr.len() != 4 {
        return None;
    }
    let a = u16::from_str_radix(addr, 16).ok()?;
    u8::try_from(a).ok().filter(|a| *a <= 0x7F)
}

/// DDR5 SPD hubs live at 0x50..0x57 (slot 0..7); DDR4 jc42 sensors at 0x18..0x1F.
fn slot_for_addr(driver: &str, addr: u8) -> Option<u8> {
    match driver {
        "spd5118" if (0x50..=0x57).contains(&addr) => Some(addr - 0x50),
        "jc42" if (0x18..=0x1F).contains(&addr) => Some(addr - 0x18),
        _ => None,
    }
}

/// The spd5118 driver registers an nvmem device under the i2c client; older
/// setups expose a compat `eeprom` attribute in the client directory itself.
fn read_spd_eeprom(sys: &Sys, client: &Path) -> Option<Vec<u8>> {
    if let Ok(b) = fs::read(client.join("eeprom")) {
        return Some(b);
    }
    if let Ok(rd) = fs::read_dir(client) {
        for e in rd.flatten() {
            if let Ok(b) = fs::read(e.path().join("nvmem")) {
                return Some(b);
            }
        }
    }
    for n in sys.list("/sys/bus/nvmem/devices") {
        let Ok(c) = fs::canonicalize(&n) else { continue };
        if c.parent() != Some(client) {
            continue;
        }
        if let Ok(b) = fs::read(c.join("nvmem")) {
            return Some(b);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Bounds-checked byte access
// ---------------------------------------------------------------------------

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn u8(&self, off: usize) -> Option<u8> {
        self.0.get(off).copied()
    }
    fn u16(&self, off: usize) -> Option<u16> {
        let s = self.slice(off, 2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&self, off: usize) -> Option<u32> {
        let s = self.slice(off, 4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn slice(&self, off: usize, n: usize) -> Option<&[u8]> {
        self.0.get(off..off.checked_add(n)?)
    }
    /// Printable ASCII, trimmed of spaces and NULs.
    fn ascii(&self, off: usize, n: usize) -> Option<String> {
        let raw = self.slice(off, n)?;
        let s: String = raw
            .iter()
            .take_while(|b| **b != 0)
            .map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '?' })
            .collect();
        let t = s.trim().to_string();
        if t.is_empty() { None } else { Some(t) }
    }
}

// ---------------------------------------------------------------------------
// SMBIOS type 17 (Memory Device)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct SmbiosDimm {
    pub handle: u16,
    pub locator: String,
    pub bank_locator: String,
    pub populated: bool,
    pub size_mib: Option<u64>,
    pub form_factor: String,
    pub memory_type: String,
    pub type_detail: Vec<String>,
    pub total_width_bits: Option<u16>,
    pub data_width_bits: Option<u16>,
    pub speed_mts: Option<u32>,
    pub configured_speed_mts: Option<u32>,
    pub manufacturer: String,
    pub serial_number: String,
    pub part_number: String,
    pub ranks: Option<u8>,
    pub min_voltage_mv: Option<u16>,
    pub max_voltage_mv: Option<u16>,
    pub configured_voltage_mv: Option<u16>,
}

impl SmbiosDimm {
    fn to_json(&self) -> Value {
        if !self.populated {
            return json!({
                "handle": self.handle,
                "locator": self.locator,
                "bank_locator": self.bank_locator,
                "populated": false,
            });
        }
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

/// Parse one raw SMBIOS structure (formatted area + string table). Returns
/// `None` if it is not a type-17 record or too short to carry a size field.
pub fn parse_type17(raw: &[u8]) -> Option<SmbiosDimm> {
    if raw.first() != Some(&17) {
        return None;
    }
    let len = *raw.get(1)? as usize;
    // Only fields inside the declared structure length are valid.
    let f = Bytes(raw.get(..len.min(raw.len()))?);
    let strings = string_table(raw, len);
    let s = |idx: Option<u8>| -> String {
        match idx {
            Some(i) if i > 0 => strings.get(i as usize - 1).cloned().unwrap_or_default(),
            _ => String::new(),
        }
    };

    let handle = f.u16(0x02)?;
    let locator = s(f.u8(0x10));
    let bank_locator = s(f.u8(0x11));
    let (populated, size_mib) = match f.u16(0x0C)? {
        0 => (false, None),
        0xFFFF => (true, None),
        0x7FFF => (true, f.u32(0x1C).map(|v| u64::from(v & 0x7FFF_FFFF))),
        v if v & 0x8000 != 0 => (true, Some(u64::from(v & 0x7FFF) / 1024)),
        v => (true, Some(u64::from(v))),
    };
    if !populated {
        return Some(SmbiosDimm { handle, locator, bank_locator, ..Default::default() });
    }

    let width = |off| f.u16(off).filter(|v| *v != 0xFFFF && *v != 0);
    let speed = |off16, off32| match f.u16(off16) {
        Some(0) | None => None,
        Some(0xFFFF) => f.u32(off32).filter(|v| *v != 0),
        Some(v) => Some(u32::from(v)),
    };
    let volt = |off| f.u16(off).filter(|v| *v != 0);

    Some(SmbiosDimm {
        handle,
        locator,
        bank_locator,
        populated: true,
        size_mib,
        form_factor: form_factor_name(f.u8(0x0E)),
        memory_type: memory_type_name(f.u8(0x12)),
        type_detail: type_detail_names(f.u16(0x13).unwrap_or(0)),
        total_width_bits: width(0x08),
        data_width_bits: width(0x0A),
        speed_mts: speed(0x15, 0x54),
        configured_speed_mts: speed(0x20, 0x58),
        manufacturer: s(f.u8(0x17)),
        serial_number: s(f.u8(0x18)),
        part_number: s(f.u8(0x1A)),
        ranks: f.u8(0x1B).map(|a| a & 0x0F).filter(|r| *r != 0),
        min_voltage_mv: volt(0x22),
        max_voltage_mv: volt(0x24),
        configured_voltage_mv: volt(0x26),
    })
}

/// Strings following the formatted area: NUL-terminated, table ends at an
/// empty string. Leading/trailing padding is trimmed.
fn string_table(raw: &[u8], len: usize) -> Vec<String> {
    let Some(tail) = raw.get(len..) else { return Vec::new() };
    let mut out = Vec::new();
    for chunk in tail.split(|b| *b == 0) {
        if chunk.is_empty() {
            break;
        }
        out.push(String::from_utf8_lossy(chunk).trim().to_string());
    }
    out
}

fn memory_type_name(code: Option<u8>) -> String {
    let name = match code {
        Some(0x01) => "Other",
        Some(0x02) => "Unknown",
        Some(0x03) => "DRAM",
        Some(0x04) => "EDRAM",
        Some(0x05) => "VRAM",
        Some(0x06) => "SRAM",
        Some(0x07) => "RAM",
        Some(0x08) => "ROM",
        Some(0x09) => "Flash",
        Some(0x0A) => "EEPROM",
        Some(0x0B) => "FEPROM",
        Some(0x0C) => "EPROM",
        Some(0x0D) => "CDRAM",
        Some(0x0E) => "3DRAM",
        Some(0x0F) => "SDRAM",
        Some(0x10) => "SGRAM",
        Some(0x11) => "RDRAM",
        Some(0x12) => "DDR",
        Some(0x13) => "DDR2",
        Some(0x14) => "DDR2 FB-DIMM",
        Some(0x18) => "DDR3",
        Some(0x19) => "FBD2",
        Some(0x1A) => "DDR4",
        Some(0x1B) => "LPDDR",
        Some(0x1C) => "LPDDR2",
        Some(0x1D) => "LPDDR3",
        Some(0x1E) => "LPDDR4",
        Some(0x1F) => "Logical non-volatile device",
        Some(0x20) => "HBM",
        Some(0x21) => "HBM2",
        Some(0x22) => "DDR5",
        Some(0x23) => "LPDDR5",
        Some(0x24) => "HBM3",
        Some(c) => return format!("Unknown (0x{c:02X})"),
        None => "Unknown",
    };
    name.to_string()
}

fn form_factor_name(code: Option<u8>) -> String {
    let name = match code {
        Some(0x01) => "Other",
        Some(0x02) => "Unknown",
        Some(0x03) => "SIMM",
        Some(0x04) => "SIP",
        Some(0x05) => "Chip",
        Some(0x06) => "DIP",
        Some(0x07) => "ZIP",
        Some(0x08) => "Proprietary Card",
        Some(0x09) => "DIMM",
        Some(0x0A) => "TSOP",
        Some(0x0B) => "Row of chips",
        Some(0x0C) => "RIMM",
        Some(0x0D) => "SODIMM",
        Some(0x0E) => "SRIMM",
        Some(0x0F) => "FB-DIMM",
        Some(0x10) => "Die",
        Some(c) => return format!("Unknown (0x{c:02X})"),
        None => "Unknown",
    };
    name.to_string()
}

fn type_detail_names(bits: u16) -> Vec<String> {
    const NAMES: [(u16, &str); 14] = [
        (1, "Other"),
        (2, "Unknown"),
        (3, "Fast-paged"),
        (4, "Static column"),
        (5, "Pseudo-static"),
        (6, "RAMBUS"),
        (7, "Synchronous"),
        (8, "CMOS"),
        (9, "EDO"),
        (10, "Window DRAM"),
        (11, "Cache DRAM"),
        (12, "Non-volatile"),
        (13, "Registered (Buffered)"),
        (14, "Unbuffered (Unregistered)"),
    ];
    let mut v: Vec<String> =
        NAMES.iter().filter(|(b, _)| bits & (1 << b) != 0).map(|(_, n)| n.to_string()).collect();
    if bits & (1 << 15) != 0 {
        v.push("LRDIMM".into());
    }
    v
}

// ---------------------------------------------------------------------------
// DDR5 SPD (JESD400-5) + XMP 3.0 / EXPO blocks
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct SpdModule {
    pub slot: Option<u8>,
    pub i2c_addr: Option<u8>,
    pub spd_bytes: usize,
    pub spd_revision: String,
    pub dram_type: String,
    pub module_type: String,
    /// CRC of bytes 0..=509 matches bytes 510..=511 (None if SPD too short).
    pub crc_ok: Option<bool>,
    pub part_number: String,
    pub revision_code: Option<u8>,
    pub module_manufacturer: String,
    pub module_manufacturer_id: String,
    pub dram_manufacturer: String,
    pub dram_manufacturer_id: String,
    pub manufacture_year: Option<u16>,
    pub manufacture_week: Option<u8>,
    pub serial_number: String,
    pub die_density_gbit: Option<u32>,
    pub dies_per_package: Option<u8>,
    pub io_width_bits: Option<u8>,
    pub ranks_per_channel: Option<u8>,
    pub channels: Option<u8>,
    pub bus_width_bits_per_channel: Option<u8>,
    /// Computed from density × dies × devices per rank × ranks × channels.
    pub size_mib: Option<u64>,
    pub tck_min_ps: Option<u16>,
    pub tck_max_ps: Option<u16>,
    /// Max JEDEC data rate (2e6 / tCKAVGmin, rounded to 100 MT/s).
    pub jedec_speed_mts: Option<u32>,
    pub cas_latencies: Vec<u16>,
    pub taa_min_ps: Option<u16>,
    pub trcd_min_ps: Option<u16>,
    pub trp_min_ps: Option<u16>,
    pub tras_min_ps: Option<u16>,
    pub trc_min_ps: Option<u16>,
    pub twr_min_ps: Option<u16>,
    pub trfc1_ns: Option<u16>,
    pub trfc2_ns: Option<u16>,
    pub trfcsb_ns: Option<u16>,
    /// "CL-tRCD-tRP-tRAS" in clocks at the JEDEC speed.
    pub jedec_timings: Option<String>,
    pub xmp: Value,
    pub expo: Value,
}

#[derive(Clone, Debug, Serialize)]
struct Profile {
    profile: u8,
    name: Option<String>,
    speed_mts: u32,
    tck_ps: u16,
    cl: u32,
    trcd: u32,
    trp: u32,
    tras: u32,
    /// "CL-tRCD-tRP-tRAS" in clocks.
    timings: String,
    taa_ps: u16,
    trcd_ps: u16,
    trp_ps: u16,
    tras_ps: u16,
    vdd_mv: u16,
    vddq_mv: u16,
    vpp_mv: u16,
}

/// Decode a DDR5 SPD image (typically 1024 bytes). Returns None for non-DDR5 SPDs.
pub fn parse_spd(bytes: &[u8], slot: Option<u8>, i2c_addr: Option<u8>) -> Option<SpdModule> {
    let b = Bytes(bytes);
    let dram_type = match b.u8(2)? {
        0x12 => "DDR5",
        0x13 => "LPDDR5",
        _ => return None,
    };
    let rev = b.u8(1).unwrap_or(0);
    let module_type = module_type_name(b.u8(3).unwrap_or(0));

    let crc_ok = b.slice(0, 510).and_then(|d| Some(crc16(d) == b.u16(510)?));

    // Organisation (byte 4..7 first SDRAM, 234/235 module).
    let b4 = b.u8(4);
    let die_density_gbit = b4.and_then(|v| match v & 0x1F {
        1 => Some(4),
        2 => Some(8),
        3 => Some(12),
        4 => Some(16),
        5 => Some(24),
        6 => Some(32),
        7 => Some(48),
        8 => Some(64),
        _ => None,
    });
    let dies_per_package = b4.and_then(|v| match v >> 5 {
        0 => Some(1),
        1 | 2 => Some(2),
        3 => Some(4),
        4 => Some(8),
        5 => Some(16),
        _ => None,
    });
    let io_width_bits = b.u8(6).and_then(|v| match v >> 5 {
        0 => Some(4),
        1 => Some(8),
        2 => Some(16),
        3 => Some(32),
        _ => None,
    });
    let ranks_per_channel = b.u8(234).map(|v| ((v >> 3) & 0x07) + 1);
    let bus_width_bits_per_channel = b.u8(235).and_then(|v| match v & 0x07 {
        0 => Some(8),
        1 => Some(16),
        2 => Some(32),
        3 => Some(64),
        _ => None,
    });
    let channels = b.u8(235).and_then(|v| match v >> 5 {
        0 => Some(1),
        1 => Some(2),
        2 => Some(4),
        3 => Some(8),
        _ => None,
    });
    let size_mib = (|| {
        let devices = u64::from(bus_width_bits_per_channel?) / u64::from(io_width_bits?);
        Some(
            u64::from(die_density_gbit?) * 128
                * u64::from(dies_per_package?)
                * devices
                * u64::from(ranks_per_channel?)
                * u64::from(channels?),
        )
    })();

    // Timings (ps unless noted).
    let tck_min_ps = b.u16(20).filter(|v| *v != 0);
    let tck_max_ps = b.u16(22).filter(|v| *v != 0);
    let mut cas_latencies = Vec::new();
    for i in 0..5usize {
        let byte = b.u8(24 + i).unwrap_or(0);
        for bit in 0..8u16 {
            if byte & (1 << bit) != 0 {
                cas_latencies.push(20 + 2 * (i as u16 * 8 + bit));
            }
        }
    }
    let t = |off| b.u16(off).filter(|v| *v != 0);
    let taa_min_ps = t(30);
    let trcd_min_ps = t(32);
    let trp_min_ps = t(34);
    let tras_min_ps = t(36);
    let jedec_speed_mts = tck_min_ps.and_then(speed_from_tck);
    let jedec_timings = (|| {
        let tck = tck_min_ps?;
        let cl_min = cycles(taa_min_ps?, tck)?;
        let cl = cas_latencies.iter().map(|c| u32::from(*c)).find(|c| *c >= cl_min).unwrap_or(cl_min);
        Some(format!(
            "{cl}-{}-{}-{}",
            cycles(trcd_min_ps?, tck)?,
            cycles(trp_min_ps?, tck)?,
            cycles(tras_min_ps?, tck)?
        ))
    })();

    // Manufacturing block (bytes 512..).
    let (module_manufacturer, module_manufacturer_id) = jedec_vendor(b.u8(512), b.u8(513));
    let (dram_manufacturer, dram_manufacturer_id) = jedec_vendor(b.u8(552), b.u8(553));
    let manufacture_year = b.u8(515).and_then(bcd).map(|y| 2000 + u16::from(y));
    let manufacture_week = b.u8(516).and_then(bcd);
    let serial_number = b.slice(517, 4).map(|s| format!("{:02X}{:02X}{:02X}{:02X}", s[0], s[1], s[2], s[3])).unwrap_or_default();

    Some(SpdModule {
        slot,
        i2c_addr,
        spd_bytes: bytes.len(),
        spd_revision: format!("{}.{}", rev >> 4, rev & 0x0F),
        dram_type: dram_type.to_string(),
        module_type,
        crc_ok,
        part_number: b.ascii(521, 30).unwrap_or_default(),
        revision_code: b.u8(551),
        module_manufacturer,
        module_manufacturer_id,
        dram_manufacturer,
        dram_manufacturer_id,
        manufacture_year,
        manufacture_week,
        serial_number,
        die_density_gbit,
        dies_per_package,
        io_width_bits,
        ranks_per_channel,
        channels,
        bus_width_bits_per_channel,
        size_mib,
        tck_min_ps,
        tck_max_ps,
        jedec_speed_mts,
        cas_latencies,
        taa_min_ps,
        trcd_min_ps,
        trp_min_ps,
        tras_min_ps,
        trc_min_ps: t(38),
        twr_min_ps: t(40),
        trfc1_ns: t(42),
        trfc2_ns: t(44),
        trfcsb_ns: t(46),
        jedec_timings,
        xmp: parse_xmp(&b),
        expo: parse_expo(&b),
    })
}

fn module_type_name(byte3: u8) -> String {
    let base = match byte3 & 0x0F {
        0x01 => "RDIMM",
        0x02 => "UDIMM",
        0x03 => "SODIMM",
        0x04 => "LRDIMM",
        0x05 => "CUDIMM",
        0x06 => "CSODIMM",
        0x07 => "MRDIMM",
        0x08 => "CAMM2",
        0x0A => "DDIMM",
        0x0B => "Solder down",
        c => return format!("Unknown (0x{c:02X})"),
    };
    if byte3 & 0x80 != 0 { format!("{base} (hybrid)") } else { base.to_string() }
}

/// JEP106 vendor name from the SPD two-byte ID (byte 0: continuation count
/// with odd-parity bit 7; byte 1: code with parity bit).
fn jedec_vendor(b0: Option<u8>, b1: Option<u8>) -> (String, String) {
    let (Some(b0), Some(b1)) = (b0, b1) else { return (String::new(), String::new()) };
    let id = format!("0x{b0:02X}{b1:02X}");
    if (b0, b1) == (0, 0) || (b0, b1) == (0xFF, 0xFF) {
        return (String::new(), id);
    }
    let bank = (b0 & 0x7F) + 1;
    let code = b1 & 0x7F;
    const VENDORS: [(u8, u8, &str); 12] = [
        (1, 0x0B, "Nanya"),
        (1, 0x2C, "Micron"),
        (1, 0x2D, "SK hynix"),
        (1, 0x4E, "Samsung"),
        (2, 0x18, "Kingston"),
        (3, 0x1E, "Corsair"),
        (5, 0x4B, "ADATA"),
        (5, 0x4D, "G.Skill"),
        (5, 0x6F, "TeamGroup"),
        (6, 0x02, "Patriot"),
        (6, 0x1B, "Crucial"),
        (9, 0x3C, "CXMT"),
    ];
    let name = VENDORS
        .iter()
        .find(|(b, c, _)| *b == bank && *c == code)
        .map(|(_, _, n)| n.to_string())
        .unwrap_or_else(|| id.clone());
    (name, id)
}

fn bcd(b: u8) -> Option<u8> {
    let (hi, lo) = (b >> 4, b & 0x0F);
    if hi > 9 || lo > 9 { None } else { Some(hi * 10 + lo) }
}

/// Data rate in MT/s from a cycle time in ps, rounded to the nearest 100.
fn speed_from_tck(tck_ps: u16) -> Option<u32> {
    if tck_ps == 0 {
        return None;
    }
    let mts = 2_000_000.0 / f64::from(tck_ps);
    Some(((mts / 100.0).round() * 100.0) as u32)
}

/// JEDEC clock-count rounding: ceil(t / tCK) with a 2.6 % guard band.
fn cycles(t_ps: u16, tck_ps: u16) -> Option<u32> {
    if tck_ps == 0 {
        return None;
    }
    Some((u32::from(t_ps) * 1000 / u32::from(tck_ps) + 974) / 1000)
}

/// CRC-16/XMODEM (poly 0x1021, init 0), as used by JEDEC SPD and the XMP/EXPO blocks.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

fn block_crc_ok(b: &Bytes, start: usize, len: usize) -> Option<bool> {
    let data = b.slice(start, len.checked_sub(2)?)?;
    Some(crc16(data) == b.u16(start + len - 2)?)
}

/// XMP/EXPO voltage byte: bits 7:5 volts, bits 4:0 in 50 mV steps.
fn profile_voltage(byte: u8) -> u16 {
    u16::from(byte >> 5) * 1000 + u16::from(byte & 0x1F) * 50
}

struct ProfileOffsets {
    tck: usize,
    taa: usize,
    trcd: usize,
    trp: usize,
    tras: usize,
    vdd: usize,
    vddq: usize,
    vpp: usize,
}

/// Decode a profile; returns None when the fields do not look like a sane DDR5
/// profile (guards against layout differences in unverified firmware).
fn profile_from(b: &Bytes, number: u8, name: Option<String>, o: ProfileOffsets) -> Option<Profile> {
    let tck_ps = b.u16(o.tck)?;
    let taa_ps = b.u16(o.taa)?;
    let trcd_ps = b.u16(o.trcd)?;
    let trp_ps = b.u16(o.trp)?;
    let tras_ps = b.u16(o.tras)?;
    // DDR5-2000 .. DDR5-13333; each timing at least one clock, tRAS >= tAA
    // (u16 picoseconds already caps everything at 65.5 ns).
    let sane = (150..=1000).contains(&tck_ps)
        && [taa_ps, trcd_ps, trp_ps].iter().all(|t| *t >= tck_ps)
        && tras_ps >= taa_ps;
    if !sane {
        return None;
    }
    let cl = cycles(taa_ps, tck_ps)?;
    let trcd = cycles(trcd_ps, tck_ps)?;
    let trp = cycles(trp_ps, tck_ps)?;
    let tras = cycles(tras_ps, tck_ps)?;
    Some(Profile {
        profile: number,
        name,
        speed_mts: speed_from_tck(tck_ps)?,
        tck_ps,
        cl,
        trcd,
        trp,
        tras,
        timings: format!("{cl}-{trcd}-{trp}-{tras}"),
        taa_ps,
        trcd_ps,
        trp_ps,
        tras_ps,
        vdd_mv: profile_voltage(b.u8(o.vdd)?),
        vddq_mv: profile_voltage(b.u8(o.vddq)?),
        vpp_mv: profile_voltage(b.u8(o.vpp)?),
    })
}

/// XMP 3.0: header at 640 (magic 0x0C 0x4A), profile enable bits at 643,
/// 16-byte names from 654, 64-byte profiles 1/2 at 704/768.
fn parse_xmp(b: &Bytes) -> Value {
    if b.slice(640, 2) != Some(&[0x0C, 0x4A]) {
        return json!({ "present": false });
    }
    let enable = b.u8(643).unwrap_or(0);
    let mut profiles = Vec::new();
    for i in 0..2usize {
        if enable & (1 << i) == 0 {
            continue;
        }
        let base = 704 + 64 * i;
        let name = b.ascii(654 + 16 * i, 16);
        if let Some(p) = profile_from(
            b,
            i as u8 + 1,
            name,
            ProfileOffsets {
                tck: base + 5,
                taa: base + 13,
                trcd: base + 15,
                trp: base + 17,
                tras: base + 19,
                vpp: base,
                vdd: base + 1,
                vddq: base + 2,
            },
        ) {
            profiles.push(p);
        }
    }
    json!({
        "present": true,
        "profile_flags": enable,
        "header_crc_ok": block_crc_ok(b, 640, 64),
        "profiles": profiles,
    })
}

/// AMD EXPO: "EXPO" at 832, 10-byte header, two 40-byte profiles at 842/882,
/// block CRC over 832..=957 at 958. Profile layout follows the community
/// decoders (tCK at +4, tAA/tRCD/tRP/tRAS at +6/+8/+10/+12, VDD/VDDQ/VPP at
/// +0/+1/+2); profiles failing the sanity gate are omitted.
fn parse_expo(b: &Bytes) -> Value {
    if b.slice(832, 4) != Some(b"EXPO") {
        return json!({ "present": false });
    }
    let mut profiles = Vec::new();
    for i in 0..2usize {
        let base = 842 + 40 * i;
        if b.u16(base + 4).unwrap_or(0) == 0 {
            continue;
        }
        if let Some(p) = profile_from(
            b,
            i as u8 + 1,
            None,
            ProfileOffsets {
                tck: base + 4,
                taa: base + 6,
                trcd: base + 8,
                trp: base + 10,
                tras: base + 12,
                vdd: base,
                vddq: base + 1,
                vpp: base + 2,
            },
        ) {
            profiles.push(p);
        }
    }
    json!({
        "present": true,
        "block_crc_ok": block_crc_ok(b, 832, 128),
        "profiles": profiles,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a type-17 record: a formatted area of `len` bytes plus a string table.
    struct T17 {
        f: Vec<u8>,
        strings: Vec<&'static str>,
    }

    impl T17 {
        fn new(len: usize, handle: u16) -> Self {
            let mut f = vec![0u8; len];
            f[0] = 17;
            f[1] = len as u8;
            f[2..4].copy_from_slice(&handle.to_le_bytes());
            T17 { f, strings: Vec::new() }
        }
        fn u8(mut self, off: usize, v: u8) -> Self {
            self.f[off] = v;
            self
        }
        fn u16(mut self, off: usize, v: u16) -> Self {
            self.f[off..off + 2].copy_from_slice(&v.to_le_bytes());
            self
        }
        fn u32(mut self, off: usize, v: u32) -> Self {
            self.f[off..off + 4].copy_from_slice(&v.to_le_bytes());
            self
        }
        /// Add a string and store its 1-based index at `off`.
        fn s(mut self, off: usize, v: &'static str) -> Self {
            self.strings.push(v);
            self.f[off] = self.strings.len() as u8;
            self
        }
        fn build(self) -> Vec<u8> {
            let mut raw = self.f;
            for s in &self.strings {
                raw.extend_from_slice(s.as_bytes());
                raw.push(0);
            }
            if self.strings.is_empty() {
                raw.push(0);
            }
            raw.push(0);
            raw
        }
    }

    fn ddr5_record() -> Vec<u8> {
        T17::new(0x5C, 0x0040)
            .u16(0x08, 64)
            .u16(0x0A, 64)
            .u16(0x0C, 0x7FFF)
            .u32(0x1C, 32 * 1024)
            .u8(0x0E, 0x09)
            .s(0x10, "DIMM_A2  ")
            .s(0x11, "BANK 1")
            .u8(0x12, 0x22)
            .u16(0x13, 1 << 7 | 1 << 14)
            .u16(0x15, 6000)
            .s(0x17, "Kingston")
            .s(0x18, "ABCD1234")
            .s(0x1A, "KF560C36-32   ")
            .u8(0x1B, 0x02)
            .u16(0x20, 4800)
            .u16(0x22, 1100)
            .u16(0x24, 1100)
            .u16(0x26, 1100)
            .build()
    }

    #[test]
    fn smbios_ddr5_dimm() {
        let d = parse_type17(&ddr5_record()).expect("type 17");
        assert!(d.populated);
        assert_eq!(d.handle, 0x40);
        assert_eq!(d.size_mib, Some(32768));
        assert_eq!(d.locator, "DIMM_A2");
        assert_eq!(d.bank_locator, "BANK 1");
        assert_eq!(d.memory_type, "DDR5");
        assert_eq!(d.form_factor, "DIMM");
        assert_eq!(d.speed_mts, Some(6000));
        assert_eq!(d.configured_speed_mts, Some(4800));
        assert_eq!(d.manufacturer, "Kingston");
        assert_eq!(d.serial_number, "ABCD1234");
        assert_eq!(d.part_number, "KF560C36-32");
        assert_eq!(d.ranks, Some(2));
        assert_eq!(d.configured_voltage_mv, Some(1100));
        assert_eq!(d.total_width_bits, Some(64));
        assert_eq!(d.type_detail, vec!["Synchronous", "Unbuffered (Unregistered)"]);
        let j = d.to_json();
        assert_eq!(j["size_mib"], 32768);
        assert_eq!(j["configured_speed_mts"], 4800);
    }

    #[test]
    fn smbios_empty_slot() {
        let raw = T17::new(0x5C, 0x0041).s(0x10, "DIMM_B1").s(0x11, "BANK 2").u8(0x12, 0x02).build();
        let d = parse_type17(&raw).expect("type 17");
        assert!(!d.populated);
        assert_eq!(d.locator, "DIMM_B1");
        assert_eq!(d.size_mib, None);
        let j = d.to_json();
        assert_eq!(j["populated"], false);
        assert!(j.get("speed_mts").is_none());
    }

    #[test]
    fn smbios_extended_speed_and_kib_size() {
        let raw = T17::new(0x5C, 0x0042)
            .u16(0x0C, 0x8000 | 2048) // 2048 KiB = 2 MiB
            .u8(0x12, 0x22)
            .u16(0x15, 0xFFFF)
            .u32(0x54, 70000)
            .u16(0x20, 0xFFFF)
            .u32(0x58, 68000)
            .build();
        let d = parse_type17(&raw).expect("type 17");
        assert_eq!(d.size_mib, Some(2));
        assert_eq!(d.speed_mts, Some(70000));
        assert_eq!(d.configured_speed_mts, Some(68000));
    }

    #[test]
    fn smbios_short_structure_stays_in_bounds() {
        // SMBIOS 2.1 length (0x15): no speed, voltage or extended fields.
        let raw = T17::new(0x15, 0x0043).u16(0x0C, 8192).u8(0x12, 0x1A).s(0x10, "DIMM0").build();
        let d = parse_type17(&raw).expect("type 17");
        assert_eq!(d.size_mib, Some(8192));
        assert_eq!(d.memory_type, "DDR4");
        assert_eq!(d.speed_mts, None);
        assert_eq!(d.manufacturer, "");
        // Truncated / garbage input must not panic.
        assert!(parse_type17(&[]).is_none());
        assert!(parse_type17(&[17]).is_none());
        assert!(parse_type17(&[17, 0x5C, 0, 0]).is_none());
        assert!(parse_type17(&[4, 0x10, 0, 0]).is_none());
        let mut truncated = ddr5_record();
        truncated.truncate(0x20);
        let d = parse_type17(&truncated).expect("truncated still parses what it has");
        assert_eq!(d.speed_mts, Some(6000));
        assert_eq!(d.configured_speed_mts, None);
    }

    fn put16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }

    fn ddr5_spd() -> Vec<u8> {
        let mut s = vec![0u8; 1024];
        s[0] = 0x30; // 1024 bytes total
        s[1] = 0x11; // SPD rev 1.1
        s[2] = 0x12; // DDR5
        s[3] = 0x02; // UDIMM
        s[4] = 0x04; // 16 Gb, monolithic
        s[6] = 0x20; // x8
        put16(&mut s, 20, 416); // tCK 416 ps -> 4800
        put16(&mut s, 22, 1010);
        s[24] = 0b1111_0000; // CL 28,30,32,34
        s[25] = 0b0000_0111; // CL 36,38,40
        put16(&mut s, 30, 16000); // tAA
        put16(&mut s, 32, 16000); // tRCD
        put16(&mut s, 34, 16000); // tRP
        put16(&mut s, 36, 32000); // tRAS
        put16(&mut s, 38, 48000); // tRC
        put16(&mut s, 40, 30000); // tWR
        put16(&mut s, 42, 295); // tRFC1 ns
        put16(&mut s, 44, 160);
        put16(&mut s, 46, 130);
        s[234] = 0b0000_0000; // 1 rank per channel
        s[235] = 0b0010_0010; // 2 channels, 32-bit each
        s[512] = 0x01; // Kingston: bank 2
        s[513] = 0x98;
        s[515] = 0x24; // 2024
        s[516] = 0x15; // week 15
        s[517..521].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let pn = b"KF560C36BBEK2-32              ";
        s[521..551].copy_from_slice(pn);
        s[551] = 0x01;
        s[552] = 0x80; // SK hynix
        s[553] = 0xAD;
        let crc = crc16(&s[..510]);
        put16(&mut s, 510, crc);
        // XMP 3.0 header + profile 1 (6000 MT/s 36-38-38-80, 1.35 V).
        s[640] = 0x0C;
        s[641] = 0x4A;
        s[643] = 0x01;
        s[654..670].copy_from_slice(b"XMP Profile 1   ");
        let p = 704;
        s[p] = 0x30; // VPP 1.8 V
        s[p + 1] = 0x27; // VDD 1.35 V
        s[p + 2] = 0x27; // VDDQ 1.35 V
        put16(&mut s, p + 5, 333);
        put16(&mut s, p + 13, 36 * 333);
        put16(&mut s, p + 15, 38 * 333);
        put16(&mut s, p + 17, 38 * 333);
        put16(&mut s, p + 19, 80 * 333);
        let crc = crc16(&s[640..702]);
        put16(&mut s, 702, crc);
        // EXPO header + profile 1 (6000 MT/s 36-38-38-80 1.35 V) + profile 2 (5600 34-36-36-76).
        s[832..836].copy_from_slice(b"EXPO");
        s[836] = 0x10;
        let p = 842;
        s[p] = 0x27;
        s[p + 1] = 0x27;
        s[p + 2] = 0x30;
        put16(&mut s, p + 4, 333);
        put16(&mut s, p + 6, 36 * 333);
        put16(&mut s, p + 8, 38 * 333);
        put16(&mut s, p + 10, 38 * 333);
        put16(&mut s, p + 12, 80 * 333);
        let p = 882;
        s[p] = 0x22;
        s[p + 1] = 0x22;
        s[p + 2] = 0x30;
        put16(&mut s, p + 4, 357);
        put16(&mut s, p + 6, 34 * 357);
        put16(&mut s, p + 8, 36 * 357);
        put16(&mut s, p + 10, 36 * 357);
        put16(&mut s, p + 12, 76 * 357);
        let crc = crc16(&s[832..958]);
        put16(&mut s, 958, crc);
        s
    }

    #[test]
    fn spd_ddr5_decode() {
        let m = parse_spd(&ddr5_spd(), Some(1), Some(0x51)).expect("ddr5");
        assert_eq!(m.dram_type, "DDR5");
        assert_eq!(m.module_type, "UDIMM");
        assert_eq!(m.spd_revision, "1.1");
        assert_eq!(m.crc_ok, Some(true));
        assert_eq!(m.part_number, "KF560C36BBEK2-32");
        assert_eq!(m.module_manufacturer, "Kingston");
        assert_eq!(m.module_manufacturer_id, "0x0198");
        assert_eq!(m.dram_manufacturer, "SK hynix");
        assert_eq!(m.dram_manufacturer_id, "0x80AD");
        assert_eq!(m.manufacture_year, Some(2024));
        assert_eq!(m.manufacture_week, Some(15));
        assert_eq!(m.serial_number, "DEADBEEF");
        assert_eq!(m.die_density_gbit, Some(16));
        assert_eq!(m.io_width_bits, Some(8));
        assert_eq!(m.channels, Some(2));
        assert_eq!(m.ranks_per_channel, Some(1));
        assert_eq!(m.size_mib, Some(16384));
        assert_eq!(m.tck_min_ps, Some(416));
        assert_eq!(m.jedec_speed_mts, Some(4800));
        assert_eq!(m.cas_latencies, vec![28, 30, 32, 34, 36, 38, 40]);
        assert_eq!(m.taa_min_ps, Some(16000));
        assert_eq!(m.trfc1_ns, Some(295));
        // 16000/416 = 38.46 -> 39 -> smallest supported CL >= 39 is 40.
        assert_eq!(m.jedec_timings.as_deref(), Some("40-39-39-77"));

        assert_eq!(m.xmp["present"], true);
        assert_eq!(m.xmp["header_crc_ok"], true);
        let xp = &m.xmp["profiles"][0];
        assert_eq!(xp["name"], "XMP Profile 1");
        assert_eq!(xp["speed_mts"], 6000);
        assert_eq!(xp["timings"], "36-38-38-80");
        assert_eq!(xp["vdd_mv"], 1350);
        assert_eq!(xp["vpp_mv"], 1800);

        assert_eq!(m.expo["present"], true);
        assert_eq!(m.expo["block_crc_ok"], true);
        let ep = m.expo["profiles"].as_array().expect("profiles");
        assert_eq!(ep.len(), 2);
        assert_eq!(ep[0]["speed_mts"], 6000);
        assert_eq!(ep[0]["timings"], "36-38-38-80");
        assert_eq!(ep[0]["vdd_mv"], 1350);
        assert_eq!(ep[1]["speed_mts"], 5600);
        assert_eq!(ep[1]["timings"], "34-36-36-76");
        assert_eq!(ep[1]["vdd_mv"], 1100);

        let j = serde_json::to_value(&m).expect("serialize");
        assert_eq!(j["jedec_speed_mts"], 4800);
        assert_eq!(j["slot"], 1);
    }

    #[test]
    fn spd_malformed_inputs() {
        assert!(parse_spd(&[], None, None).is_none());
        assert!(parse_spd(&[0x23, 0x10, 0x0C], None, None).is_none()); // DDR4
        // A DDR5 key byte with nothing else: no panic, mostly empty.
        let m = parse_spd(&[0x30, 0x11, 0x12], None, None).expect("ddr5");
        assert_eq!(m.crc_ok, None);
        assert_eq!(m.size_mib, None);
        assert_eq!(m.xmp["present"], false);
        assert_eq!(m.expo["present"], false);
        // Garbage profile values are gated out, presence still reported.
        let mut s = ddr5_spd();
        put16(&mut s, 842 + 4, 7);
        put16(&mut s, 704 + 5, 0xFFFF);
        let m = parse_spd(&s, None, None).expect("ddr5");
        assert_eq!(m.expo["present"], true);
        assert_eq!(m.expo["profiles"].as_array().map(Vec::len), Some(1));
        assert_eq!(m.xmp["profiles"].as_array().map(Vec::len), Some(0));
    }

    #[test]
    fn helpers() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
        assert_eq!(speed_from_tck(416), Some(4800));
        assert_eq!(speed_from_tck(333), Some(6000));
        assert_eq!(speed_from_tck(312), Some(6400));
        assert_eq!(cycles(36 * 333, 333), Some(36));
        assert_eq!(cycles(13750, 416), Some(34));
        assert_eq!(profile_voltage(0x27), 1350);
        assert_eq!(i2c_addr_of(Path::new("/sys/devices/platform/i2c-1/1-0051")), Some(0x51));
        assert_eq!(i2c_addr_of(Path::new("/sys/devices/i2c-1")), None);
        assert_eq!(i2c_addr_of(Path::new("/sys/devices/platform/it87.2624")), None);
        assert_eq!(i2c_addr_of(Path::new("1-00ad")), None);
        assert_eq!(slot_for_addr("spd5118", 0x53), Some(3));
        assert_eq!(slot_for_addr("jc42", 0x1A), Some(2));
        assert_eq!(slot_for_addr("spd5118", 0x20), None);
        assert_eq!(jedec_vendor(Some(0x80), Some(0xCE)).0, "Samsung");
        assert_eq!(jedec_vendor(Some(0x04), Some(0xCD)).0, "G.Skill");
        assert_eq!(jedec_vendor(Some(0x7F), Some(0x01)).0, "0x7F01");
        let mi = parse_meminfo("MemTotal:  16384000 kB\nMemAvailable: 8192000 kB\nCached: 1024 kB\nSwapTotal: 2048 kB\nSwapFree: 1024 kB\n");
        assert_eq!(mi.total_kib, 16384000);
        assert_eq!(mi.swap_free_kib, 1024);
    }

    #[test]
    fn fake_sysfs_end_to_end() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "omarchy-oc-mem-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&root);
        let mk = |rel: &str| {
            let p = root.join(rel);
            fs::create_dir_all(&p).expect("mkdir");
            p
        };
        mk("proc");
        fs::write(
            root.join("proc/meminfo"),
            "MemTotal:       16777216 kB\nMemFree:         1000000 kB\nMemAvailable:    8388608 kB\nCached:          4194304 kB\nSwapTotal:       4194304 kB\nSwapFree:        3145728 kB\n",
        )
        .expect("meminfo");
        let client = mk("sys/devices/platform/i2c-1/1-0051");
        fs::write(client.join("name"), "spd5118\n").expect("name");
        let nv = mk("sys/devices/platform/i2c-1/1-0051/1-0051");
        fs::write(nv.join("nvmem"), ddr5_spd()).expect("nvmem");
        let hw = mk("sys/class/hwmon/hwmon0");
        fs::write(hw.join("name"), "spd5118\n").expect("name");
        fs::write(hw.join("temp1_input"), "43500\n").expect("temp");
        symlink("../../../devices/platform/i2c-1/1-0051", hw.join("device")).expect("symlink");
        // An unrelated hwmon that must be ignored.
        let other = mk("sys/class/hwmon/hwmon1");
        fs::write(other.join("name"), "k10temp\n").expect("name");
        let e0 = mk("sys/firmware/dmi/entries/17-0");
        fs::write(e0.join("raw"), ddr5_record()).expect("raw");
        let e1 = mk("sys/firmware/dmi/entries/17-1");
        fs::write(e1.join("raw"), T17::new(0x5C, 0x41).s(0x10, "DIMM_B1").build()).expect("raw");
        mk("sys/firmware/dmi/entries/0-0");

        let sys = Sys::new(&root);
        let mut mem = Mem::detect(&sys);
        mem.sample();
        let r = mem.reading();
        assert_eq!(r["total_mib"], 16384);
        assert_eq!(r["available_mib"], 8192);
        assert_eq!(r["used_mib"], 8192);
        assert_eq!(r["used_pct"], 50.0);
        assert_eq!(r["cached_mib"], 4096);
        assert_eq!(r["swap_total_mib"], 4096);
        assert_eq!(r["swap_used_mib"], 1024);
        assert_eq!(r["dimm_temps"][0]["dimm"], "DIMM 1");
        assert_eq!(r["dimm_temps"][0]["slot"], 1);
        assert_eq!(r["dimm_temps"][0]["temp"], 43.5);
        assert_eq!(r["dimm_temps"].as_array().map(Vec::len), Some(1));

        let i = mem.info();
        assert_eq!(i["note"], NOTE);
        assert_eq!(i["smbios_available"], true);
        assert_eq!(i["slots_total"], 2);
        assert_eq!(i["slots_populated"], 1);
        assert_eq!(i["total_installed_mib"], 32768);
        assert_eq!(i["dimms"][0]["part_number"], "KF560C36-32");
        assert_eq!(i["dimms"][1]["populated"], false);
        assert_eq!(i["spd"][0]["part_number"], "KF560C36BBEK2-32");
        assert_eq!(i["spd"][0]["slot"], 1);
        assert_eq!(i["spd"][0]["expo"]["profiles"][0]["speed_mts"], 6000);
        assert_eq!(i["summary"], "1 x 32 GiB DDR5, rated 6000 MT/s, running 4800 MT/s, 1/2 slots populated");

        // Sensor disappearing at runtime: reading degrades to null, no panic.
        fs::remove_file(hw.join("temp1_input")).expect("rm");
        mem.sample();
        assert_eq!(mem.reading()["dimm_temps"][0]["temp"], Value::Null);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_tree_degrades() {
        let sys = Sys::new("/nonexistent/omarchy-oc-mem");
        let mut mem = Mem::detect(&sys);
        mem.sample();
        let r = mem.reading();
        assert_eq!(r["total_mib"], 0);
        assert_eq!(r["used_pct"], 0.0);
        let i = mem.info();
        assert_eq!(i["slots_total"], 0);
        assert_eq!(i["slots_populated"], Value::Null);
        assert_eq!(i["smbios_available"], false);
        assert_eq!(i["total_installed_mib"], Value::Null);
    }
}
