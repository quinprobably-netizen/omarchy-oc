//! CPU: topology, live readings and the kernel-side tuning knobs
//! (amd-pstate mode, governor, EPP, boost, frequency limits, SMT, C-states).
//! Firmware-side limits (PPT, Curve Optimizer) live in `smu.rs`.

use crate::tune::{CpuTune, Write};
use crate::util::{Sys, read_num, read_trim};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

const CPU: &str = "/sys/devices/system/cpu";

#[derive(Clone, Debug, Serialize)]
pub struct Thread {
    pub cpu: u32,
    pub core: u32,
    /// "fast" (full Zen core) or "dense" (compact core, lower boost), from
    /// the CPPC preferred-core ranking. Everything is "fast" on a uniform CPU.
    pub tier: String,
    pub ranking: u32,
    pub online: bool,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CpuIdent {
    pub model: String,
    pub vendor: String,
    pub family: u32,
    pub model_id: u32,
    pub stepping: u32,
    pub microcode: String,
    pub cores: u32,
    pub threads: u32,
    pub caches: Vec<String>,
    pub driver: String,
    pub flags_of_note: Vec<String>,
}

pub struct Cpu {
    pub sys: Sys,
    pub ident: CpuIdent,
    pub threads: Vec<Thread>,
    rapl_pkg: Option<PathBuf>,
    rapl_core: Option<PathBuf>,
    tctl: Option<PathBuf>,
    last_stat: BTreeMap<u32, (u64, u64)>,
    last_energy: Option<(f64, u64, u64)>,
    pub reading: CpuReading,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct CpuReading {
    pub temp: Option<f64>,
    pub package_w: Option<f64>,
    pub core_w: Option<f64>,
    pub util: f64,
    pub mhz_avg: f64,
    pub mhz_max: f64,
    pub threads: Vec<ThreadReading>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub struct ThreadReading {
    pub cpu: u32,
    pub mhz: Option<f64>,
    pub util: f64,
}

impl Cpu {
    pub fn detect(sys: &Sys) -> Cpu {
        let ident = ident(sys);
        let threads = threads(sys);
        let rapl = |name: &str| {
            sys.list("/sys/class/powercap")
                .into_iter()
                .filter(|p| p.join("energy_uj").exists())
                .find(|p| read_trim(&p.join("name")).as_deref() == Some(name))
        };
        let tctl = crate::util::hwmon_by_name(sys, "k10temp")
            .or_else(|| crate::util::hwmon_by_name(sys, "zenpower"))
            .or_else(|| crate::util::hwmon_by_name(sys, "coretemp"))
            .map(|h| h.join("temp1_input"));
        Cpu {
            sys: sys.clone(),
            ident,
            threads,
            rapl_pkg: rapl("package-0"),
            rapl_core: rapl("core"),
            tctl,
            last_stat: BTreeMap::new(),
            last_energy: None,
            reading: CpuReading::default(),
        }
    }

    pub fn ncpu(&self) -> u32 {
        self.threads.len() as u32
    }

    pub fn sample(&mut self, t: f64) {
        let sys = &self.sys;
        let mut r = CpuReading {
            temp: self.tctl.as_ref().and_then(|p| read_num::<f64>(p)).map(|m| m / 1000.0),
            ..Default::default()
        };
        // Utilisation from /proc/stat deltas.
        let stat = proc_stat(sys);
        let mut utils: BTreeMap<u32, f64> = BTreeMap::new();
        for (cpu, (b1, i1)) in &stat {
            if let Some((b0, i0)) = self.last_stat.get(cpu) {
                let (db, di) = (b1.saturating_sub(*b0) as f64, i1.saturating_sub(*i0) as f64);
                if db + di > 0.0 {
                    utils.insert(*cpu, 100.0 * db / (db + di));
                }
            }
        }
        self.last_stat = stat;
        // Energy counters (root only on current kernels).
        let pkg = self.rapl_pkg.as_ref().and_then(|p| read_num::<u64>(&p.join("energy_uj")));
        let core = self.rapl_core.as_ref().and_then(|p| read_num::<u64>(&p.join("energy_uj")));
        if let (Some(pkg), Some((t0, p0, c0))) = (pkg, self.last_energy) {
            let dt = t - t0;
            if dt > 0.05 && pkg >= p0 {
                r.package_w = Some((pkg - p0) as f64 / 1e6 / dt);
                if let Some(c) = core.filter(|c| *c >= c0) {
                    r.core_w = Some((c - c0) as f64 / 1e6 / dt);
                }
            }
        }
        self.last_energy = pkg.map(|p| (t, p, core.unwrap_or(0)));

        let (mut sum, mut n, mut max) = (0.0, 0, 0.0f64);
        for th in self.threads.iter_mut() {
            th.online = sys.exists(format!("{CPU}/cpu{}/cpufreq", th.cpu)) || th.cpu == 0;
            let base = format!("{CPU}/cpu{}/cpufreq", th.cpu);
            let khz = sys
                .read_u64(format!("{base}/cpuinfo_avg_freq"))
                .or_else(|| sys.read_u64(format!("{base}/scaling_cur_freq")));
            let mhz = khz.map(|k| k as f64 / 1000.0).filter(|m| *m > 0.0);
            if let Some(m) = mhz {
                sum += m;
                n += 1;
                max = max.max(m);
            }
            r.threads.push(ThreadReading { cpu: th.cpu, mhz, util: crate::util::round1(utils.get(&th.cpu).copied().unwrap_or(0.0)) });
        }
        r.util = crate::util::round1(utils.values().sum::<f64>() / utils.len().max(1) as f64);
        r.mhz_avg = if n > 0 { (sum / n as f64).round() } else { 0.0 };
        r.mhz_max = max.round();
        self.reading = r;
    }

    fn policy_of(&self, cpu: u32) -> String {
        format!("{CPU}/cpu{cpu}/cpufreq")
    }

    fn first_online(&self, tier: &str) -> Option<u32> {
        self.threads
            .iter()
            .filter(|t| t.tier == tier && self.sys.exists(self.policy_of(t.cpu)))
            .map(|t| t.cpu)
            .next()
    }

    /// The current knob values and their allowed ranges, for the UI.
    pub fn knobs(&self) -> Value {
        let sys = &self.sys;
        let p0 = self.first_online("fast").map(|c| self.policy_of(c)).unwrap_or_else(|| self.policy_of(0));
        let dense = self.first_online("dense").map(|c| self.policy_of(c));
        let words = |p: String| -> Vec<String> {
            sys.read(p).map(|s| s.split_whitespace().map(|w| w.trim_matches(['[', ']']).to_string()).collect()).unwrap_or_default()
        };
        let mhz = |p: String| sys.read_u64(p).map(|k| (k as f64 / 1000.0).round());
        let idle: Vec<Value> = sys
            .list(format!("{CPU}/cpu0/cpuidle"))
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("state")))
            .map(|p| {
                json!({
                    "name": read_trim(&p.join("name")),
                    "latency_us": read_num::<u64>(&p.join("latency")),
                    "disabled": read_trim(&p.join("disable")).as_deref() == Some("1"),
                    "usage": read_num::<u64>(&p.join("usage")),
                    "time_us": read_num::<u64>(&p.join("time")),
                })
            })
            .collect();
        json!({
            "pstate_mode": sys.read(format!("{CPU}/amd_pstate/status")),
            "pstate_modes": if sys.exists(format!("{CPU}/amd_pstate/status")) { json!(["active", "passive", "guided"]) } else { json!([]) },
            "prefcore": sys.read(format!("{CPU}/amd_pstate/prefcore")),
            "boost": sys.read(format!("{CPU}/cpufreq/boost")).map(|b| b == "1"),
            "governor": sys.read(format!("{p0}/scaling_governor")),
            "governors": words(format!("{p0}/scaling_available_governors")),
            "epp": sys.read(format!("{p0}/energy_performance_preference")),
            "epps": words(format!("{p0}/energy_performance_available_preferences")),
            "min_mhz": mhz(format!("{p0}/scaling_min_freq")),
            "max_mhz": mhz(format!("{p0}/scaling_max_freq")),
            "max_mhz_dense": dense.as_ref().and_then(|d| mhz(format!("{d}/scaling_max_freq"))),
            "hw_min_mhz": mhz(format!("{p0}/cpuinfo_min_freq")),
            "hw_max_mhz": mhz(format!("{p0}/cpuinfo_max_freq")),
            "nonlinear_mhz": mhz(format!("{p0}/amd_pstate_lowest_nonlinear_freq")),
            "has_dense": dense.is_some() || self.threads.iter().any(|t| t.tier == "dense"),
            "smt": sys.read(format!("{CPU}/smt/control")),
            "smt_active": sys.read(format!("{CPU}/smt/active")).map(|s| s == "1"),
            "idle_states": idle,
        })
    }

