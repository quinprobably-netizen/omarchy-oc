//! Executes sysfs write plans. Before a knob is first written, its value is
//! snapshotted (once per boot, in /run) so "stock" always means the value
//! the hardware booted with, even across daemon restarts.

use crate::platform::bracketed;
use crate::tune::Write;
use crate::util::Sys;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub struct Applier {
    sys: Sys,
    file: PathBuf,
    pub originals: BTreeMap<String, String>,
}

#[derive(Default, Debug)]
pub struct Report {
    pub changed: Vec<String>,
    pub errors: Vec<String>,
    pub notes: Vec<String>,
}

impl Report {
    pub fn merge(&mut self, o: Report) {
        self.changed.extend(o.changed);
        self.errors.extend(o.errors);
        self.notes.extend(o.notes);
    }
}

/// Current value in the same form we write it: bracketed choice files
/// ("[base] power_saving") reduce to the active word.
fn normalize(raw: &str) -> String {
    if raw.contains('[') { bracketed(raw).unwrap_or_else(|| raw.to_string()) } else { raw.trim().to_string() }
}

fn short(path: &str) -> String {
    // "/sys/devices/system/cpu/cpu3/cpufreq/scaling_max_freq" -> "scaling_max_freq"
    // grouped so twelve identical per-CPU errors read as one.
    let p = std::path::Path::new(path);
    let file = p.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
    if file == "disable" {
        return "cpuidle state".into();
    }
    file
}

impl Applier {
    pub fn load(sys: &Sys, file: PathBuf) -> Applier {
        let originals = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Applier { sys: sys.clone(), file, originals }
    }

    pub fn read(&self, path: &str) -> Option<String> {
        self.sys.read(path).map(|s| normalize(&s))
    }

    /// Record the current value of every path not yet snapshotted.
    pub fn snapshot(&mut self, paths: &[String]) {
        let mut added = false;
        for p in paths {
            if self.originals.contains_key(p) {
                continue;
            }
            if let Some(v) = self.read(p) {
                self.originals.insert(p.clone(), v);
                added = true;
            }
        }
        if added {
            self.save();
        }
    }

    fn save(&self) {
        if let Ok(b) = serde_json::to_vec_pretty(&self.originals) {
            let _ = crate::util::write_atomic(&self.file, &b, 0o644);
        }
    }

    /// Ok(Some(value)) written, Ok(None) nothing to do, Err(reason).
    fn try_write(&self, w: &Write) -> Result<Option<String>, String> {
        let Some(want) = w.value.clone().or_else(|| self.originals.get(&w.path).cloned()) else { return Ok(None) };
        let Some(cur) = self.read(&w.path) else { return Ok(None) }; // offline CPU, missing knob
        if cur == want {
            return Ok(None);
        }
        match self.sys.write(&w.path, &want) {
            Ok(()) => {
                if w.global {
                    // amd-pstate mode and SMT changes re-initialise policies.
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
                Ok(Some(want))
            }
            Err(e) => {
                let msg = e.to_string();
                Err(format!("{want}: {}", msg.rsplit(": ").next().unwrap_or(&msg)))
            }
        }
    }

    pub fn execute(&mut self, writes: &[Write]) -> Report {
        let mut r = Report::default();
        let paths: Vec<String> = writes.iter().map(|w| w.path.clone()).collect();
        self.snapshot(&paths);
        let mut changed: BTreeMap<String, usize> = BTreeMap::new();
        let mut errs: BTreeMap<String, (String, usize)> = BTreeMap::new();
        let mut notes = BTreeSet::new();
        let mut deferred = Vec::new();
        for pass in 0..2 {
            let list: Vec<&Write> = if pass == 0 { writes.iter().collect() } else { std::mem::take(&mut deferred) };
            for w in list {
                match self.try_write(w) {
                    Ok(Some(v)) => *changed.entry(format!("{}={v}", short(&w.path))).or_insert(0) += 1,
                    Ok(None) => {}
                    // min above current max (or the reverse): retry after the pair.
                    Err(_) if pass == 0 && w.pair.is_some() => deferred.push(w),
                    Err(e) if short(&w.path) == "energy_performance_preference" && e.to_lowercase().contains("busy") => {
                        notes.insert("EPP is fixed while the performance governor is active".to_string());
                    }
                    Err(e) => {
                        let ent = errs.entry(short(&w.path)).or_insert((e, 0));
                        ent.1 += 1;
                    }
                }
            }
        }
        for (k, n) in changed {
            r.changed.push(if n > 1 { format!("{k} ×{n}") } else { k });
        }
        for (k, (msg, n)) in errs {
            r.errors.push(if n > 1 { format!("{k}: {msg} (×{n})") } else { format!("{k}: {msg}") });
        }
        r.notes = notes.into_iter().collect();
        r
    }

    /// Restore every snapshotted knob (daemon exit, `omarchy-ocd restore`).
    /// Global knobs first, since they reset the rest.
    pub fn restore_all(&mut self) -> Report {
        let mut keys: Vec<String> = self.originals.keys().cloned().collect();
        let global = |k: &str| k.ends_with("amd_pstate/status") || k.ends_with("smt/control");
        keys.sort_by_key(|k| !global(k));
        let writes: Vec<Write> = keys
            .iter()
            .map(|k| {
                let mut w = Write::new(k.clone(), None).global(global(k));
                if k.ends_with("scaling_min_freq") || k.ends_with("min_freq") {
                    w.pair = Some(k.replace("min_freq", "max_freq"));
                } else if k.ends_with("max_freq") {
                    w.pair = Some(k.replace("max_freq", "min_freq"));
                }
                w
            })
            .collect();
        self.execute(&writes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_and_restore() {
        let dir = std::env::temp_dir().join(format!("oc-apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("k")).unwrap();
        std::fs::write(dir.join("k/gov"), "powersave\n").unwrap();
        std::fs::write(dir.join("k/prof"), "[base] power_saving\n").unwrap();
        let sys = Sys::new(&dir);
        let mut a = Applier::load(&sys, dir.join("orig.json"));
        let r = a.execute(&[
            Write::new("/k/gov".into(), Some("performance".into())),
            Write::new("/k/prof".into(), Some("power_saving".into())),
        ]);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(std::fs::read_to_string(dir.join("k/gov")).unwrap(), "performance");
        assert_eq!(a.originals["/k/gov"], "powersave");
        assert_eq!(a.originals["/k/prof"], "base");
        // None restores the snapshot.
        a.execute(&[Write::new("/k/gov".into(), None)]);
        assert_eq!(std::fs::read_to_string(dir.join("k/gov")).unwrap(), "powersave");
        // Snapshot survives a reload and is not overwritten.
        std::fs::write(dir.join("k/gov"), "performance").unwrap();
        let mut b = Applier::load(&sys, dir.join("orig.json"));
        b.snapshot(&["/k/gov".into()]);
        assert_eq!(b.originals["/k/gov"], "powersave");
        b.restore_all();
        assert_eq!(std::fs::read_to_string(dir.join("k/gov")).unwrap(), "powersave");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
