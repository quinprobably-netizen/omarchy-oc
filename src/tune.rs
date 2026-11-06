//! The settings model. A [`Tune`] says what to change; every field left
//! `None` means "stock" (the value the hardware had when the daemon first
//! started this boot). A profile is a named Tune.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CpuTune {
    /// amd-pstate operating mode: active | passive | guided.
    pub pstate_mode: Option<String>,
    pub governor: Option<String>,
    /// Energy-performance preference (active mode only).
    pub epp: Option<String>,
    pub boost: Option<bool>,
    pub min_mhz: Option<u32>,
    pub max_mhz: Option<u32>,
    /// Max frequency for the compact (Zen 4c) cores; defaults to max_mhz.
    pub max_mhz_dense: Option<u32>,
    pub smt: Option<bool>,
    /// Deepest idle state allowed (e.g. "C1"); deeper ones are disabled.
    pub idle_max: Option<String>,
}

/// Firmware (SMU) limits. These go to the CPU's power-management firmware
/// and reset on every reboot.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SmuTune {
    /// Sustained power (STAPM), W.
    pub stapm_w: Option<u32>,
    /// PPT fast (boost) limit, W.
    pub fast_w: Option<u32>,
    /// PPT slow limit, W.
    pub slow_w: Option<u32>,
    /// Temperature target, °C.
    pub tctl_c: Option<u32>,
    /// Core VRM thermal (TDC) and peak (EDC) current, A.
    pub tdc_a: Option<u32>,
    pub edc_a: Option<u32>,
    pub soc_tdc_a: Option<u32>,
    pub soc_edc_a: Option<u32>,
    /// Curve Optimizer, all cores (negative = undervolt).
    pub co_all: Option<i32>,
    /// Curve Optimizer per physical core, applied after co_all.
    pub co_cores: BTreeMap<u32, i32>,
}

