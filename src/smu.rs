//! AMD SMU (System Management Unit) mailbox: firmware power limits and
//! Curve Optimizer on Ryzen APUs, written in Rust from the protocol that
//! FlyGoat/RyzenAdj documents (LGPL; no code copied).
//!
//! Access goes through the SMN index/data pair in the host bridge's PCI
//! config space (0000:00:00.0, offsets 0xB8/0xBC), so it needs no kernel
//! module: just root and a kernel that is not locked down.
//!
//! Supported: Phoenix / Phoenix 2 / Hawk Point (family 19h models 0x74,
//! 0x75, 0x78), which covers Ryzen 7040/8040 mobile and 8000G desktop APUs.
//! Every value set here is volatile: the SMU forgets it on reboot.

use crate::tune::SmuTune;
use crate::util::Sys;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NB_ADDR: u64 = 0xB8;
const NB_DATA: u64 = 0xBC;

#[derive(Clone, Copy, Debug)]
struct Mailbox {
    msg: u32,
    rsp: u32,
    arg: u32,
}

const MP1: Mailbox = Mailbox { msg: 0x3B10528, rsp: 0x3B10578, arg: 0x3B10998 };
const PSMU: Mailbox = Mailbox { msg: 0x3B10A20, rsp: 0x3B10A80, arg: 0x3B10A88 };

pub const RESP_OK: u32 = 0x1;

pub fn resp_name(r: u32) -> &'static str {
    match r {
        0x1 => "ok",
        0xFF => "failed",
        0xFE => "unknown command",
        0xFD => "rejected (prerequisite)",
        0xFC => "rejected (busy)",
        0 => "timeout",
        _ => "unexpected response",
    }
}

/// MP1 commands on Phoenix-family firmware.
pub mod cmd {
    pub const STAPM_LIMIT: u32 = 0x14;
    pub const FAST_LIMIT: u32 = 0x15;
    pub const SLOW_LIMIT: u32 = 0x16;
    pub const TCTL_TEMP: u32 = 0x19;
    pub const VRM_CURRENT: u32 = 0x1A;
    pub const VRMSOC_CURRENT: u32 = 0x1B;
    pub const VRMMAX_CURRENT: u32 = 0x1C;
    pub const VRMSOCMAX_CURRENT: u32 = 0x1D;
    pub const CO_PER_CORE: u32 = 0x4B;
    pub const CO_ALL: u32 = 0x4C;
    // PSMU
    pub const TABLE_VERSION: u32 = 0x6;
    pub const TRANSFER_TABLE: u32 = 0x65;
    pub const TABLE_ADDRESS: u32 = 0x66;
}

/// Which CPUs this module knows the mailbox layout for.
pub fn supported(vendor: &str, family: u32, model: u32) -> Option<&'static str> {
    if vendor != "AuthenticAMD" || family != 0x19 {
        return None;
    }
    match model {
        0x74 | 0x78 => Some("Phoenix"),
        0x75 => Some("Hawk Point"),
        _ => None,
    }
}

pub struct Smn {
    cfg: File,
}

impl Smn {
    pub fn open(sys: &Sys) -> std::io::Result<Smn> {
        let p: PathBuf = sys.p("/sys/bus/pci/devices/0000:00:00.0/config");
        Ok(Smn { cfg: OpenOptions::new().read(true).write(true).open(p)? })
    }

    pub fn read(&self, addr: u32) -> std::io::Result<u32> {
        self.cfg.write_all_at(&addr.to_le_bytes(), NB_ADDR)?;
        let mut b = [0u8; 4];
        self.cfg.read_exact_at(&mut b, NB_DATA)?;
        Ok(u32::from_le_bytes(b))
    }

    pub fn write(&self, addr: u32, val: u32) -> std::io::Result<()> {
        self.cfg.write_all_at(&addr.to_le_bytes(), NB_ADDR)?;
        self.cfg.write_all_at(&val.to_le_bytes(), NB_DATA)
    }
}

