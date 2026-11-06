//! omarchy-ocd: samples everything once a second, applies profiles, and
//! keeps them safe:
//!
//! * risky settings (SMU limits, Curve Optimizer, raised GPU power) revert
//!   by themselves unless confirmed within `confirm_seconds`;
//! * a crash while risky settings were unconfirmed or under test means the
//!   next boot (or the daemon's restart on the same boot) starts at stock;
//!   an unconfirmed overclock is never resumed; two crashes with a profile stop it from
//!   applying at boot;
//! * the watchdog falls back to a safe profile on overheating, machine
//!   check errors or GPU hangs;
//! * settings are re-applied after suspend (the SMU forgets them).

use crate::apply::{Applier, Report};
use crate::config::{Config, Rule, Stability, TestResult, glob, valid_name};
use crate::cpu::Cpu;
use crate::fans::Fans;
use crate::gpu::Gpu;
use crate::health::Health;
use crate::history::{History, SERIES};
use crate::mem::Mem;
use crate::platform::Platform;
use crate::smu::{PmLimits, Smu, SmuIo};
use crate::testrun::TestRun;
use crate::tune::{SmuTune, Tune, key_is_risky, value_text};
use crate::util::{Sys, now, round1, write_atomic};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

pub static SIGNAL_STOP: AtomicBool = AtomicBool::new(false);
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct Paths {
    pub run_dir: PathBuf,
    pub state_dir: PathBuf,
    pub group: String,
    pub kmsg: PathBuf,
    /// Where the test children live (`omarchy-oc`, `omarchy-oc-gpustress`).
    pub bin_dir: PathBuf,
}