    /// Sysfs writes for a CPU tune. `None` values mean "back to stock".
    pub fn plan(&self, t: &CpuTune) -> Vec<Write> {
        let mut w = Vec::new();
        let sys = &self.sys;
        if sys.exists(format!("{CPU}/amd_pstate/status")) {
            w.push(Write::new(format!("{CPU}/amd_pstate/status"), t.pstate_mode.clone()).global(true));
        }
        if sys.exists(format!("{CPU}/smt/control")) {
            w.push(Write::new(format!("{CPU}/smt/control"), t.smt.map(|s| if s { "on" } else { "off" }.to_string())).global(true));
        }
        if sys.exists(format!("{CPU}/cpufreq/boost")) {
            w.push(Write::new(format!("{CPU}/cpufreq/boost"), t.boost.map(|b| if b { "1" } else { "0" }.to_string())));
        }
        for th in &self.threads {
            let p = self.policy_of(th.cpu);
            w.push(Write::new(format!("{p}/scaling_governor"), t.governor.clone()));
            w.push(Write::new(format!("{p}/energy_performance_preference"), t.epp.clone()));
            let max = if th.tier == "dense" { t.max_mhz_dense.or(t.max_mhz) } else { t.max_mhz };
            w.push(Write::new(format!("{p}/scaling_min_freq"), t.min_mhz.map(|m| (m * 1000).to_string())).pair(format!("{p}/scaling_max_freq")));
            w.push(Write::new(format!("{p}/scaling_max_freq"), max.map(|m| (m * 1000).to_string())).pair(format!("{p}/scaling_min_freq")));
        }
        // C-states: disable every state deeper than the named one.
        let names: Vec<(String, String)> = sys
            .list(format!("{CPU}/cpu0/cpuidle"))
            .iter()
            .filter_map(|p| Some((p.file_name()?.to_string_lossy().to_string(), read_trim(&p.join("name"))?)))
            .filter(|(d, _)| d.starts_with("state"))
            .collect();
        let limit = t.idle_max.as_ref().and_then(|n| names.iter().position(|(_, name)| name.eq_ignore_ascii_case(n)));
        for th in &self.threads {
            for (i, (dir, _)) in names.iter().enumerate() {
                let path = format!("{CPU}/cpu{}/cpuidle/{dir}/disable", th.cpu);
                let v = limit.map(|l| if i > l { "1" } else { "0" }.to_string());
                w.push(Write::new(path, v));
            }
        }
        w
    }