pub struct Smu {
    smn: Smn,
    pub family: String,
}

/// Values read back from the SMU's power-metrics table. `None` where the
/// table version is unknown.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PmLimits {
    pub table_version: u32,
    pub stapm_limit: Option<f32>,
    pub stapm_value: Option<f32>,
    pub fast_limit: Option<f32>,
    pub fast_value: Option<f32>,
    pub slow_limit: Option<f32>,
    pub slow_value: Option<f32>,
    pub tdc_limit: Option<f32>,
    pub tdc_value: Option<f32>,
    pub soc_tdc_limit: Option<f32>,
    pub soc_tdc_value: Option<f32>,
    pub edc_limit: Option<f32>,
    pub edc_value: Option<f32>,
    pub soc_edc_limit: Option<f32>,
    pub soc_edc_value: Option<f32>,
    pub tctl_limit: Option<f32>,
    pub tctl_value: Option<f32>,
}

impl PmLimits {
    pub fn parse(version: u32, t: &[u8]) -> PmLimits {
        let f = |off: usize| -> Option<f32> {
            let b = t.get(off..off + 4)?;
            let v = f32::from_le_bytes(b.try_into().ok()?);
            v.is_finite().then_some(v)
        };
        let mut p = PmLimits { table_version: version, ..Default::default() };
        // Offsets for the Phoenix/Hawk Point table family (0x4C000x).
        if version >> 16 == 0x4C {
            p.stapm_limit = f(0x0);
            p.stapm_value = f(0x4);
            p.fast_limit = f(0x8);
            p.fast_value = f(0xC);
            p.slow_limit = f(0x10);
            p.slow_value = f(0x14);
            p.tdc_limit = f(0x20);
            p.tdc_value = f(0x24);
            p.soc_tdc_limit = f(0x28);
            p.soc_tdc_value = f(0x2C);
            p.edc_limit = f(0x30);
            p.edc_value = f(0x34);
            p.soc_edc_limit = f(0x38);
            p.soc_edc_value = f(0x3C);
            p.tctl_limit = f(0x40);
            p.tctl_value = f(0x44);
        }
        p
    }
}

fn table_size(version: u32) -> usize {
    match version {
        0x4C0003 => 0xB18,
        0x4C0004 => 0xB1C,
        0x4C0005 => 0xAF8,
        0x4C0006 => 0xAFC,
        0x4C0008 => 0xAF0,
        0x4C0007 | 0x4C0009 => 0xB00,
        _ => 0x1000,
    }
}

impl Smu {
    /// Open the mailbox and check it answers the test message.
    pub fn open(sys: &Sys, family: &str) -> Result<Smu, String> {
        let smn = Smn::open(sys).map_err(|e| format!("host bridge config space: {e}"))?;
        let smu = Smu { smn, family: family.to_string() };
        smu.smn.write(MP1.arg, 0x47).map_err(|e| e.to_string())?;
        if smu.smn.read(MP1.arg).map_err(|e| e.to_string())? != 0x47 {
            return Err("SMN is not writable (kernel lockdown?)".into());
        }
        let (r, _) = smu.send(MP1, 0x1, [0; 6]);
        if r != RESP_OK {
            return Err(format!("SMU test message: {}", resp_name(r)));
        }
        Ok(smu)
    }

    fn send(&self, mb: Mailbox, id: u32, args: [u32; 6]) -> (u32, [u32; 6]) {
        let go = || -> std::io::Result<(u32, [u32; 6])> {
            // Wait for any earlier message to finish.
            let t0 = Instant::now();
            while self.smn.read(mb.rsp)? == 0 && t0.elapsed() < Duration::from_millis(100) {
                std::thread::sleep(Duration::from_micros(200));
            }
            self.smn.write(mb.rsp, 0)?;
            for (i, a) in args.iter().enumerate() {
                self.smn.write(mb.arg + 4 * i as u32, *a)?;
            }
            self.smn.write(mb.msg, id)?;
            let t0 = Instant::now();
            let mut r = 0;
            while r == 0 && t0.elapsed() < Duration::from_millis(500) {
                r = self.smn.read(mb.rsp)?;
                if r == 0 {
                    std::thread::sleep(Duration::from_micros(100));
                }
            }
            let mut out = [0u32; 6];
            for (i, o) in out.iter_mut().enumerate() {
                *o = self.smn.read(mb.arg + 4 * i as u32)?;
            }
            Ok((r, out))
        };
        go().unwrap_or((0, [0; 6]))
    }