impl Paths {
    pub fn run(&self, f: &str) -> PathBuf {
        self.run_dir.join(f)
    }
    fn state(&self, f: &str) -> PathBuf {
        self.state_dir.join(f)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Origin {
    User,
    Boot,
    Rule,
    Watchdog,
    Revert,
    Resume,
}

struct Pending {
    profile: String,
    previous: String,
    deadline: f64,
}

/// A stability test in progress: the daemon's own child ([`Core::test`])
/// or a CLI run that reported in with `stress-begin`.
struct StressState {
    kind: String,
    started: f64,
    profile: String,
    /// Requested length, when known (the daemon's own runs).
    seconds: Option<u32>,
    /// The CLI's process, when it said (`stress-begin` with `pid`): a run
    /// whose process is gone (killed with -9) is dropped on the next tick.
    pid: Option<u32>,
}

pub struct Core {
    pub sys: Sys,
    pub paths: Paths,
    pub cfg: Config,
    pub cpu: Cpu,
    pub gpu: Option<Gpu>,
    pub mem: Mem,
    pub platform: Platform,
    pub fans: Fans,
    applier: Applier,
    smu: Option<Box<dyn SmuIo>>,
    smu_family: Option<String>,
    smu_error: Option<String>,
    pm: Option<PmLimits>,
    pm_error: Option<String>,
    smu_stock: Option<PmLimits>,
    smu_applied: SmuTune,
    needs_reboot: Vec<String>,
    pub active: String,
    auto_profile: Option<String>,
    auto_blocked: Option<String>,
    pending: Option<Pending>,
    pub message: String,
    alert: Option<String>,
    history: History,
    health: Arc<Health>,
    health_base: (u64, u64),
    stress: Option<StressState>,
    /// The tester child when the daemon runs the test itself.
    test: Option<TestRun>,
    hot_secs: u32,
    last_auto: f64,
    last_gap: f64,
    risky_since: Option<f64>,
    boot_id: String,
    pub metrics_latest: Arc<Mutex<String>>,
    metrics_serving: bool,
    last_report: Value,
    /// Queued for the socket server to push to subscribers.
    events: Vec<Value>,
    /// False for `detect`: sample only, never write.
    manage: bool,
}

fn read_json<T: serde::de::DeserializeOwned>(p: &PathBuf) -> Option<T> {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn report_json(r: &Report) -> Value {
    json!({"changed": r.changed, "errors": r.errors, "notes": r.notes})
}

impl Core {
    pub fn new(sys: Sys, paths: Paths) -> Core {
        Core::build(sys, paths, true)
    }

    /// Read-only: detect hardware and sample, never write a setting.
    pub fn passive(sys: Sys, paths: Paths) -> Core {
        Core::build(sys, paths, false)
    }

    fn build(sys: Sys, paths: Paths, manage: bool) -> Core {
        let _ = std::fs::create_dir_all(&paths.run_dir);
        let _ = std::fs::create_dir_all(&paths.state_dir);
        let cfg = Config::load(&paths.state("config.json"));
        let cpu = Cpu::detect(&sys);
        let gpu = Gpu::detect(&sys).into_iter().next();
        let mem = Mem::detect(&sys);
        let platform = Platform::detect(&sys);
        let fans = Fans::new(&sys);
        let mut applier = Applier::load(&sys, paths.run("original.json"));
        let mut knobs = cpu.knob_paths();
        if let Some(g) = &gpu {
            knobs.extend(g.knob_paths());
        }
        knobs.extend(platform.knob_paths());
        applier.snapshot(&knobs);

        let smu_family = crate::smu::supported(&cpu.ident.vendor, cpu.ident.family, cpu.ident.model_id).map(String::from);
        let (smu, smu_error) = match &smu_family {
            None => (None, Some(format!("no SMU support for {} (family {:#x} model {:#x})", cpu.ident.model, cpu.ident.family, cpu.ident.model_id))),
            // `detect`: no SMU commands at all (opening sends a test message).
            Some(_) if !manage => (None, Some("not opened in a read-only snapshot".into())),
            Some(f) => match Smu::open(&sys, f) {
                Ok(s) => (Some(Box::new(s) as Box<dyn SmuIo>), None),
                Err(e) => (None, Some(e)),
            },
        };
        let smu_applied: SmuTune = read_json(&paths.run("smu-applied.json")).unwrap_or_default();
        let mut smu_stock: Option<PmLimits> = read_json(&paths.run("smu-stock.json"));
        let (mut pm, mut pm_error) = (None, None);
        if let Some(s) = &smu {
            match s.read_table(&sys) {
                Ok(t) => pm = Some(t),
                Err(e) => pm_error = Some(e),
            }
        }
        if smu_stock.is_none() && smu_applied.is_empty() {
            if let Some(t) = &pm {
                smu_stock = Some(t.clone());
                if let Ok(b) = serde_json::to_vec_pretty(t) {
                    let _ = write_atomic(&paths.run("smu-stock.json"), &b, 0o644);
                }
            }
        }
        let health = Health::start(paths.kmsg.clone());
        let boot_id = sys.read("/proc/sys/kernel/random/boot_id").unwrap_or_default();
        let (mono, boot) = crate::util::clocks();
        let mut core = Core {
            sys,
            paths,
            cfg,
            cpu,
            gpu,
            mem,
            platform,
            fans,
            applier,
            smu,
            smu_family,
            smu_error,
            pm,
            pm_error,
            smu_stock,
            smu_applied,
            needs_reboot: Vec::new(),
            active: "stock".into(),
            auto_profile: None,
            auto_blocked: None,
            pending: None,
            message: String::new(),
            alert: None,
            history: History::new(3600),
            health,
            health_base: (0, 0),
            stress: None,
            test: None,
            hot_secs: 0,
            last_auto: 0.0,
            last_gap: boot - mono,
            risky_since: None,
            boot_id,
            metrics_latest: Arc::new(Mutex::new(String::new())),
            metrics_serving: false,
            last_report: Value::Null,
            events: Vec::new(),
            manage,
        };
        if manage {
            // Not from `detect`: the CLI reads the daemon's info.json.
            core.write_info();
            core.startup();
        }
        core
    }

    /// Tests: put in a fake SMU mailbox (and the stock limits it reports).
    #[doc(hidden)]
    pub fn set_smu(&mut self, dev: Box<dyn SmuIo>, stock: Option<PmLimits>) {
        self.smu = Some(dev);
        self.smu_error = None;
        self.smu_stock = stock;
    }

    fn stock_gpu_w(&self) -> Option<f64> {
        self.gpu.as_ref().and_then(|g| g.stock_power_w(&self.applier.originals))
    }

    fn risky(&self, t: &Tune) -> bool {
        t.risky(self.stock_gpu_w())
    }

    /// The keys of a live-edit batch that need Apply (see `key_is_risky`).
    fn risky_keys(&self, changes: &serde_json::Map<String, Value>) -> Vec<String> {
        let stock = self.stock_gpu_w();
        changes.iter().filter(|(k, v)| key_is_risky(k, v, stock)).map(|(k, _)| k.clone()).collect()
    }

    fn save_cfg(&self) {
        if let Err(e) = self.cfg.save(&self.paths.state("config.json")) {
            eprintln!("omarchy-ocd: saving config: {e}");
        }
    }

    // ---- events for socket subscribers -------------------------------------

    pub fn push_event(&mut self, ev: Value) {
        self.events.push(ev);
    }

    pub fn take_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.events)
    }

    fn pending_json(&self) -> Value {
        let t = now();
        self.pending
            .as_ref()
            .map(|p| json!({"profile": p.profile, "previous": p.previous, "remaining": (p.deadline - t).max(0.0).round()}))
            .unwrap_or(Value::Null)
    }

    /// `{"event":"pending","pending":{...}|null}` whenever the pending state
    /// is set or cleared.
    fn set_pending(&mut self, p: Option<Pending>) {
        let changed = match (&self.pending, &p) {
            (None, None) => false,
            (Some(a), Some(b)) => a.profile != b.profile,
            _ => true,
        };
        self.pending = p;
        if changed {
            let ev = json!({"event": "pending", "pending": self.pending_json()});
            self.push_event(ev);
        }
    }

    fn take_pending(&mut self) -> Option<Pending> {
        let p = self.pending.take();
        if p.is_some() {
            self.push_event(json!({"event": "pending", "pending": Value::Null}));
        }
        p
    }

    fn set_alert(&mut self, text: String) {
        self.push_event(json!({"event": "alert", "text": text}));
        self.alert = Some(text);
    }

    /// Crash guard and boot apply.
    fn startup(&mut self) {
        let marker = |p: PathBuf| -> Option<(String, String)> {
            let s = std::fs::read_to_string(&p).ok()?;
            let mut it = s.lines();
            Some((it.next()?.to_string(), it.next().unwrap_or("").to_string()))
        };
        let mut crashed = false;
        // Same boot: the daemon itself restarted; carry on where it was.
        let same_boot_active = marker(self.paths.run("active")).map(|(p, _)| p);
        if let Some((profile, boot)) = marker(self.paths.state("dirty")) {
            if boot == self.boot_id {
                // The daemon died (panic, SIGKILL) during a Keep/Revert
                // window or a test: nobody kept those settings, so start
                // at stock (`restore` should have done it; make sure).
                crashed = true;
                let _ = self.apply_tune(&Tune::default());
                self.set_alert(format!(
                    "omarchy-ocd stopped unexpectedly while \"{profile}\" was unconfirmed or under test. Started at stock settings."
                ));
            } else {
                crashed = true;
                if let Some(t) = self.cfg.profile(&profile) {
                    let s = self.cfg.stability_mut(&profile, &t);
                    s.crashes += 1;
                    s.confirmed = false;
                }
                self.set_alert(format!(
                    "The last boot ended while \"{profile}\" was unconfirmed or under test. Started at stock settings."
                ));
            }
            let _ = std::fs::remove_file(self.paths.state("dirty"));
        }
        if let Some((profile, boot)) = marker(self.paths.state("running")) {
            if boot != self.boot_id && !crashed {
                if let Some(t) = self.cfg.profile(&profile).filter(|t| self.risky(t)) {
                    let s = self.cfg.stability_mut(&profile, &t);
                    s.crashes += 1;
                    let crashes = s.crashes;
                    if crashes >= 2 {
                        crashed = true;
                        self.cfg.boot_apply = false;
                        self.set_alert(format!(
                            "\"{profile}\" was active during {crashes} unclean shutdowns in a row. Apply-at-boot is now off."
                        ));
                    }
                }
            }
        }
        self.save_cfg();
        let target = if let Some(p) = same_boot_active.filter(|p| !crashed && self.cfg.profile_exists(p)) {
            Some((p, Origin::Resume))
        } else if self.cfg.boot_apply && !crashed && self.cfg.base != "stock" {
            Some((self.cfg.base.clone(), Origin::Boot))
        } else {
            None
        };
        if let Some((p, origin)) = target {
            match self.apply_profile(&p, origin) {
                Ok(_) => self.message = format!("Applied \"{p}\" at start"),
                Err(e) if origin == Origin::Resume => {
                    // Still on the hardware if `restore` did not run.
                    let _ = self.apply_tune(&Tune::default());
                    self.message = e;
                    self.set_alert(format!(
                        "omarchy-ocd restarted while \"{p}\" was active but never kept. Started at stock settings."
                    ));
                }
                Err(e) => self.message = e,
            }
        }
        self.write_running();
    }

    fn write_running(&self) {
        let body = format!("{}\n{}\n", self.active, self.boot_id);
        let _ = write_atomic(&self.paths.state("running"), body.as_bytes(), 0o644);
        let _ = write_atomic(&self.paths.run("active"), body.as_bytes(), 0o644);
    }

    fn set_dirty(&self, profile: &str) {
        let body = format!("{profile}\n{}\n", self.boot_id);
        let _ = write_atomic(&self.paths.state("dirty"), body.as_bytes(), 0o644);
        // Make sure it is on disk before anything risky happens.
        if let Ok(f) = std::fs::File::open(self.paths.state("dirty")) {
            let _ = f.sync_all();
        }
        if let Ok(d) = std::fs::File::open(&self.paths.state_dir) {
            let _ = d.sync_all();
        }
    }

    fn clear_dirty(&self) {
        if self.pending.is_none() && self.stress.is_none() {
            let _ = std::fs::remove_file(self.paths.state("dirty"));
        }
    }

    pub fn apply_tune(&mut self, t: &Tune) -> Report {
        let mut writes = self.cpu.plan(&t.cpu);
        if let Some(g) = &self.gpu {
            writes.extend(g.plan(&t.gpu));
        } else if t.gpu != Default::default() {
            // nothing to do
        }
        writes.extend(self.platform.plan(&t.platform));
        let mut r = self.applier.execute(&writes);
        match &self.smu {
            Some(smu) => {
                let rep = crate::smu::apply(smu.as_ref(), &t.smu, &self.smu_applied, self.smu_stock.as_ref());
                r.changed.extend(rep.sent.into_iter().map(|s| format!("smu {s}")));
                r.errors.extend(rep.errors);
                // A field whose command went through no longer waits for a reboot.
                self.needs_reboot.retain(|n| !rep.done.contains(n));
                let mut new = Vec::new();
                for x in rep.needs_reboot {
                    if !self.needs_reboot.contains(&x) {
                        self.needs_reboot.push(x.clone());
                        new.push(x);
                    }
                }
                if !new.is_empty() {
                    self.set_alert(format!("Could not put the SMU back to stock for {}. Reboot to restore it.", new.join(", ")));
                }
                // Failed fields stay recorded so a later stop or restore retries them.
                self.smu_applied = rep.applied;
                if let Ok(b) = serde_json::to_vec_pretty(&self.smu_applied) {
                    let _ = write_atomic(&self.paths.run("smu-applied.json"), &b, 0o644);
                }
            }
            None if !t.smu.is_empty() => {
                r.errors.push(format!("SMU settings skipped: {}", self.smu_error.clone().unwrap_or_default()));
            }
            None => {}
        }
        if let Some(fp) = &t.fans.profile {
            self.fans.sample();
            if self.fans.available() {
                match self.fans.set_profile(fp) {
                    Ok(()) => r.changed.push(format!("fans={fp}")),
                    Err(e) => r.errors.push(e),
                }
            } else {
                r.notes.push("omarchy-fand is not running; fan profile skipped".into());
            }
        }
        r
    }

    pub fn apply_profile(&mut self, name: &str, origin: Origin) -> Result<Value, String> {
        let t = self.cfg.profile(name).ok_or_else(|| format!("no profile named \"{name}\""))?;
        let risky = self.risky(&t);
        let confirmed = self.cfg.stable(name, &t).is_some_and(|s| s.confirmed);
        if risky && !confirmed && matches!(origin, Origin::Boot | Origin::Rule | Origin::Resume) {
            return Err(format!(
                "\"{name}\" has unconfirmed overclock settings; apply it by hand and keep it once before it can apply {}",
                match origin {
                    Origin::Boot => "at boot",
                    Origin::Resume => "after a daemon restart",
                    _ => "automatically",
                }
            ));
        }
        // The safe profile to go back to is the last one that was not pending.
        let previous = match &self.pending {
            Some(p) => p.previous.clone(),
            None => self.active.clone(),
        };
        if risky {
            self.set_dirty(name);
        }
        if origin == Origin::User {
            // Before the apply, which may raise an alert of its own.
            self.alert = None;
        }
        let r = self.apply_tune(&t);
        self.active = name.to_string();
        self.health_base = self.health.counts();
        self.hot_secs = 0;
        self.risky_since = risky.then(now);
        if origin == Origin::User {
            self.cfg.base = name.to_string();
            self.auto_profile = None;
            self.auto_blocked = None;
        }
        self.set_pending(if risky && !confirmed && origin == Origin::User {
            Some(Pending { profile: name.to_string(), previous, deadline: now() + self.cfg.confirm_seconds as f64 })
        } else {
            None
        });
        self.clear_dirty();
        self.save_cfg();
        self.write_running();
        let summary = if r.errors.is_empty() {
            format!("Applied \"{name}\"")
        } else {
            format!("Applied \"{name}\" with {} error(s): {}", r.errors.len(), r.errors.join("; "))
        };
        self.message = summary;
        let out = json!({"profile": name, "pending": self.pending.is_some(), "report": report_json(&r)});
        self.last_report = out.clone();
        Ok(out)
    }

    fn confirm(&mut self) -> Result<Value, String> {
        let p = self.take_pending().ok_or("nothing to confirm")?;
        if let Some(t) = self.cfg.profile(&p.profile) {
            let s = self.cfg.stability_mut(&p.profile, &t);
            s.confirmed = true;
            s.crashes = 0;
        }
        self.clear_dirty();
        self.save_cfg();
        self.message = format!("Kept \"{}\"", p.profile);
        Ok(json!({"profile": p.profile}))
    }

    fn revert(&mut self, why: &str) -> Result<Value, String> {
        let p = self.take_pending().ok_or("nothing to revert")?;
        let r = self.apply_profile(&p.previous, Origin::Revert);
        self.cfg.base = p.previous.clone();
        self.save_cfg();
        self.message = format!("Reverted to \"{}\" ({why})", p.previous);
        r
    }

    /// Where the watchdog goes: the configured fallback while it exists and
    /// is safe, else stock (a fallback edited into an overclock later is
    /// no refuge).
    fn fallback_profile(&self) -> String {
        let fb = &self.cfg.watchdog.fallback;
        match self.cfg.profile(fb) {
            Some(t) if !self.risky(&t) => fb.clone(),
            _ => "stock".into(),
        }
    }

    fn fallback(&mut self, reason: String) {
        let fb = self.fallback_profile();
        self.set_pending(None);
        if let Some(a) = self.auto_profile.clone() {
            self.auto_blocked = Some(a);
        }
        let prev = self.active.clone();
        // Safety beats the test: a run the watchdog had to cut short is a
        // failed verdict for the profile it ran on.
        if let Some(st) = &self.stress {
            let kind = st.kind.clone();
            if let Some(mut t) = self.test.take() {
                t.stop();
            }
            let r = json!({"passed": false, "errors": 0, "detail": format!("watchdog stepped in: {reason}"), "completed": true});
            self.finish_test(&kind, r, false);
        }
        let _ = self.apply_profile(&fb, Origin::Watchdog);
        self.set_alert(format!("Watchdog: {reason}. Switched from \"{prev}\" to \"{fb}\"."));
        self.message = self.alert.clone().unwrap_or_default();
    }

    // ---- periodic work ---------------------------------------------------

    pub fn tick(&mut self) {
        let t = now();
        self.cpu.sample(t);
        if let Some(g) = &mut self.gpu {
            g.sample(t, true);
        }
        self.mem.sample();
        self.platform.sample();
        self.fans.sample();
        if let Some(s) = &self.smu {
            match s.read_table(&self.sys) {
                Ok(p) => {
                    self.pm = Some(p);
                    self.pm_error = None;
                }
                Err(e) => self.pm_error = Some(e),
            }
        }
        self.push_history(t);
        if !self.manage {
            return;
        }
        self.poll_test();
        self.reap_cli_test();

        if let Some(p) = &self.pending {
            if t >= p.deadline {
                let _ = self.revert("not confirmed in time");
            }
        }
        self.watchdog();
        if t - self.last_auto >= 2.0 {
            self.last_auto = t;
            self.auto_switch();
        }
        // Resume from suspend: boot-time clock jumps ahead of monotonic.
        let (mono, boot) = crate::util::clocks();
        let gap = boot - mono;
        if gap - self.last_gap > 2.0 {
            self.last_gap = gap;
            self.smu_applied = SmuTune::default();
            if let Some(tn) = self.cfg.profile(&self.active.clone()) {
                let r = self.apply_tune(&tn);
                self.message = format!("Re-applied \"{}\" after resume{}", self.active, if r.errors.is_empty() { "" } else { " (with errors)" });
            }
        }
        self.last_gap = gap;
        // A risky profile that ran 30 min without a crash clears its count.
        if let Some(since) = self.risky_since {
            if t - since > 1800.0 {
                self.risky_since = None;
                if let Some(tn) = self.cfg.profile(&self.active) {
                    let s = self.cfg.stability_mut(&self.active.clone(), &tn);
                    if s.crashes > 0 {
                        s.crashes = 0;
                        self.save_cfg();
                    }
                }
            }
        }
        self.write_status();
    }

    fn push_history(&mut self, t: f64) {
        let c = &self.cpu.reading;
        let g = self.gpu.as_ref().map(|g| &g.reading);
        let gf = |k: &str| g.and_then(|r| r[k].as_f64());
        let ppt = self.pm.as_ref().and_then(|p| p.fast_value.or(p.stapm_value)).map(|v| v as f64);
        let gpu_fan = g.and_then(|r| r["fans"].as_array().and_then(|a| a.iter().filter_map(|f| f["rpm"].as_f64()).reduce(f64::max)));
        let fan_cpu = self.fans.status["fans"]
            .as_array()
            .and_then(|a| a.iter().find(|f| f["role"] == "cpu").and_then(|f| f["rpm"].as_f64()));
        let mem_pct = self.mem.reading()["used_pct"].as_f64();
        let vals: [Option<f64>; SERIES.len()] = [
            Some(c.mhz_avg),
            Some(c.mhz_max),
            Some(c.util),
            c.temp,
            c.package_w,
            ppt,
            gf("act_mhz"),
            gf("temp"),
            gf("vram_temp"),
            gf("power_w"),
            gf("busy"),
            gpu_fan,
            mem_pct,
            fan_cpu,
        ];
        self.history.push(t, vals);
    }

    fn watchdog(&mut self) {
        let wd = self.cfg.watchdog.clone();
        if !wd.enabled || self.active == self.fallback_profile() || self.active == "stock" {
            self.hot_secs = 0;
            return;
        }
        let ct = self.cpu.reading.temp.unwrap_or(0.0);
        let gt = self.gpu.as_ref().and_then(|g| g.reading["temp"].as_f64()).unwrap_or(0.0);
        let hot = ct >= wd.cpu_max_c || gt >= wd.gpu_max_c;
        self.hot_secs = if hot { self.hot_secs + 1 } else { 0 };
        if self.hot_secs >= wd.seconds.max(1) {
            let what = if ct >= wd.cpu_max_c { format!("CPU at {ct:.0} °C") } else { format!("GPU at {gt:.0} °C") };
            self.fallback(format!("{what} for {} s", wd.seconds));
            return;
        }
        if wd.on_errors {
            let tune = self.cfg.profile(&self.active).unwrap_or_default();
            if self.risky(&tune) {
                let (m, g) = self.health.counts();
                if m > self.health_base.0 {
                    self.fallback("the kernel logged a machine check (hardware) error".into());
                } else if g > self.health_base.1 {
                    self.fallback("the GPU hung or reset".into());
                }
            }
        }
    }

    fn auto_switch(&mut self) {
        if !self.cfg.auto_switch || self.cfg.rules.is_empty() || self.pending.is_some() || self.stress.is_some() {
            return;
        }
        let procs = processes(&self.sys, self.cfg.rules.iter().any(|r| r.matcher == "gamemode"));
        let want = self.cfg.rules.iter().find(|r| rule_matches(r, &procs)).map(|r| r.profile.clone());
        if want == self.auto_profile {
            return;
        }
        match &want {
            Some(p) if self.auto_blocked.as_ref() == Some(p) => return,
            Some(p) => match self.apply_profile(p, Origin::Rule) {
                Ok(_) => self.message = format!("Auto: switched to \"{p}\""),
                Err(e) => {
                    self.message = format!("Auto: {e}");
                    self.auto_blocked = Some(p.clone());
                    return;
                }
            },
            None => {
                let base = self.cfg.base.clone();
                let _ = self.apply_profile(&base, Origin::Rule);
                self.message = format!("Auto: back to \"{base}\"");
                self.auto_blocked = None;
            }
        }
        self.auto_profile = want;
    }

    // ---- output ------------------------------------------------------------

    fn write_info(&self) {
        let info = json!({
            "version": VERSION,
            "cpu": self.cpu.ident,
            "cpu_tiers": self.cpu.tier_counts(),
            "threads": self.cpu.threads,
            "gpu": self.gpu.as_ref().map(|g| g.ident.clone()),
            "mem": self.mem.info(),
            "platform": self.platform.ident,
            "smu": {"family": self.smu_family, "available": self.smu.is_some(), "error": self.smu_error, "stock": self.smu_stock},
        });
        if let Ok(b) = serde_json::to_vec_pretty(&info) {
            let _ = write_atomic(&self.paths.run("info.json"), &b, 0o644);
        }
    }

    pub fn status(&self, history: usize) -> Value {
        let t = now();
        let profiles: Vec<Value> = self
            .cfg
            .names()
            .into_iter()
            .filter_map(|n| {
                let tune = self.cfg.profile(&n)?;
                let st = self.cfg.stable(&n, &tune).cloned().unwrap_or_default();
                Some(json!({
                    "name": n,
                    "builtin": Config::is_builtin(&n),
                    "risky": self.risky(&tune),
                    "confirmed": st.confirmed,
                    "crashes": st.crashes,
                    "cpu_test": st.cpu,
                    "gpu_test": st.gpu,
                    "tune": tune,
                }))
            })
            .collect();
        let (mce, hangs) = self.health.counts();
        json!({
            "time": round1(t),
            "version": VERSION,
            "active": self.active,
            "base": self.cfg.base,
            "auto_profile": self.auto_profile,
            "boot_apply": self.cfg.boot_apply,
            "auto_switch": self.cfg.auto_switch,
            "pending": self.pending_json(),
            "message": self.message,
            "alert": self.alert,
            "caps": {
                "smu": self.smu.is_some(),
                "smu_family": self.smu_family,
                "smu_error": self.smu_error,
                "pm_table": self.pm.is_some(),
                "pm_error": self.pm_error,
                "gpu": self.gpu.is_some(),
                "fans": self.fans.available(),
            },
            "cpu": {"model": self.cpu.ident.model, "reading": self.cpu.reading, "knobs": self.cpu.knobs(),
                    "cores": self.cpu.threads.iter().filter(|t| t.cpu == *self.cpu.threads.iter().filter(|u| u.core == t.core).map(|u| &u.cpu).min().unwrap_or(&t.cpu)).map(|t| json!({"core": t.core, "tier": t.tier, "cpu": t.cpu})).collect::<Vec<_>>()},
            "smu": {"limits": self.pm, "stock": self.smu_stock, "applied": self.smu_applied, "needs_reboot": self.needs_reboot},
            "gpu": self.gpu.as_ref().map(|g| json!({"name": g.ident["name"], "reading": g.reading, "knobs": g.knobs(), "procs": g.procs,
                                                   "stock_power_w": self.stock_gpu_w()})),
            "mem": {"reading": self.mem.reading()},
            "platform": {"reading": self.platform.reading, "knobs": self.platform.knobs()},
            "fans": self.fans.summary(),
            "profiles": profiles,
            "rules": self.cfg.rules,
            "watchdog": self.cfg.watchdog,
            "metrics": {"http": self.cfg.metrics.http, "listen": self.cfg.metrics.listen, "serving": self.metrics_serving},
            "confirm_seconds": self.cfg.confirm_seconds,
            "stress": self.stress.as_ref().map(|s| json!({"kind": s.kind, "started": s.started, "profile": s.profile, "seconds": s.seconds})),
            "health": {"mce": mce, "gpu_hangs": hangs, "recent": self.health.last.lock().map(|v| v.clone()).unwrap_or_default()},
            "last_report": self.last_report,
            "history": self.history.tail(history),
        })
    }

    fn write_status(&mut self) {
        let st = self.status(180);
        if let Ok(b) = serde_json::to_vec(&st) {
            let _ = write_atomic(&self.paths.run("status.json"), &b, 0o644);
        }
        let prom = crate::metrics::render(&st);
        let _ = write_atomic(&self.paths.run("metrics.prom"), prom.as_bytes(), 0o644);
        if let Ok(mut l) = self.metrics_latest.lock() {
            *l = prom;
        }
    }

    pub fn start_metrics(&mut self) {
        if self.cfg.metrics.http && !self.metrics_serving {
            match crate::metrics::serve(&self.cfg.metrics.listen, self.metrics_latest.clone()) {
                Ok(()) => self.metrics_serving = true,
                Err(e) => self.message = format!("metrics listener {}: {e}", self.cfg.metrics.listen),
            }
        }
    }

    /// Hand everything back (daemon exit).
    pub fn shutdown(&mut self) {
        // A test of our own ends with the daemon: a clean stop is not a
        // crash, so it leaves no verdict and no dirty marker behind.
        if let Some(mut t) = self.test.take() {
            t.stop();
            self.stress = None;
        }
        let _ = self.apply_tune(&Tune::default());
        let _ = std::fs::remove_file(self.paths.state("running"));
        let _ = std::fs::remove_file(self.paths.run("active"));
        if self.pending.is_none() && self.stress.is_none() {
            let _ = std::fs::remove_file(self.paths.state("dirty"));
        }
    }

    // ---- stability tests -----------------------------------------------------

    /// A test starts on the active profile: refuse while settings are
    /// pending or another test runs; set the dirty marker so a crash under
    /// test counts against the profile.
    fn begin_test(&mut self, kind: &str, seconds: Option<u32>, pid: Option<u32>) -> Result<Value, String> {
        if self.stress.is_some() {
            return Err("a test is already running".into());
        }
        if self.pending.is_some() {
            return Err("confirm or revert the pending settings first".into());
        }
        self.set_dirty(&self.active.clone());
        self.stress = Some(StressState { kind: kind.to_string(), started: now(), profile: self.active.clone(), seconds, pid });
        self.health_base = self.health.counts();
        Ok(json!({"profile": self.active}))
    }

    /// The test that `begin_test` started is over: record its verdict
    /// against the profile it ran on. `r` carries `passed`, `errors`,
    /// `seconds`, `detail` and `completed` (false when the run was cut
    /// short). Kernel-reported hardware errors during the run (machine
    /// checks; GPU hangs for a GPU test) count as failures too. A run cut
    /// short without errors says nothing and is not recorded. Returns
    /// `{profile, result, recorded}`.
    pub fn record_test(&mut self, kind: &str, r: &Value) -> Result<Value, String> {
        let st = self.stress.take().ok_or("no stress test running")?;
        let (m, g) = self.health.counts();
        let hw_errors = (m - self.health_base.0) + if kind == "gpu" { g - self.health_base.1 } else { 0 };
        let errors = r.get("errors").and_then(Value::as_u64).unwrap_or(0) + hw_errors;
        let completed = r.get("completed").and_then(Value::as_bool).unwrap_or(true);
        let passed = r.get("passed").and_then(Value::as_bool).unwrap_or(false) && hw_errors == 0;
        let mut detail = r.get("detail").and_then(Value::as_str).unwrap_or("").to_string();
        if hw_errors > 0 {
            detail = format!("{detail} {hw_errors} kernel-reported hardware error(s)").trim().to_string();
        }
        let res = TestResult {
            time: now(),
            seconds: r.get("seconds").and_then(Value::as_f64).unwrap_or(now() - st.started),
            passed,
            errors,
            detail,
        };
        let recorded = completed || !passed;
        if recorded && let Some(t) = self.cfg.profile(&st.profile) {
            let rec: &mut Stability = self.cfg.stability_mut(&st.profile, &t);
            if kind == "gpu" {
                rec.gpu = Some(res.clone());
            } else {
                rec.cpu = Some(res.clone());
            }
        }
        self.clear_dirty();
        self.save_cfg();
        Ok(json!({"profile": st.profile, "result": res, "recorded": recorded}))
    }

    /// Each tick: relay the daemon-run test's progress lines as
    /// `test-progress` events; when it ends, record the verdict and push
    /// `test-done`.
    fn poll_test(&mut self) {
        let Some(run) = self.test.as_mut() else { return };
        let progress = run.poll();
        let fin = run.finished();
        for mut ev in progress {
            ev["event"] = json!("test-progress");
            self.push_event(ev);
        }
        let Some(fin) = fin else { return };
        let run = self.test.take().expect("a run was polled");
        let kind = run.kind.clone();
        let exit = fin["exit"].as_i64().unwrap_or(2);
        let errors = fin["errors"].as_u64().unwrap_or(0);
        let message = fin["message"].as_str().map(str::to_string);
        // The CPU tester says "not passed" when stopped; gpustress keeps
        // "passed" and sets "interrupted". Treat both as cut short.
        let interrupted = fin["interrupted"] == json!(true) || run.stopped() || message.as_deref() == Some("stopped before the end");
        let could_not_run = exit == 2;
        let completed = !interrupted && !could_not_run;
        // Cut short with no errors: no verdict (record_test then skips it).
        let passed = errors == 0 && !could_not_run && (fin["passed"] == json!(true) || interrupted);
        let detail = message.unwrap_or_else(|| {
            if kind == "gpu" {
                format!("{} GFLOPS, {} GB/s", round1(fin["gflops"].as_f64().unwrap_or(0.0)), round1(fin["vram_gbps"].as_f64().unwrap_or(0.0)))
            } else {
                format!("{}, {} blocks", fin["isa"].as_str().unwrap_or("?"), fin["blocks"].as_u64().unwrap_or(0))
            }
        });
        let r = json!({"passed": passed, "errors": errors, "seconds": fin["t"], "detail": detail, "completed": completed});
        self.finish_test(&kind, r, interrupted);
    }

    /// A CLI run whose process is gone without `stress-end` (killed with
    /// -9) says nothing about the profile: drop it, no verdict, and clear
    /// the dirty marker it set.
    fn reap_cli_test(&mut self) {
        let Some(st) = &self.stress else { return };
        let Some(pid) = st.pid else { return };
        if self.test.is_some() || self.sys.exists(format!("/proc/{pid}")) {
            return;
        }
        let (kind, profile) = (st.kind.clone(), st.profile.clone());
        self.stress = None;
        self.clear_dirty();
        self.message = format!("The {kind} test on \"{profile}\" ended without a result (its process is gone); not recorded");
    }

    /// Record a test's verdict `r` (the `record_test` shape) and push
    /// `test-done`.
    fn finish_test(&mut self, kind: &str, r: Value, interrupted: bool) {
        let errors = r["errors"].as_u64().unwrap_or(0);
        let ev = match self.record_test(kind, &r) {
            Ok(v) => {
                let recorded = v["recorded"] == json!(true);
                let res = &v["result"];
                self.message = format!(
                    "{} test {} on \"{}\"",
                    kind,
                    if interrupted && recorded { "stopped with errors" } else if interrupted { "stopped" } else if res["passed"] == json!(true) { "passed" } else { "FAILED" },
                    v["profile"].as_str().unwrap_or("")
                );
                json!({
                    "event": "test-done", "kind": kind, "profile": v["profile"],
                    "passed": recorded && res["passed"] == json!(true), "errors": res["errors"],
                    "interrupted": interrupted, "recorded": recorded, "seconds": res["seconds"], "detail": res["detail"],
                })
            }
            Err(e) => json!({"event": "test-done", "kind": kind, "passed": false, "errors": errors, "interrupted": interrupted, "recorded": false, "detail": e}),
        };
        self.push_event(ev);
    }

    /// Nothing may change the hardware under a running test: its verdict
    /// belongs to the settings it started on.
    fn no_test_running(&self) -> Result<(), String> {
        match &self.stress {
            Some(st) => Err(format!("stop the test first (a {} stability test is running)", st.kind)),
            None => Ok(()),
        }
    }

    // ---- requests ------------------------------------------------------------

    pub fn handle(&mut self, req: &Value) -> Result<Value, String> {
        let s = |k: &str| req.get(k).and_then(Value::as_str);
        let b = |k: &str| req.get(k).and_then(Value::as_bool);
        let name_arg = |k: &str| -> Result<String, String> {
            let n = s(k).ok_or_else(|| format!("missing \"{k}\""))?;
            Ok(n.to_string())
        };
        match s("cmd").unwrap_or("") {
            "status" => Ok(self.status(req.get("history").and_then(Value::as_u64).unwrap_or(180) as usize)),
            "info" => {
                self.write_info();
                read_json(&self.paths.run("info.json")).ok_or_else(|| "no info".into())
            }
            "apply" => {
                let p = name_arg("profile")?;
                self.no_test_running()?;
                self.apply_profile(&p, Origin::User)
            }
            "confirm" => self.confirm(),
            "revert" => self.revert("by request"),
            "set" => {
                let p = name_arg("profile")?;
                let key = name_arg("key")?;
                let val = s("value").unwrap_or("").to_string();
                if Config::is_builtin(&p) {
                    return Err(format!("\"{p}\" is built in; copy it first (omarchy-oc copy {p} my-{p})"));
                }
                let mut t = self.cfg.profiles.get(&p).cloned().ok_or_else(|| format!("no profile named \"{p}\""))?;
                t.set_key(&key, &val)?;
                if self.active == p {
                    self.no_test_running()?;
                }
                self.cfg.profiles.insert(p.clone(), t);
                self.save_cfg();
                self.reapply_if_active(&p)
            }
            "edit" => {
                // The panel's live edit: a batch of dotted keys. Safe changes
                // apply at once when the profile is active; risky ones need
                // "apply": true and open the Keep/Revert window. Editing a
                // built-in forks it into "custom" (or the first free
                // "custom-N" when "custom" holds edits), which then becomes
                // active; the reply's "profile" names it.
                let p = name_arg("profile")?;
                let changes = req.get("changes").and_then(Value::as_object).ok_or("missing \"changes\"")?;
                let apply = b("apply").unwrap_or(false);
                let mut t = self.cfg.profile(&p).ok_or_else(|| format!("no profile named \"{p}\""))?;
                // All or nothing: the whole batch is validated on a copy first.
                for (k, v) in changes {
                    t.set_key(k, &value_text(v))?;
                }
                let risky = self.risky_keys(changes);
                if !risky.is_empty() && !apply {
                    return Err(format!("risky changes need Apply: {}", risky.join(", ")));
                }
                let forked = Config::is_builtin(&p);
                let target = if forked { self.fork_name() } else { p };
                let applies = forked || !risky.is_empty() || self.active == target;
                if apply || applies {
                    self.no_test_running()?;
                }
                self.cfg.profiles.insert(target.clone(), t);
                self.save_cfg();
                if applies {
                    self.apply_profile(&target, Origin::User)
                } else {
                    Ok(json!({
                        "profile": target, "pending": false, "applied": false,
                        "report": {"changed": [], "errors": [], "notes": [format!("saved; \"{target}\" is not active")]},
                    }))
                }
            }
            "save" => {
                let p = name_arg("profile")?;
                if Config::is_builtin(&p) {
                    return Err(format!("\"{p}\" is built in"));
                }
                if !valid_name(&p) {
                    return Err("profile names are letters, digits, - and _ (max 32)".into());
                }
                let t: Tune = serde_json::from_value(req.get("tune").cloned().unwrap_or_default()).map_err(|e| format!("tune: {e}"))?;
                t.validate()?;
                if self.active == p {
                    self.no_test_running()?;
                }
                self.cfg.profiles.insert(p.clone(), t);
                self.save_cfg();
                self.reapply_if_active(&p)
            }
            "copy" => {
                let from = name_arg("from")?;
                let to = name_arg("to")?;
                if !valid_name(&to) || Config::is_builtin(&to) {
                    return Err(format!("\"{to}\" is not a usable profile name"));
                }
                let t = self.cfg.profile(&from).ok_or_else(|| format!("no profile named \"{from}\""))?;
                self.cfg.profiles.insert(to.clone(), t);
                self.save_cfg();
                Ok(json!({"profile": to}))
            }
            "rename" => {
                let from = name_arg("from")?;
                let to = name_arg("to")?;
                if !valid_name(&to) || Config::is_builtin(&to) || self.cfg.profile_exists(&to) {
                    return Err(format!("\"{to}\" is not a usable profile name"));
                }
                let t = self.cfg.profiles.remove(&from).ok_or_else(|| format!("no user profile named \"{from}\""))?;
                self.cfg.profiles.insert(to.clone(), t);
                if let Some(st) = self.cfg.stability.remove(&from) {
                    self.cfg.stability.insert(to.clone(), st);
                }
                for r in &mut self.cfg.rules {
                    if r.profile == from {
                        r.profile = to.clone();
                    }
                }
                if self.cfg.base == from {
                    self.cfg.base = to.clone();
                }
                if self.active == from {
                    self.active = to.clone();
                }
                self.save_cfg();
                Ok(json!({"profile": to}))
            }
            "delete" => {
                let p = name_arg("profile")?;
                if self.active == p {
                    return Err("switch to another profile first".into());
                }
                self.cfg.profiles.remove(&p).ok_or_else(|| format!("no user profile named \"{p}\""))?;
                self.cfg.stability.remove(&p);
                self.cfg.rules.retain(|r| r.profile != p);
                self.save_cfg();
                Ok(json!({}))
            }
            "boot" => {
                self.cfg.boot_apply = b("enabled").ok_or("missing \"enabled\"")?;
                self.save_cfg();
                Ok(json!({"boot_apply": self.cfg.boot_apply}))
            }
            "auto" => {
                self.cfg.auto_switch = b("enabled").ok_or("missing \"enabled\"")?;
                if !self.cfg.auto_switch && self.auto_profile.take().is_some() {
                    let base = self.cfg.base.clone();
                    let _ = self.apply_profile(&base, Origin::Rule);
                }
                self.auto_blocked = None;
                self.save_cfg();
                Ok(json!({"auto_switch": self.cfg.auto_switch}))
            }
            "rule-add" => {
                let matcher = name_arg("matcher")?;
                let profile = name_arg("profile")?;
                if !(matcher.starts_with("process:") && matcher.len() > 8 || matcher == "steam-game" || matcher == "gamemode") {
                    return Err("matcher must be process:<glob>, steam-game or gamemode".into());
                }
                if !self.cfg.profile_exists(&profile) {
                    return Err(format!("no profile named \"{profile}\""));
                }
                self.cfg.rules.push(Rule { matcher, profile });
                self.save_cfg();
                Ok(json!({"rules": self.cfg.rules}))
            }
            "rule-del" => {
                let i = req.get("index").and_then(Value::as_u64).ok_or("missing \"index\"")? as usize;
                if i >= self.cfg.rules.len() {
                    return Err("no such rule".into());
                }
                self.cfg.rules.remove(i);
                self.save_cfg();
                Ok(json!({"rules": self.cfg.rules}))
            }
            "rule-set" => {
                // Change a rule's profile where it is: first match wins, so
                // the panel must not have to delete and re-add (reorder) it.
                let i = req.get("index").and_then(Value::as_u64).ok_or("missing \"index\"")? as usize;
                let profile = name_arg("profile")?;
                if !self.cfg.profile_exists(&profile) {
                    return Err(format!("no profile named \"{profile}\""));
                }
                let r = self.cfg.rules.get_mut(i).ok_or("no such rule")?;
                r.profile = profile;
                self.save_cfg();
                Ok(json!({"rules": self.cfg.rules}))
            }
            "processes" => {
                // For the panel's "add a rule" picker: the names of the
                // running programs (kernel threads skipped), unique, sorted.
                let mut names: Vec<String> = processes(&self.sys, false).into_iter().map(|p| p.comm).collect();
                names.sort();
                names.dedup();
                names.truncate(200);
                Ok(Value::Array(names.into_iter().map(|n| json!({"name": n})).collect()))
            }
            "watchdog" => {
                // Checked first: a refused request changes nothing.
                if let Some(v) = s("fallback") {
                    let t = self.cfg.profile(v).ok_or_else(|| format!("no profile named \"{v}\""))?;
                    if self.risky(&t) {
                        return Err(format!("\"{v}\" has overclock settings; the watchdog fallback must be a safe profile"));
                    }
                }
                let w = &mut self.cfg.watchdog;
                if let Some(v) = b("enabled") {
                    w.enabled = v;
                }
                if let Some(v) = b("on_errors") {
                    w.on_errors = v;
                }
                if let Some(v) = req.get("cpu_max_c").and_then(Value::as_f64) {
                    w.cpu_max_c = v.clamp(60.0, 105.0);
                }
                if let Some(v) = req.get("gpu_max_c").and_then(Value::as_f64) {
                    w.gpu_max_c = v.clamp(60.0, 110.0);
                }
                if let Some(v) = req.get("seconds").and_then(Value::as_u64) {
                    w.seconds = (v as u32).clamp(1, 120);
                }
                if let Some(v) = s("fallback") {
                    self.cfg.watchdog.fallback = v.to_string();
                }
                self.save_cfg();
                Ok(serde_json::to_value(&self.cfg.watchdog).unwrap_or_default())
            }
            "metrics" => {
                if let Some(v) = b("http") {
                    self.cfg.metrics.http = v;
                }
                if let Some(v) = s("listen") {
                    let a: std::net::SocketAddr = v.parse().map_err(|_| format!("bad address {v}"))?;
                    if !a.ip().is_loopback() {
                        return Err("the metrics listener only binds loopback addresses".into());
                    }
                    self.cfg.metrics.listen = v.to_string();
                }
                self.save_cfg();
                self.start_metrics();
                Ok(json!({"http": self.cfg.metrics.http, "listen": self.cfg.metrics.listen, "serving": self.metrics_serving,
                          "note": if self.metrics_serving && !self.cfg.metrics.http { "listener stops at next daemon restart" } else { "" }}))
            }
            "confirm-seconds" => {
                let v = req.get("seconds").and_then(Value::as_u64).ok_or("missing \"seconds\"")?;
                self.cfg.confirm_seconds = (v as u32).clamp(5, 300);
                self.save_cfg();
                Ok(json!({"confirm_seconds": self.cfg.confirm_seconds}))
            }
            "stress-begin" => {
                let kind = s("kind").unwrap_or("cpu").to_string();
                let pid = req.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok());
                self.begin_test(&kind, None, pid)
            }
            "stress-end" => {
                let kind = match s("kind") {
                    Some(k) => k.to_string(),
                    None => self.stress.as_ref().map(|st| st.kind.clone()).ok_or("no stress test running")?,
                };
                self.record_test(&kind, req)
            }
            "test-start" => {
                // The daemon runs the tester as its own child and streams
                // its progress as events; see `poll_test`.
                let kind = s("kind").unwrap_or("cpu").to_string();
                if !["cpu", "cpu-per-core", "gpu"].contains(&kind.as_str()) {
                    return Err(format!("test kind must be cpu, cpu-per-core or gpu, not \"{kind}\""));
                }
                let seconds = req.get("seconds").and_then(Value::as_u64).unwrap_or(120).clamp(5, 86_400) as u32;
                // Ours, or a CLI run that reported in with stress-begin.
                if self.test.is_some() || self.stress.is_some() {
                    return Err("a test is already running".into());
                }
                if self.pending.is_some() {
                    return Err("confirm or revert the pending settings first".into());
                }
                let cores = self.cpu.threads.iter().map(|t| t.core).collect::<std::collections::BTreeSet<_>>().len();
                let run = TestRun::start(&kind, seconds, cores, &self.paths.bin_dir).map_err(|e| format!("cannot start the {kind} test: {e}"))?;
                let r = self.begin_test(&kind, Some(seconds), None)?;
                self.test = Some(run);
                self.message = format!("Running the {kind} stability test on \"{}\"", self.active);
                Ok(json!({"kind": kind, "seconds": seconds, "profile": r["profile"]}))
            }
            "test-stop" => {
                let t = self.test.as_mut().ok_or("no test running")?;
                t.stop();
                Ok(json!({"kind": t.kind}))
            }
            "history" => Ok(self.history.tail(req.get("seconds").and_then(Value::as_u64).unwrap_or(3600) as usize)),
            "csv" => Ok(Value::String(self.history.csv(req.get("seconds").and_then(Value::as_f64)))),
            "export" => {
                let p = name_arg("profile")?;
                let t = self.cfg.profile(&p).ok_or_else(|| format!("no profile named \"{p}\""))?;
                Ok(json!({"omarchy_oc_profile": 1, "name": p, "tune": t}))
            }
            "fans" => fans_proxy(&self.fans, req),
            "clear-alert" => {
                self.alert = None;
                Ok(json!({}))
            }
            "probe" => Ok(json!({
                "smu_family": self.smu_family, "smu": self.smu.is_some(), "smu_error": self.smu_error,
                "pm_table": self.pm, "pm_error": self.pm_error, "stock": self.smu_stock,
                "originals": self.applier.originals.len(),
            })),
            other => Err(format!("unknown command \"{other}\"")),
        }
    }