    /// Knob paths to snapshot before the first write.
    pub fn knob_paths(&self) -> Vec<String> {
        self.plan(&CpuTune::default()).into_iter().map(|w| w.path).collect()
    }

    pub fn tier_counts(&self) -> BTreeMap<String, u32> {
        let mut m = BTreeMap::new();
        for t in &self.threads {
            *m.entry(t.tier.clone()).or_insert(0) += 1;
        }
        m
    }
}

fn ident(sys: &Sys) -> CpuIdent {
    let mut id = CpuIdent::default();
    let info = sys.read("/proc/cpuinfo").unwrap_or_default();
    let mut threads = 0;
    let mut cores = std::collections::BTreeSet::new();
    let mut phys = String::new();
    for line in info.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "processor" => threads += 1,
            "model name" if id.model.is_empty() => id.model = v.into(),
            "vendor_id" if id.vendor.is_empty() => id.vendor = v.into(),
            "cpu family" if id.family == 0 => id.family = v.parse().unwrap_or(0),
            "model" if id.model_id == 0 => id.model_id = v.parse().unwrap_or(0),
            "stepping" if id.stepping == 0 => id.stepping = v.parse().unwrap_or(0),
            "microcode" if id.microcode.is_empty() => id.microcode = v.into(),
            "physical id" => phys = v.into(),
            "core id" => {
                cores.insert(format!("{phys}:{v}"));
            }
            "flags" if id.flags_of_note.is_empty() => {
                let want = ["avx2", "avx512f", "avx512_bf16", "fma", "sha_ni", "vaes", "cppc", "svm"];
                id.flags_of_note = v.split_whitespace().filter(|f| want.contains(f)).map(String::from).collect();
            }
            _ => {}
        }
    }
    id.threads = threads;
    id.cores = cores.len().max(1) as u32;
    for idx in sys.list(format!("{CPU}/cpu0/cache")) {
        let (Some(level), Some(kind), Some(size)) =
            (read_trim(&idx.join("level")), read_trim(&idx.join("type")), read_trim(&idx.join("size")))
        else {
            continue;
        };
        let shared = read_trim(&idx.join("shared_cpu_list")).map(|s| crate::util::parse_cpulist(&s).len()).unwrap_or(1);
        let instances = (threads as usize / shared.max(1)).max(1);
        let k = match kind.as_str() {
            "Data" => "d",
            "Instruction" => "i",
            _ => "",
        };
        id.caches.push(format!("L{level}{k} {size} ×{instances}"));
    }
    id.driver = sys.read(format!("{CPU}/cpu0/cpufreq/scaling_driver")).unwrap_or_default();
    id
}