    pub fn mp1(&self, id: u32, arg0: u32) -> Result<(), String> {
        let (r, _) = self.send(MP1, id, [arg0, 0, 0, 0, 0, 0]);
        if r == RESP_OK { Ok(()) } else { Err(format!("SMU command 0x{id:X}: {}", resp_name(r))) }
    }

    /// Read the power-metrics table (needs /dev/mem access to the table's
    /// physical address, which is firmware-reserved memory).
    pub fn read_table(&self, sys: &Sys) -> Result<PmLimits, String> {
        let (r, a) = self.send(PSMU, cmd::TABLE_VERSION, [0; 6]);
        if r != RESP_OK {
            return Err(format!("table version: {}", resp_name(r)));
        }
        let version = a[0];
        let (r, a) = self.send(PSMU, cmd::TABLE_ADDRESS, [0; 6]);
        if r != RESP_OK {
            return Err(format!("table address: {}", resp_name(r)));
        }
        let addr = (a[1] as u64) << 32 | a[0] as u64;
        if addr == 0 {
            return Err("SMU returned no table address".into());
        }
        let mut r = self.send(PSMU, cmd::TRANSFER_TABLE, [0; 6]).0;
        for _ in 0..2 {
            if r != 0xFD {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
            r = self.send(PSMU, cmd::TRANSFER_TABLE, [0; 6]).0;
        }
        if r != RESP_OK {
            return Err(format!("table transfer: {}", resp_name(r)));
        }
        let size = table_size(version);
        let mut buf = vec![0u8; size];
        let mem = File::open(sys.p("/dev/mem")).map_err(|e| format!("/dev/mem: {e}"))?;
        mem.read_exact_at(&mut buf, addr).map_err(|e| format!("/dev/mem at 0x{addr:x}: {e}"))?;
        Ok(PmLimits::parse(version, &buf))
    }

    /// Send every set field of `t`. For fields set before (in `prev`) but
    /// unset now, send the stock value from `stock` (or 0 for the Curve
    /// Optimizer). Returns (sent, errors, needs_reboot) where needs_reboot
    /// lists limits that could not be restored because stock is unknown.
    pub fn apply(&self, t: &SmuTune, prev: &SmuTune, stock: Option<&PmLimits>) -> (Vec<String>, Vec<String>, Vec<String>) {
        let mut sent = Vec::new();
        let mut errs = Vec::new();
        let mut reboot = Vec::new();
        let st = |f: fn(&PmLimits) -> Option<f32>| stock.and_then(f);
        let mut lim = |name: &str, id: u32, want: Option<u32>, was: Option<u32>, stockv: Option<f32>, scale: f32| {
            let arg = match (want, was) {
                (Some(v), _) => Some((v as f32 * scale) as u32),
                (None, Some(_)) => match stockv {
                    Some(s) if s > 0.0 => Some((s * scale).round() as u32),
                    _ => {
                        reboot.push(name.to_string());
                        None
                    }
                },
                (None, None) => None,
            };
            if let Some(arg) = arg {
                match self.mp1(id, arg) {
                    Ok(()) => sent.push(format!("{name}={arg}")),
                    Err(e) => errs.push(format!("{name}: {e}")),
                }
            }
        };
        lim("stapm", cmd::STAPM_LIMIT, t.stapm_w, prev.stapm_w, st(|p| p.stapm_limit), 1000.0);
        lim("fast", cmd::FAST_LIMIT, t.fast_w, prev.fast_w, st(|p| p.fast_limit), 1000.0);
        lim("slow", cmd::SLOW_LIMIT, t.slow_w, prev.slow_w, st(|p| p.slow_limit), 1000.0);
        lim("tctl", cmd::TCTL_TEMP, t.tctl_c, prev.tctl_c, st(|p| p.tctl_limit), 1.0);
        lim("tdc", cmd::VRM_CURRENT, t.tdc_a, prev.tdc_a, st(|p| p.tdc_limit), 1000.0);
        lim("soc_tdc", cmd::VRMSOC_CURRENT, t.soc_tdc_a, prev.soc_tdc_a, st(|p| p.soc_tdc_limit), 1000.0);
        lim("edc", cmd::VRMMAX_CURRENT, t.edc_a, prev.edc_a, st(|p| p.edc_limit), 1000.0);
        lim("soc_edc", cmd::VRMSOCMAX_CURRENT, t.soc_edc_a, prev.soc_edc_a, st(|p| p.soc_edc_limit), 1000.0);

        // Curve Optimizer: all-core first, then per-core overrides. Any core
        // that had a per-core value and no longer does falls back to co_all.
        let co_changed = t.co_all != prev.co_all || t.co_cores != prev.co_cores;
        if co_changed {
            let all = t.co_all.unwrap_or(0);
            match self.mp1(cmd::CO_ALL, co_arg(all)) {
                Ok(()) => sent.push(format!("co_all={all}")),
                Err(e) => errs.push(format!("co_all: {e}")),
            }
            let mut cores: Vec<u32> = t.co_cores.keys().chain(prev.co_cores.keys()).copied().collect();
            cores.sort();
            cores.dedup();
            for core in cores {
                let v = t.co_cores.get(&core).copied().unwrap_or(all);
                match self.mp1(cmd::CO_PER_CORE, co_core_arg(core, v)) {
                    Ok(()) => sent.push(format!("co[{core}]={v}")),
                    Err(e) => errs.push(format!("co[{core}]: {e}")),
                }
            }
        }
        (sent, errs, reboot)
    }
}

/// Curve Optimizer argument: a 20-bit two's-complement count.
pub fn co_arg(v: i32) -> u32 {
    (v as u32) & 0xFFFFF
}

/// Per-core: core index in bits 20+ (CCD in the high nibble on multi-CCD
/// parts; APUs have one), value in the low 20 bits.
pub fn co_core_arg(core: u32, v: i32) -> u32 {
    let ccd = core / 8;
    let idx = core % 8;
    (((ccd << 4) | idx) << 20) | co_arg(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn co_encoding() {
        assert_eq!(co_arg(-20), 0xFFFEC);
        assert_eq!(co_arg(0), 0);
        assert_eq!(co_arg(5), 5);
        assert_eq!(co_core_arg(3, -10), (3 << 20) | 0xFFFF6);
        assert_eq!(co_core_arg(9, -1), ((1 << 4 | 1) << 20) | 0xFFFFF);
    }

    #[test]
    fn table_parse() {
        let mut t = vec![0u8; 0x100];
        let put = |t: &mut Vec<u8>, off: usize, v: f32| t[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut t, 0x0, 65.0);
        put(&mut t, 0x8, 88.0);
        put(&mut t, 0x40, 95.0);
        let p = PmLimits::parse(0x4C0006, &t);
        assert_eq!(p.stapm_limit, Some(65.0));
        assert_eq!(p.fast_limit, Some(88.0));
        assert_eq!(p.tctl_limit, Some(95.0));
        let q = PmLimits::parse(0x370000, &t);
        assert_eq!(q.stapm_limit, None);
    }

    #[test]
    fn support() {
        assert_eq!(supported("AuthenticAMD", 0x19, 0x78), Some("Phoenix"));
        assert_eq!(supported("AuthenticAMD", 0x19, 0x61), None);
        assert_eq!(supported("GenuineIntel", 6, 0x78), None);
    }
}