impl SmuTune {
    pub fn is_empty(&self) -> bool {
        *self == SmuTune::default()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GpuTune {
    pub min_mhz: Option<u32>,
    pub max_mhz: Option<u32>,
    /// Sustained power limit (PL1), W.
    pub power_w: Option<f64>,
    /// PL1 averaging window, ms.
    pub power_window_ms: Option<u32>,
    /// xe power profile: base | power_saving.
    pub power_profile: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PlatformTune {
    /// PCIe ASPM policy: default | performance | powersave | powersupersave.
    pub aspm: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FanTune {
    /// omarchy-fans profile to select with this tune.
    pub profile: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Tune {
    pub cpu: CpuTune,
    pub smu: SmuTune,
    pub gpu: GpuTune,
    pub platform: PlatformTune,
    pub fans: FanTune,
}

/// One sysfs write in an apply plan. `value: None` means "restore stock".
#[derive(Clone, Debug)]
pub struct Write {
    pub path: String,
    pub value: Option<String>,
    /// Writing this resets other knobs (amd-pstate mode, SMT).
    pub global: bool,
    /// The other half of a min/max pair: if a write fails, retry after it.
    pub pair: Option<String>,
}

impl Write {
    pub fn new(path: String, value: Option<String>) -> Write {
        Write { path, value, global: false, pair: None }
    }
    pub fn global(mut self, g: bool) -> Write {
        self.global = g;
        self
    }
    pub fn pair(mut self, p: String) -> Write {
        self.pair = Some(p);
        self
    }
}

/// Hard bounds. Values outside are rejected, not clamped, so a typo never
/// silently becomes a different overclock.
pub struct Bounds;
impl Bounds {
    pub const CO: (i32, i32) = (-50, 30);
    pub const SMU_W: (u32, u32) = (10, 200);
    pub const TCTL: (u32, u32) = (60, 100);
    pub const AMPS: (u32, u32) = (10, 250);
    pub const GPU_W: (f64, f64) = (30.0, 300.0);
    pub const GPU_WINDOW_MS: (u32, u32) = (1, 1_000_000);
    pub const CPU_MHZ: (u32, u32) = (200, 7000);
}

fn check<T: PartialOrd + std::fmt::Display + Copy>(name: &str, v: Option<T>, (lo, hi): (T, T)) -> Result<(), String> {
    match v {
        Some(x) if x < lo || x > hi => Err(format!("{name} {x} is outside {lo}..{hi}")),
        _ => Ok(()),
    }
}

impl Tune {
    pub fn validate(&self) -> Result<(), String> {
        let c = &self.cpu;
        check("cpu.min_mhz", c.min_mhz, Bounds::CPU_MHZ)?;
        check("cpu.max_mhz", c.max_mhz, Bounds::CPU_MHZ)?;
        check("cpu.max_mhz_dense", c.max_mhz_dense, Bounds::CPU_MHZ)?;
        if let (Some(a), Some(b)) = (c.min_mhz, c.max_mhz) {
            if a > b {
                return Err(format!("cpu.min_mhz {a} is above cpu.max_mhz {b}"));
            }
        }
        if let Some(m) = &c.pstate_mode {
            if !["active", "passive", "guided"].contains(&m.as_str()) {
                return Err(format!("cpu.pstate_mode must be active, passive or guided, not {m}"));
            }
        }
        let s = &self.smu;
        for (n, v) in [("smu.stapm_w", s.stapm_w), ("smu.fast_w", s.fast_w), ("smu.slow_w", s.slow_w)] {
            check(n, v, Bounds::SMU_W)?;
        }
        check("smu.tctl_c", s.tctl_c, Bounds::TCTL)?;
        for (n, v) in [("smu.tdc_a", s.tdc_a), ("smu.edc_a", s.edc_a), ("smu.soc_tdc_a", s.soc_tdc_a), ("smu.soc_edc_a", s.soc_edc_a)] {
            check(n, v, Bounds::AMPS)?;
        }
        check("smu.co_all", s.co_all, Bounds::CO)?;
        for (core, v) in &s.co_cores {
            if *core > 63 {
                return Err(format!("smu.co_cores: core {core} does not exist"));
            }
            check(&format!("smu.co_cores[{core}]"), Some(*v), Bounds::CO)?;
        }
        let g = &self.gpu;
        check("gpu.min_mhz", g.min_mhz, (100, 4000))?;
        check("gpu.max_mhz", g.max_mhz, (100, 4000))?;
        if let (Some(a), Some(b)) = (g.min_mhz, g.max_mhz) {
            if a > b {
                return Err(format!("gpu.min_mhz {a} is above gpu.max_mhz {b}"));
            }
        }
        check("gpu.power_w", g.power_w, Bounds::GPU_W)?;
        check("gpu.power_window_ms", g.power_window_ms, Bounds::GPU_WINDOW_MS)?;
        if let Some(p) = &g.power_profile {
            if !["base", "power_saving"].contains(&p.as_str()) {
                return Err(format!("gpu.power_profile must be base or power_saving, not {p}"));
            }
        }
        if let Some(a) = &self.platform.aspm {
            if !["default", "performance", "powersave", "powersupersave"].contains(&a.as_str()) {
                return Err(format!("platform.aspm: unknown policy {a}"));
            }
        }
        Ok(())
    }

    /// Settings that can make the machine unstable and so need a confirm
    /// within the revert window, and a pass of the stability test before
    /// they may apply at boot or from an auto-switch rule.
    pub fn risky(&self, stock_gpu_power_w: Option<f64>) -> bool {
        let s = &self.smu;
        let co = s.co_all.is_some_and(|v| v != 0) || s.co_cores.values().any(|v| *v != 0);
        let limits = s.stapm_w.is_some()
            || s.fast_w.is_some()
            || s.slow_w.is_some()
            || s.tctl_c.is_some()
            || s.tdc_a.is_some()
            || s.edc_a.is_some()
            || s.soc_tdc_a.is_some()
            || s.soc_edc_a.is_some();
        let gpu_raise = match (self.gpu.power_w, stock_gpu_power_w) {
            (Some(w), Some(stock)) => w > stock + 0.5,
            (Some(_), None) => true,
            _ => false,
        };
        co || limits || gpu_raise
    }

    /// Stable fingerprint of the settings that bear on stability, so a
    /// stability record can tell whether the profile changed in a way
    /// that matters since it was kept or tested. Only the risky settings
    /// count (every SMU field and the GPU power limit; see [`Tune::risky`]):
    /// clocks, governor, EPP, GPU profile and window, ASPM and fans can be
    /// edited live without losing the record or reopening Keep/Revert.
    pub fn fingerprint(&self) -> String {
        let s = serde_json::to_string(&serde_json::json!({"smu": self.smu, "gpu_power_w": self.gpu.power_w})).unwrap_or_default();
        // FNV-1a, 64 bit.
        let mut h: u64 = 0xcbf29ce484222325;
        for b in s.bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{h:016x}")
    }

    /// Set one field from a dotted key ("gpu.power_w", "smu.co_cores.3",
    /// "cpu.boost"). An empty value or "stock" clears it.
    pub fn set_key(&mut self, key: &str, value: &str) -> Result<(), String> {
        let mut v = serde_json::to_value(&*self).map_err(|e| e.to_string())?;
        let parts: Vec<&str> = key.split('.').collect();
        let clear = value.is_empty() || value == "stock" || value == "null";
        let parsed: serde_json::Value = if clear {
            serde_json::Value::Null
        } else {
            serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::String(value.to_string()))
        };
        match parts.as_slice() {
            [sec, field] => {
                let obj = v.get_mut(*sec).and_then(|o| o.as_object_mut()).ok_or_else(|| format!("unknown section {sec}"))?;
                if !obj.contains_key(*field) {
                    return Err(format!("unknown setting {key}"));
                }
                if *field == "co_cores" {
                    return Err("use smu.co_cores.<core>".into());
                }
                obj.insert(field.to_string(), parsed);
            }
            ["smu", "co_cores", core] => {
                let core: u32 = core.parse().map_err(|_| format!("bad core number {core}"))?;
                let cc = v["smu"]["co_cores"].as_object_mut().ok_or("bad co_cores")?;
                if clear {
                    cc.remove(&core.to_string());
                } else {
                    cc.insert(core.to_string(), parsed);
                }
            }
            _ => return Err(format!("unknown setting {key}")),
        }
        let t: Tune = serde_json::from_value(v).map_err(|e| format!("{key}: {e}"))?;
        t.validate()?;
        *self = t;
        Ok(())
    }
}

/// True when a [`Tune::set_key`] value for `key` is clearing it: JSON null,
/// or the strings "", "stock" and "null" (the same spellings `set_key` takes).
fn value_clears(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => true,
        serde_json::Value::String(s) => s.is_empty() || s == "stock" || s == "null",
        _ => false,
    }
}

/// The string form of a live-edit value for [`Tune::set_key`]: strings as
/// given, null as "" (clear), numbers and booleans as their JSON text.
pub fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Whether setting one key makes a profile risky in the sense of
/// [`Tune::risky`]: any SMU limit or non-zero Curve Optimizer value, or
/// `gpu.power_w` above stock (or when stock is unknown). Clearing a key is
/// never risky, and neither is anything else. The panel applies safe
/// changes live and stages risky ones behind Apply.
pub fn key_is_risky(key: &str, value: &serde_json::Value, stock_gpu_power_w: Option<f64>) -> bool {
    if value_clears(value) {
        return false;
    }
    let num = |v: &serde_json::Value| -> Option<f64> {
        match v {
            serde_json::Value::Number(n) => n.as_f64(),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        }
    };
    match key.split('.').collect::<Vec<_>>().as_slice() {
        ["smu", "co_all"] | ["smu", "co_cores", _] => num(value) != Some(0.0),
        ["smu", _] => true,
        ["gpu", "power_w"] => match (num(value), stock_gpu_power_w) {
            (Some(w), Some(stock)) => w > stock + 0.5,
            _ => true,
        },
        _ => false,
    }
}

/// Built-in profiles, tuned for a 65 W Ryzen 8000G desktop APU with an
/// Arc A-series card. "stock" is the empty tune.
pub fn builtins() -> BTreeMap<String, Tune> {
    let mut m = BTreeMap::new();
    m.insert("stock".to_string(), Tune::default());

    let mut eco = Tune::default();
    eco.cpu.governor = Some("powersave".into());
    eco.cpu.epp = Some("power".into());
    eco.cpu.boost = Some(false);
    eco.gpu.max_mhz = Some(1800);
    eco.gpu.power_profile = Some("power_saving".into());
    eco.gpu.power_w = Some(100.0);
    eco.platform.aspm = Some("powersupersave".into());
    eco.fans.profile = Some("silent".into());
    m.insert("eco".to_string(), eco);

    let mut bal = Tune::default();
    bal.cpu.governor = Some("powersave".into());
    bal.cpu.epp = Some("balance_performance".into());
    bal.cpu.boost = Some(true);
    bal.gpu.power_profile = Some("base".into());
    bal.fans.profile = Some("balanced".into());
    m.insert("balanced".to_string(), bal);

    let mut perf = Tune::default();
    perf.cpu.governor = Some("performance".into());
    perf.cpu.boost = Some(true);
    perf.gpu.power_profile = Some("base".into());
    perf.gpu.min_mhz = Some(1000);
    perf.platform.aspm = Some("performance".into());
    perf.fans.profile = Some("performance".into());
    m.insert("performance".to_string(), perf);

    // Firmware limits raised past the 88 W stock PPT, a mild all-core
    // undervolt, and the Arc card's PL1 raised to its board power.
    let mut max = m["performance"].clone();
    max.smu.stapm_w = Some(110);
    max.smu.fast_w = Some(120);
    max.smu.slow_w = Some(110);
    max.smu.tctl_c = Some(95);
    max.smu.co_all = Some(-10);
    max.gpu.power_w = Some(190.0);
    max.gpu.min_mhz = Some(1500);
    max.cpu.idle_max = Some("C1".into());
    m.insert("max".to_string(), max);
    m
}

pub const BUILTIN_ORDER: [&str; 5] = ["stock", "eco", "balanced", "performance", "max"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_validate() {
        for (n, t) in builtins() {
            t.validate().unwrap_or_else(|e| panic!("{n}: {e}"));
        }
        assert!(!builtins()["stock"].risky(Some(150.0)));
        assert!(builtins()["max"].risky(Some(150.0)));
        assert!(!builtins()["performance"].risky(Some(150.0)));
    }

    #[test]
    fn set_key_works() {
        let mut t = Tune::default();
        t.set_key("gpu.power_w", "170").unwrap();
        assert_eq!(t.gpu.power_w, Some(170.0));
        t.set_key("smu.co_cores.3", "-15").unwrap();
        assert_eq!(t.smu.co_cores.get(&3), Some(&-15));
        t.set_key("smu.co_cores.3", "stock").unwrap();
        assert!(t.smu.co_cores.is_empty());
        t.set_key("cpu.governor", "performance").unwrap();
        assert_eq!(t.cpu.governor.as_deref(), Some("performance"));
        t.set_key("cpu.boost", "false").unwrap();
        assert_eq!(t.cpu.boost, Some(false));
        assert!(t.set_key("smu.co_all", "-80").is_err());
        assert!(t.set_key("gpu.bogus", "1").is_err());
        assert!(t.set_key("cpu.min_mhz", "6000").is_err() || t.cpu.max_mhz.is_none());
        t.set_key("gpu.power_w", "").unwrap();
        assert_eq!(t.gpu.power_w, None);
    }

    #[test]
    fn key_risk_matches_tune_risk() {
        use serde_json::json;
        // Each risky key, set on an empty tune, must make Tune::risky true,
        // and each safe key must leave it false.
        let stock = Some(150.0);
        for (k, v) in [
            ("smu.co_all", json!(-10)),
            ("smu.co_cores.2", json!(-5)),
            ("smu.stapm_w", json!(90)),
            ("smu.fast_w", json!("95")),
            ("smu.tctl_c", json!(85)),
            ("smu.edc_a", json!(120)),
            ("gpu.power_w", json!(151.0)),
            ("gpu.power_w", json!("200")),
        ] {
            let mut t = Tune::default();
            t.set_key(k, &value_text(&v)).unwrap();
            assert!(t.risky(stock), "{k}={v} should make the tune risky");
            assert!(key_is_risky(k, &v, stock), "{k}={v}");
        }
        for (k, v) in [
            ("smu.co_all", json!(0)),
            ("smu.co_cores.2", json!("0")),
            ("gpu.power_w", json!(150.0)),
            ("gpu.power_w", json!(150.4)),
            ("gpu.power_w", json!(100)),
            ("gpu.power_w", json!("stock")),
            ("smu.co_all", json!(null)),
            ("cpu.epp", json!("power")),
            ("cpu.boost", json!(false)),
            ("gpu.max_mhz", json!(2000)),
            ("platform.aspm", json!("powersave")),
            ("fans.profile", json!("silent")),
        ] {
            let mut t = Tune::default();
            t.set_key(k, &value_text(&v)).unwrap();
            assert!(!t.risky(stock), "{k}={v} should leave the tune safe");
            assert!(!key_is_risky(k, &v, stock), "{k}={v}");
        }
        // Unknown stock: any GPU power value is risky, clearing is not.
        assert!(key_is_risky("gpu.power_w", &json!(100), None));
        assert!(!key_is_risky("gpu.power_w", &json!(""), None));
    }

    #[test]
    fn fingerprint_changes() {
        let a = Tune::default();
        let mut b = a.clone();
        b.smu.co_all = Some(-5);
        assert_ne!(a.fingerprint(), b.fingerprint());
        let mut c = a.clone();
        c.gpu.power_w = Some(190.0);
        assert_ne!(a.fingerprint(), c.fingerprint());
        let mut d = a.clone();
        d.smu.co_cores.insert(2, -10);
        assert_ne!(a.fingerprint(), d.fingerprint());
    }

    #[test]
    fn fingerprint_ignores_safe_settings() {
        // Everything a safe live edit can touch leaves the fingerprint alone.
        let mut a = builtins()["max"].clone();
        let fp = a.fingerprint();
        a.cpu.epp = Some("power".into());
        a.cpu.governor = Some("powersave".into());
        a.cpu.boost = Some(false);
        a.cpu.max_mhz = Some(4500);
        a.cpu.smt = Some(false);
        a.cpu.idle_max = None;
        a.gpu.min_mhz = None;
        a.gpu.max_mhz = Some(2000);
        a.gpu.power_window_ms = Some(50_000);
        a.gpu.power_profile = Some("power_saving".into());
        a.platform.aspm = Some("powersave".into());
        a.fans.profile = Some("silent".into());
        assert_eq!(a.fingerprint(), fp);
        // Every key key_is_risky flags changes it.
        a.smu.stapm_w = Some(100);
        assert_ne!(a.fingerprint(), fp);
    }
}
