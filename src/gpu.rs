//! Intel Arc GPUs on the `xe` driver: identity, live readings, per-process
//! usage, and the tunable limits (frequency range, PL1 power limit and
//! window, power profile). Fan control goes through omarchy-fans.

use crate::tune::{GpuTune, Write};
use crate::util::{Sys, read_num, read_trim};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub struct Gpu {
    pub sys: Sys,
    pub pci: String,
    /// Real sysfs device dir (/sys/devices/...).
    dev: PathBuf,
    pub card: String,
    freq: PathBuf,
    gt: PathBuf,
    hwmon: Option<PathBuf>,
    pub ident: Value,
    last_energy: Option<(f64, u64)>,
    last_idle: Option<(f64, u64)>,
    last_clients: BTreeMap<String, (f64, BTreeMap<String, (u64, u64)>)>,
    last_fdinfo: f64,
    pub reading: Value,
    pub procs: Vec<Value>,
    pub vram_used_mib: Option<f64>,
}

fn device_name(dev: u32) -> &'static str {
    match dev {
        0x56a0 => "Arc A770",
        0x56a1 => "Arc A750",
        0x56a2 => "Arc A580",
        0x56a5 => "Arc A380",
        0x56a6 => "Arc A310",
        0x5690..=0x5697 => "Arc A-series Mobile",
        0x56b0..=0x56b3 => "Arc Pro A-series",
        0xe20b => "Arc B580",
        0xe20c => "Arc B570",
        0xe211 | 0xe212 | 0xe222 | 0xe223 => "Arc Pro B-series",
        _ => "Intel GPU",
    }
}

fn vendor_name(v: u32) -> String {
    match v {
        0x8086 => "Intel".into(),
        0x1849 => "ASRock".into(),
        0x1043 => "ASUS".into(),
        0x1458 => "Gigabyte".into(),
        0x1462 => "MSI".into(),
        0x172f => "Acer".into(),
        0x1ef7 | 0x1bf8 => "Sparkle".into(),
        0x1da2 => "Sapphire".into(),
        0x196e => "PNY".into(),
        0x19da => "Zotac".into(),
        0x1b4c => "Gunnir".into(),
        x => format!("0x{x:04x}"),
    }
}

fn hex(p: &Path) -> u32 {
    read_trim(p).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0)
}