    /// Where an edit of a built-in goes: "custom" while it is missing or
    /// still the empty default, else the first free "custom-N" (N = 2..),
    /// so a fork never overwrites a profile the user made.
    fn fork_name(&self) -> String {
        match self.cfg.profiles.get("custom") {
            None => return "custom".into(),
            Some(t) if *t == Tune::default() => return "custom".into(),
            Some(_) => {}
        }
        (2..).map(|n| format!("custom-{n}")).find(|n| !self.cfg.profile_exists(n)).expect("a free name")
    }

    fn reapply_if_active(&mut self, p: &str) -> Result<Value, String> {
        if self.active == p {
            self.no_test_running()?;
            self.apply_profile(p, Origin::User)
        } else {
            Ok(json!({"profile": p, "applied": false}))
        }
    }
}

/// Forward a fan request to omarchy-fand (the fan backend); only its
/// control commands pass.
pub fn fans_proxy(fans: &Fans, req: &Value) -> Result<Value, String> {
    let inner = req.get("req").cloned().ok_or("missing \"req\"")?;
    let c = inner.get("cmd").and_then(Value::as_str).unwrap_or("");
    const ALLOWED: [&str; 9] = ["status", "profile", "curve", "tuning", "fan", "calibrate", "retest", "mode", "rescan"];
    if !ALLOWED.contains(&c) {
        return Err(format!("fans: command \"{c}\" is not allowed"));
    }
    fans.call(&inner)
}