fn threads(sys: &Sys) -> Vec<Thread> {
    let possible = sys.read(format!("{CPU}/possible")).map(|s| crate::util::parse_cpulist(&s)).unwrap_or_default();
    let mut v: Vec<Thread> = possible
        .into_iter()
        .map(|cpu| {
            let core = sys.read_u64(format!("{CPU}/cpu{cpu}/topology/core_id")).unwrap_or(cpu as u64) as u32;
            let ranking = sys.read_u64(format!("{CPU}/cpu{cpu}/cpufreq/amd_pstate_prefcore_ranking")).unwrap_or(0) as u32;
            Thread { cpu, core, tier: "fast".into(), ranking, online: true }
        })
        .collect();
    // Ranking tiers: the top ranking is "fast"; a second, clearly lower tier
    // (Zen 4c / Zen 5c compact cores) is "dense". Prefcore rankings move a
    // little within a tier at runtime, so use a 15 % gap, not equality.
    let top = v.iter().map(|t| t.ranking).max().unwrap_or(0);
    if top > 0 {
        for t in &mut v {
            if (t.ranking as f64) < top as f64 * 0.85 && t.ranking > 0 {
                t.tier = "dense".into();
            }
        }
    }
    v
}

/// (busy, idle) jiffies per online CPU from /proc/stat.
fn proc_stat(sys: &Sys) -> BTreeMap<u32, (u64, u64)> {
    sys.read("/proc/stat")
        .unwrap_or_default()
        .lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let cpu: u32 = it.next()?.strip_prefix("cpu")?.parse().ok()?;
            let n: Vec<u64> = it.filter_map(|x| x.parse().ok()).collect();
            let idle = n.get(3).unwrap_or(&0) + n.get(4).unwrap_or(&0);
            let total: u64 = n.iter().take(8).sum();
            Some((cpu, (total.saturating_sub(idle), idle)))
        })
        .collect()
}