/// Sorted entries of an already-real (root-prefixed) directory.
fn list_real(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

impl Gpu {
    pub fn detect(sys: &Sys) -> Vec<Gpu> {
        let mut out = Vec::new();
        for entry in sys.list("/sys/bus/pci/drivers/xe") {
            let Some(name) = entry.file_name().map(|n| n.to_string_lossy().to_string()) else { continue };
            if !name.contains(':') {
                continue;
            }
            let dev = std::fs::canonicalize(&entry).unwrap_or(entry.clone());
            let card = list_real(&dev.join("drm"))
                .into_iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .find(|n| n.starts_with("card"))
                .unwrap_or_default();
            let gt = dev.join("tile0/gt0");
            let freq = gt.join("freq0");
            let hwmon = list_real(&dev.join("hwmon")).into_iter().next();
            let mut g = Gpu {
                sys: sys.clone(),
                pci: name,
                dev,
                card,
                freq,
                gt,
                hwmon,
                ident: Value::Null,
                last_energy: None,
                last_idle: None,
                last_clients: BTreeMap::new(),
                last_fdinfo: 0.0,
                reading: Value::Null,
                procs: Vec::new(),
                vram_used_mib: None,
            };
            g.ident = g.identify();
            out.push(g);
        }
        out
    }

    fn identify(&self) -> Value {
        let s = &self.sys;
        let d = &self.dev;
        let device = hex(&d.join("device"));
        let subven = hex(&d.join("subsystem_vendor"));
        let subdev = hex(&d.join("subsystem_device"));
        // BAR2 is the VRAM aperture; with Resizable BAR it covers all VRAM.
        let bars: Vec<(u64, u64)> = read_trim(&d.join("resource"))
            .unwrap_or_default()
            .lines()
            .map(|l| {
                let f: Vec<u64> = l.split_whitespace().filter_map(|x| u64::from_str_radix(x.trim_start_matches("0x"), 16).ok()).collect();
                (f.first().copied().unwrap_or(0), f.get(1).copied().unwrap_or(0))
            })
            .collect();
        let bar2 = bars.get(2).filter(|(a, b)| *b > *a).map(|(a, b)| b - a + 1).unwrap_or(0);
        let bar2_gib = bar2 as f64 / (1u64 << 30) as f64;
        // The card sits behind its own PCIe switch; the slot link is the
        // ancestor directly under the root port.
        let ancestors: Vec<PathBuf> = d.ancestors().skip(1).take_while(|p| p.file_name().is_some_and(|n| n.to_string_lossy().contains(':'))).map(Path::to_path_buf).collect();
        let slot = ancestors.iter().rev().nth(1).cloned().unwrap_or_else(|| d.clone());
        let link = |p: &Path| {
            json!({
                "speed": read_trim(&p.join("current_link_speed")),
                "width": read_num::<u32>(&p.join("current_link_width")),
                "max_speed": read_trim(&p.join("max_link_speed")),
                "max_width": read_num::<u32>(&p.join("max_link_width")),
            })
        };
        // GSC firmware version from the MEI device that hangs off this GPU.
        let gsc = s.list("/sys/class/mei").into_iter().find_map(|m| {
            let real = std::fs::canonicalize(m.join("device")).ok()?;
            if !real.starts_with(d) {
                return None;
            }
            let fw = read_trim(&m.join("fw_ver"))?;
            fw.lines().map(|l| l.split_once(':').map(|(_, v)| v).unwrap_or(l).to_string()).find(|v| v != "0.0.0.0")
        });
        let kernel = s.read("/proc/sys/kernel/osrelease").unwrap_or_default();
        json!({
            "name": device_name(device),
            "vendor": vendor_name(subven),
            "pci": self.pci,
            "card": self.card,
            "device_id": format!("{:04x}", device),
            "subsystem": format!("{:04x}:{:04x}", subven, subdev),
            "driver": "xe",
            "kernel": kernel,
            "gsc_firmware": gsc,
            "vram_bar_gib": crate::util::round1(bar2_gib),
            "resizable_bar": bar2_gib >= 2.0,
            "pcie_link": link(&slot),
            "rp0_mhz": read_num::<u32>(&self.freq.join("rp0_freq")),
            "rpe_mhz": read_num::<u32>(&self.freq.join("rpe_freq")),
            "rpn_mhz": read_num::<u32>(&self.freq.join("rpn_freq")),
            "engines": list_real(&self.gt.join("engines")).iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string())).collect::<Vec<_>>(),
        })
    }

    fn hw(&self, label: &str, kind: &str) -> Option<PathBuf> {
        let h = self.hwmon.as_ref()?;
        for i in 1..=8 {
            let l = h.join(format!("{kind}{i}_label"));
            if read_trim(&l).as_deref() == Some(label) {
                return Some(h.join(format!("{kind}{i}")));
            }
        }
        None
    }

    fn hwfile(&self, label: &str, kind: &str, suffix: &str) -> Option<PathBuf> {
        self.hw(label, kind).map(|p| PathBuf::from(format!("{}{suffix}", p.display())))
    }

    pub fn sample(&mut self, t: f64, scan_procs: bool) {
        let freq = self.freq.clone();
        let f = |n: &str| read_num::<f64>(&freq.join(n));
        let (temp_pkg, temp_vram) = {
            let temp = |l: &str| self.hwfile(l, "temp", "_input").and_then(|p| read_num::<f64>(&p)).map(|m| m / 1000.0);
            (temp("pkg"), temp("vram"))
        };
        let energy = self.hwfile("pkg", "energy", "_input").or_else(|| self.hwfile("card", "energy", "_input")).and_then(|p| read_num::<u64>(&p));
        let mut power = None;
        if let (Some(e), Some((t0, e0))) = (energy, self.last_energy) {
            if t - t0 > 0.05 && e >= e0 {
                power = Some(crate::util::round1((e - e0) as f64 / 1e6 / (t - t0)));
            }
        }
        self.last_energy = energy.map(|e| (t, e));
        let idle_ms = read_num::<u64>(&self.gt.join("gtidle/idle_residency_ms"));
        let mut busy = None;
        if let (Some(i), Some((t0, i0))) = (idle_ms, self.last_idle) {
            let dt = (t - t0) * 1000.0;
            if dt > 50.0 && i >= i0 {
                busy = Some(crate::util::round1((100.0 * (1.0 - (i - i0) as f64 / dt)).clamp(0.0, 100.0)));
            }
        }
        self.last_idle = idle_ms.map(|i| (t, i));
        let fans: Vec<Value> = (1..=3)
            .filter_map(|i| {
                let h = self.hwmon.as_ref()?;
                let rpm = read_num::<u64>(&h.join(format!("fan{i}_input")))?;
                Some(json!({"fan": i, "rpm": rpm}))
            })
            .collect();
        let throttle = self.freq.join("throttle");
        let reasons: Vec<String> = read_trim(&throttle.join("reasons"))
            .map(|s| s.split_whitespace().filter(|w| *w != "none").map(String::from).collect())
            .unwrap_or_default();
        if scan_procs && t - self.last_fdinfo >= 1.9 {
            self.last_fdinfo = t;
            self.scan_fdinfo(t);
        }
        self.reading = json!({
            "act_mhz": f("act_freq"),
            "cur_mhz": f("cur_freq"),
            "temp": temp_pkg,
            "vram_temp": temp_vram,
            "power_w": power,
            "voltage_mv": self.hwfile("pkg", "in", "_input").and_then(|p| read_num::<f64>(&p)),
            "busy": busy,
            "fans": fans,
            "throttled": read_trim(&throttle.join("status")).as_deref() == Some("1"),
            "throttle_reasons": reasons,
            "vram_used_mib": self.vram_used_mib,
            "idle": read_trim(&self.gt.join("gtidle/idle_status")),
        });
    }

    /// Per-process VRAM and engine use from DRM fdinfo (root sees all).
    fn scan_fdinfo(&mut self, t: f64) {
        let sys = self.sys.clone();
        let mut procs: BTreeMap<u32, (String, f64, BTreeMap<String, f64>)> = BTreeMap::new();
        let mut seen_clients = BTreeMap::new();
        let mut vram_total = 0.0;
        let mut any = false;
        for pdir in sys.list("/proc") {
            let Some(pid) = pdir.file_name().and_then(|n| n.to_str()).and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(fds) = std::fs::read_dir(pdir.join("fd")) else { continue };
            for fd in fds.flatten() {
                let Ok(target) = std::fs::read_link(fd.path()) else { continue };
                if !target.to_string_lossy().starts_with("/dev/dri/") {
                    continue;
                }
                let Ok(info) = std::fs::read_to_string(pdir.join("fdinfo").join(fd.file_name())) else { continue };
                let kv: BTreeMap<&str, &str> = info.lines().filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim(), v.trim())).collect();
                if kv.get("drm-pdev") != Some(&self.pci.as_str()) {
                    continue;
                }
                let Some(client) = kv.get("drm-client-id").map(|s| s.to_string()) else { continue };
                if seen_clients.contains_key(&client) {
                    continue;
                }
                any = true;
                let vram = kv.get("drm-total-vram0").and_then(|v| parse_mem(v)).unwrap_or(0.0);
                vram_total += vram;
                let mut cycles = BTreeMap::new();
                for (k, v) in &kv {
                    if let Some(eng) = k.strip_prefix("drm-cycles-") {
                        let total = kv.get(format!("drm-total-cycles-{eng}").as_str()).and_then(|x| x.parse::<u64>().ok()).unwrap_or(0);
                        cycles.insert(eng.to_string(), (v.parse::<u64>().unwrap_or(0), total));
                    }
                }
                let mut usage = BTreeMap::new();
                if let Some((_, prev)) = self.last_clients.get(&client) {
                    for (eng, (c1, tot1)) in &cycles {
                        if let Some((c0, tot0)) = prev.get(eng) {
                            let dt = tot1.saturating_sub(*tot0) as f64;
                            let cap = kv.get(format!("drm-engine-capacity-{eng}").as_str()).and_then(|x| x.parse::<f64>().ok()).unwrap_or(1.0);
                            if dt > 0.0 {
                                usage.insert(eng.clone(), (100.0 * c1.saturating_sub(*c0) as f64 / dt / cap).min(100.0));
                            }
                        }
                    }
                }
                seen_clients.insert(client, (t, cycles));
                let comm = read_trim(&pdir.join("comm")).unwrap_or_default();
                let e = procs.entry(pid).or_insert((comm, 0.0, BTreeMap::new()));
                e.1 += vram;
                for (eng, u) in usage {
                    *e.2.entry(eng).or_insert(0.0) += u;
                }
            }
        }
        self.last_clients = seen_clients;
        self.vram_used_mib = any.then(|| (vram_total / 1048576.0).round());
        let mut list: Vec<Value> = procs
            .into_iter()
            .map(|(pid, (comm, vram, usage))| {
                let busiest = usage.values().cloned().fold(0.0, f64::max);
                json!({"pid": pid, "name": comm, "vram_mib": (vram / 1048576.0).round(),
                       "engines": usage.into_iter().map(|(k, v)| (k, crate::util::round1(v))).collect::<BTreeMap<_, _>>(),
                       "busy": crate::util::round1(busiest)})
            })
            .collect();
        list.retain(|p| p["vram_mib"].as_f64().unwrap_or(0.0) >= 1.0 || p["busy"].as_f64().unwrap_or(0.0) > 0.0);
        list.sort_by(|a, b| {
            let k = |v: &Value| v["busy"].as_f64().unwrap_or(0.0) * 1e6 + v["vram_mib"].as_f64().unwrap_or(0.0);
            k(b).partial_cmp(&k(a)).unwrap_or(std::cmp::Ordering::Equal)
        });
        list.truncate(12);
        self.procs = list;
    }

    pub fn knobs(&self) -> Value {
        let f = |n: &str| read_num::<u32>(&self.freq.join(n));
        let profiles = read_trim(&self.freq.join("power_profile")).unwrap_or_default();
        let active = profiles.split_whitespace().find(|w| w.starts_with('[')).map(|w| w.trim_matches(['[', ']']).to_string());
        let choices: Vec<String> = profiles.split_whitespace().map(|w| w.trim_matches(['[', ']']).to_string()).collect();
        let pl = self.hwfile("pkg", "power", "_max").or_else(|| self.hwfile("card", "power", "_max"));
        json!({
            "min_mhz": f("min_freq"),
            "max_mhz": f("max_freq"),
            "rpn_mhz": f("rpn_freq"),
            "rpe_mhz": f("rpe_freq"),
            "rp0_mhz": f("rp0_freq"),
            "power_w": pl.as_ref().and_then(|p| read_num::<f64>(p)).map(|u| crate::util::round1(u / 1e6)),
            "power_window_ms": pl.as_ref().and_then(|p| read_num::<u32>(&PathBuf::from(format!("{}_interval", p.display())))),
            "power_crit_w": self.hwfile("pkg", "power", "_crit").and_then(|p| read_num::<f64>(&p)).map(|u| crate::util::round1(u / 1e6)),
            "power_profile": active,
            "power_profiles": choices,
        })
    }

    fn real(&self, p: &Path) -> String {
        // Paths are real sysfs paths already (built from the canonical
        // device dir); strip the test root back off for Write.
        let root = &self.sys.root;
        match p.strip_prefix(root) {
            Ok(rel) if root != Path::new("/") => format!("/{}", rel.display()),
            _ => p.display().to_string(),
        }
    }

    pub fn plan(&self, t: &GpuTune) -> Vec<Write> {
        let mut w = Vec::new();
        let minp = self.real(&self.freq.join("min_freq"));
        let maxp = self.real(&self.freq.join("max_freq"));
        w.push(Write::new(minp.clone(), t.min_mhz.map(|v| v.to_string())).pair(maxp.clone()));
        w.push(Write::new(maxp.clone(), t.max_mhz.map(|v| v.to_string())).pair(minp));
        if let Some(pl) = self.hwfile("pkg", "power", "_max").or_else(|| self.hwfile("card", "power", "_max")) {
            w.push(Write::new(self.real(&pl), t.power_w.map(|v| ((v * 1e6).round() as u64).to_string())));
            let iv = PathBuf::from(format!("{}_interval", pl.display()));
            if iv.exists() {
                w.push(Write::new(self.real(&iv), t.power_window_ms.map(|v| v.to_string())));
            }
        }
        let pp = self.freq.join("power_profile");
        if pp.exists() {
            w.push(Write::new(self.real(&pp), t.power_profile.clone()));
        }
        w
    }

    pub fn knob_paths(&self) -> Vec<String> {
        self.plan(&GpuTune::default()).into_iter().map(|w| w.path).collect()
    }

    pub fn stock_power_w(&self, originals: &BTreeMap<String, String>) -> Option<f64> {
        let pl = self.hwfile("pkg", "power", "_max").or_else(|| self.hwfile("card", "power", "_max"))?;
        let v = originals.get(&self.real(&pl)).and_then(|s| s.parse::<f64>().ok()).or_else(|| read_num::<f64>(&pl))?;
        Some(v / 1e6)
    }
}

/// fdinfo memory values: "123", "123 KiB", "4 MiB".
fn parse_mem(v: &str) -> Option<f64> {
    let mut it = v.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let mul = match it.next() {
        Some("KiB") => 1024.0,
        Some("MiB") => 1048576.0,
        Some("GiB") => 1073741824.0,
        _ => 1.0,
    };
    Some(n * mul)
}

#[cfg(test)]
mod tests {
    #[test]
    fn mem_units() {
        assert_eq!(super::parse_mem("4 MiB"), Some(4194304.0));
        assert_eq!(super::parse_mem("12"), Some(12.0));
    }
}