pub struct Proc {
    pub comm: String,
    pub argv0: String,
    pub cmdline: String,
    pub gamemode: bool,
}

pub fn processes(sys: &Sys, want_env: bool) -> Vec<Proc> {
    let mut v = Vec::new();
    for p in sys.list("/proc") {
        if !p.file_name().is_some_and(|n| n.to_string_lossy().bytes().all(|b| b.is_ascii_digit())) {
            continue;
        }
        let Some(comm) = crate::util::read_trim(&p.join("comm")) else { continue };
        let raw = std::fs::read(p.join("cmdline")).unwrap_or_default();
        if raw.is_empty() {
            continue; // kernel thread
        }
        let cmdline = String::from_utf8_lossy(&raw).replace('\0', " ");
        let first = raw.split(|b| *b == 0).next().unwrap_or_default();
        let argv0 = String::from_utf8_lossy(first).rsplit(['/', '\\']).next().unwrap_or("").to_string();
        let gamemode = want_env
            && (comm == "gamemoderun"
                || std::fs::read(p.join("environ")).is_ok_and(|e| e.windows(15).any(|w| w == b"libgamemodeauto")));
        v.push(Proc { comm, argv0, cmdline, gamemode });
    }
    v
}

pub fn rule_matches(r: &Rule, procs: &[Proc]) -> bool {
    if let Some(pat) = r.matcher.strip_prefix("process:") {
        return procs.iter().any(|p| glob(pat, &p.comm) || glob(pat, &p.argv0));
    }
    match r.matcher.as_str() {
        // Steam starts every game under `reaper SteamLaunch AppId=N`.
        "steam-game" => procs.iter().any(|p| p.comm == "reaper" && p.cmdline.contains("SteamLaunch") && !p.cmdline.contains("AppId=0 ")),
        "gamemode" => procs.iter().any(|p| p.gamemode),
        _ => false,
    }
}

