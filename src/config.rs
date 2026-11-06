//! Persistent configuration: /var/lib/omarchy-oc/config.json.

use crate::tune::{Tune, builtins};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Rule {
    /// "process:<glob>" matches a process name or executable basename
    /// (case-insensitive, `*` and `?` wildcards); "steam-game" matches any
    /// running Steam game; "gamemode" matches any process run with
    /// gamemoderun / libgamemode.
    pub matcher: String,
    pub profile: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Watchdog {
    pub enabled: bool,
    pub cpu_max_c: f64,
    pub gpu_max_c: f64,
    pub seconds: u32,
    /// Revert to `fallback` when the kernel reports a machine check or GPU
    /// hang while a risky profile is active.
    pub on_errors: bool,
    pub fallback: String,
}

impl Default for Watchdog {
    fn default() -> Self {
        Watchdog { enabled: true, cpu_max_c: 98.0, gpu_max_c: 100.0, seconds: 5, on_errors: true, fallback: "stock".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct MetricsCfg {
    pub http: bool,
    pub listen: String,
}

impl Default for MetricsCfg {
    fn default() -> Self {
        MetricsCfg { http: false, listen: "127.0.0.1:9877".into() }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct TestResult {
    pub time: f64,
    pub seconds: f64,
    pub passed: bool,
    pub errors: u64,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Stability {
    /// Tune fingerprint these results belong to.
    pub fingerprint: String,
    /// User kept the settings through the confirm window.
    pub confirmed: bool,
    pub cpu: Option<TestResult>,
    pub gpu: Option<TestResult>,
    /// Boots that ended in a crash with this profile applied.
    pub crashes: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    /// Profile the user picked.
    pub base: String,
    pub boot_apply: bool,
    pub auto_switch: bool,
    pub rules: Vec<Rule>,
    /// User profiles. Built-in names cannot be stored here.
    pub profiles: BTreeMap<String, Tune>,
    pub confirm_seconds: u32,
    pub watchdog: Watchdog,
    pub metrics: MetricsCfg,
    pub stability: BTreeMap<String, Stability>,
}

impl Default for Config {
    fn default() -> Self {
        let mut profiles = BTreeMap::new();
        profiles.insert("custom".to_string(), Tune::default());
        Config {
            version: 1,
            base: "stock".into(),
            boot_apply: true,
            auto_switch: false,
            rules: Vec::new(),
            profiles,
            confirm_seconds: 20,
            watchdog: Watchdog::default(),
            metrics: MetricsCfg::default(),
            stability: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Config {
        let mut c: Config = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let b = builtins();
        c.profiles.retain(|k, _| !b.contains_key(k));
        if !c.profile_exists(&c.base) {
            c.base = "stock".into();
        }
        c
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let data = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        crate::util::write_atomic(path, &data, 0o644)
    }

    pub fn profile(&self, name: &str) -> Option<Tune> {
        builtins().remove(name).or_else(|| self.profiles.get(name).cloned())
    }

    pub fn profile_exists(&self, name: &str) -> bool {
        self.profile(name).is_some()
    }

    pub fn is_builtin(name: &str) -> bool {
        builtins().contains_key(name)
    }

    /// All profile names: built-ins in their order, then user ones.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = crate::tune::BUILTIN_ORDER.iter().map(|s| s.to_string()).collect();
        v.extend(self.profiles.keys().cloned());
        v
    }

    /// Stability record that still matches the profile's current settings.
    pub fn stable(&self, name: &str, t: &Tune) -> Option<&Stability> {
        self.stability.get(name).filter(|s| s.fingerprint == t.fingerprint())
    }

    pub fn stability_mut(&mut self, name: &str, t: &Tune) -> &mut Stability {
        let fp = t.fingerprint();
        let e = self.stability.entry(name.to_string()).or_default();
        if e.fingerprint != fp {
            *e = Stability { fingerprint: fp, ..Default::default() };
        }
        e
    }
}

pub fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 32 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Glob with `*` and `?`, case-insensitive.
pub fn glob(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.to_lowercase().chars().collect();
    let s: Vec<char> = s.to_lowercase().chars().collect();
    let (mut pi, mut si, mut star, mut mark) = (0, 0, None, 0);
    while si < s.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == s[si]) {
            pi += 1;
            si += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = si;
            pi += 1;
        } else if let Some(st) = star {
            pi = st + 1;
            mark += 1;
            si = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globbing() {
        assert!(glob("cyberpunk*", "Cyberpunk2077.exe"));
        assert!(glob("*.exe", "witcher3.exe"));
        assert!(glob("blender", "blender"));
        assert!(!glob("blender", "blender-bin"));
        assert!(glob("b?ender", "blender"));
    }

    #[test]
    fn builtins_not_stored() {
        let mut c = Config::default();
        c.profiles.insert("max".into(), Tune::default());
        let dir = std::env::temp_dir().join(format!("oc-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("c.json");
        c.save(&p).unwrap();
        let d = Config::load(&p);
        assert!(!d.profiles.contains_key("max"));
        assert!(d.profile("max").unwrap().smu.co_all.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stability_resets_on_change() {
        let mut c = Config::default();
        let mut t = Tune::default();
        c.stability_mut("custom", &t).confirmed = true;
        assert!(c.stable("custom", &t).is_some_and(|s| s.confirmed));
        t.smu.co_all = Some(-5);
        assert!(c.stable("custom", &t).is_none());
        assert!(!c.stability_mut("custom", &t).confirmed);
    }
}