/// `omarchy-ocd restore`: put back every snapshotted knob and the SMU stock
/// limits, without a running daemon (systemd ExecStopPost).
pub fn restore(sys: Sys, paths: Paths) -> Vec<String> {
    let mut out = Vec::new();
    // Settings that were unconfirmed or under test must not come back when
    // systemd restarts the daemon: drop its resume marker.
    if paths.state("dirty").exists() && std::fs::remove_file(paths.run("active")).is_ok() {
        out.push("dropped the resume marker (settings were unconfirmed or under test)".into());
    }
    let mut a = Applier::load(&sys, paths.run("original.json"));
    let r = a.restore_all();
    out.extend(r.changed.iter().map(|c| format!("restored {c}")));
    out.extend(r.errors.iter().map(|e| format!("error: {e}")));
    let applied: SmuTune = read_json(&paths.run("smu-applied.json")).unwrap_or_default();
    if !applied.is_empty() {
        let cpu = Cpu::detect(&sys);
        if let Some(f) = crate::smu::supported(&cpu.ident.vendor, cpu.ident.family, cpu.ident.model_id) {
            match Smu::open(&sys, f) {
                Ok(s) => out.extend(restore_smu(&s, &paths)),
                Err(e) => out.push(format!("error: SMU: {e}")),
            }
        }
    }
    out
}

/// Put the SMU limits in run/smu-applied.json back to stock. The record
/// goes only when every field was restored; otherwise it keeps the ones
/// still set, for the next start or restore to retry.
pub fn restore_smu(dev: &dyn SmuIo, paths: &Paths) -> Vec<String> {
    let mut out = Vec::new();
    let applied: SmuTune = read_json(&paths.run("smu-applied.json")).unwrap_or_default();
    if applied.is_empty() {
        return out;
    }
    let stock: Option<PmLimits> = read_json(&paths.run("smu-stock.json"));
    let rep = crate::smu::apply(dev, &SmuTune::default(), &applied, stock.as_ref());
    out.extend(rep.sent.into_iter().map(|x| format!("smu {x}")));
    out.extend(rep.errors.into_iter().map(|e| format!("error: {e}")));
    if !rep.needs_reboot.is_empty() {
        out.push(format!("reboot to restore: {}", rep.needs_reboot.join(", ")));
    }
    if rep.applied.is_empty() {
        let _ = std::fs::remove_file(paths.run("smu-applied.json"));
    } else if let Ok(b) = serde_json::to_vec_pretty(&rep.applied) {
        let _ = write_atomic(&paths.run("smu-applied.json"), &b, 0o644);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(comm: &str, cmd: &str) -> Proc {
        Proc { comm: comm.into(), argv0: cmd.split(' ').next().unwrap_or("").rsplit('/').next().unwrap_or("").into(), cmdline: cmd.into(), gamemode: false }
    }

    #[test]
    fn rules() {
        let procs = vec![p("reaper", "/home/u/.steam/reaper SteamLaunch AppId=1091500 -- foo"), p("blender", "/usr/bin/blender")];
        assert!(rule_matches(&Rule { matcher: "steam-game".into(), profile: "max".into() }, &procs));
        assert!(rule_matches(&Rule { matcher: "process:blend*".into(), profile: "max".into() }, &procs));
        assert!(!rule_matches(&Rule { matcher: "process:gimp".into(), profile: "max".into() }, &procs));
        assert!(!rule_matches(&Rule { matcher: "gamemode".into(), profile: "max".into() }, &procs));
    }
}
