//! Daemon safety logic against a fake sysfs tree: apply, keep/revert,
//! crash guard, boot-apply refusal, restore to stock.

mod common;

use common::{Fake, r, w};
use omarchy_oc::daemon::Core;
use serde_json::{Value, json};

fn call(c: &mut Core, v: Value) -> Result<Value, String> {
    c.handle(&v)
}

#[test]
fn safe_profile_applies_and_restores() {
    let f = Fake::new("safe");
    let mut c = f.core();
    let res = call(&mut c, json!({"cmd": "apply", "profile": "performance"})).unwrap();
    assert_eq!(res["pending"], json!(false), "performance has no overclock settings");
    let cpu = "/sys/devices/system/cpu";
    assert_eq!(r(&f.root, &format!("{cpu}/cpu0/cpufreq/scaling_governor")), "performance");
    assert_eq!(r(&f.root, "/sys/module/pcie_aspm/parameters/policy"), "performance");
    assert_eq!(r(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/tile0/gt0/freq0/min_freq"), "1000");
    // Back to stock restores the snapshot.
    call(&mut c, json!({"cmd": "apply", "profile": "stock"})).unwrap();
    assert_eq!(r(&f.root, &format!("{cpu}/cpu0/cpufreq/scaling_governor")), "powersave");
    assert_eq!(r(&f.root, "/sys/module/pcie_aspm/parameters/policy"), "default");
    assert_eq!(r(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/tile0/gt0/freq0/min_freq"), "600");
}

#[test]
fn risky_profile_needs_confirm_and_reverts() {
    let f = Fake::new("risky");
    let mut c = f.core();
    call(&mut c, json!({"cmd": "apply", "profile": "balanced"})).unwrap();
    let res = call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
    assert_eq!(res["pending"], json!(true));
    // No SMU in the fake tree: reported, not silently ignored.
    assert!(res["report"]["errors"].to_string().contains("SMU"), "{res}");
    assert!(f.dir.join("state/dirty").exists(), "dirty marker guards the unconfirmed state");
    assert_eq!(r(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/hwmon/hwmon2/power2_max"), "190000000");
    // Revert goes back to the last confirmed profile and clears the marker.
    call(&mut c, json!({"cmd": "revert"})).unwrap();
    assert_eq!(c.active, "balanced");
    assert!(!f.dir.join("state/dirty").exists());
    assert_eq!(r(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/hwmon/hwmon2/power2_max"), "150000000");
    // Confirm path marks it kept.
    call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
    call(&mut c, json!({"cmd": "confirm"})).unwrap();
    assert!(!f.dir.join("state/dirty").exists());
    let st = c.status(1);
    let max = st["profiles"].as_array().unwrap().iter().find(|p| p["name"] == "max").unwrap().clone();
    assert_eq!(max["confirmed"], json!(true));
}

#[test]
fn crash_while_unconfirmed_boots_to_stock() {
    let f = Fake::new("crash");
    {
        let mut c = f.core();
        call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
        // Machine dies here: no shutdown(), dirty marker stays.
    }
    f.reboot();
    let c = f.core();
    assert_eq!(c.active, "stock");
    let st = c.status(1);
    assert!(st["alert"].as_str().unwrap_or("").contains("unconfirmed"), "{}", st["alert"]);
    let max = st["profiles"].as_array().unwrap().iter().find(|p| p["name"] == "max").unwrap().clone();
    assert_eq!(max["crashes"], json!(1));
    assert_eq!(max["confirmed"], json!(false));
}

#[test]
fn boot_apply_refuses_untested_overclock_but_applies_kept_one() {
    let f = Fake::new("boot");
    {
        let mut c = f.core();
        // User picks "custom" with an untested CO value, never confirms.
        call(&mut c, json!({"cmd": "set", "profile": "custom", "key": "gpu.power_w", "value": "180"})).unwrap();
        call(&mut c, json!({"cmd": "apply", "profile": "custom"})).unwrap();
        call(&mut c, json!({"cmd": "revert"})).unwrap();
        // ...then the safe performance profile as base, clean shutdown.
        call(&mut c, json!({"cmd": "apply", "profile": "custom"})).unwrap();
        call(&mut c, json!({"cmd": "confirm"})).unwrap();
        c.shutdown();
    }
    f.reboot();
    let c = f.core();
    assert_eq!(c.active, "custom", "a kept overclock applies at boot");
    assert_eq!(r(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/hwmon/hwmon2/power2_max"), "180000000");
    drop(c);

    // Edit it (fingerprint changes → untested again), clean shutdown.
    let f2 = Fake::new("boot2");
    {
        let mut c = f2.core();
        call(&mut c, json!({"cmd": "set", "profile": "custom", "key": "gpu.power_w", "value": "185"})).unwrap();
        call(&mut c, json!({"cmd": "apply", "profile": "custom"})).unwrap();
        // Not confirmed; base is custom; daemon stops cleanly mid-window.
        c.shutdown();
    }
    f2.reboot();
    let c = f2.core();
    assert_eq!(c.active, "stock", "an unconfirmed overclock never applies at boot");
}

#[test]
fn set_rejects_builtin_and_out_of_range() {
    let f = Fake::new("set");
    let mut c = f.core();
    assert!(call(&mut c, json!({"cmd": "set", "profile": "max", "key": "smu.co_all", "value": "-5"})).is_err());
    assert!(call(&mut c, json!({"cmd": "set", "profile": "custom", "key": "smu.co_all", "value": "-90"})).is_err());
    assert!(call(&mut c, json!({"cmd": "fans", "req": {"cmd": "shutdown"}})).is_err());
    call(&mut c, json!({"cmd": "copy", "from": "max", "to": "my-max"})).unwrap();
    call(&mut c, json!({"cmd": "set", "profile": "my-max", "key": "smu.co_cores.2", "value": "-20"})).unwrap();
}

// ---- live-edit endpoint ---------------------------------------------------

#[test]
fn edit_builtin_forks_custom() {
    let f = Fake::new("edit-fork");
    let mut c = f.core();
    call(&mut c, json!({"cmd": "apply", "profile": "balanced"})).unwrap();
    let res = call(&mut c, json!({"cmd": "edit", "profile": "balanced", "changes": {"cpu.epp": "power"}})).unwrap();
    assert_eq!(res["profile"], json!("custom"), "{res}");
    assert_eq!(res["pending"], json!(false), "{res}");
    assert_eq!(c.active, "custom");
    assert_eq!(r(&f.root, "/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference"), "power");
    let custom = &c.cfg.profiles["custom"];
    assert_eq!(custom.cpu.epp.as_deref(), Some("power"));
    assert_eq!(custom.cpu.governor.as_deref(), Some("powersave"), "copied from balanced");
    // The panel classifies gpu.power_w against this.
    let st = c.status(1);
    assert_eq!(st["gpu"]["stock_power_w"], json!(150.0), "{}", st["gpu"]);
}

#[test]
fn edit_risky_needs_apply() {
    let f = Fake::new("edit-risky");
    let mut c = f.core();
    call(&mut c, json!({"cmd": "apply", "profile": "balanced"})).unwrap();
    let err = call(&mut c, json!({"cmd": "edit", "profile": "custom", "changes": {"smu.co_all": "-10"}})).unwrap_err();
    assert!(err.contains("risky changes need Apply"), "{err}");
    assert_eq!(c.cfg.profiles["custom"].smu.co_all, None, "nothing saved without Apply");
    assert_eq!(c.active, "balanced");
    let res = call(&mut c, json!({"cmd": "edit", "profile": "custom", "changes": {"smu.co_all": "-10"}, "apply": true})).unwrap();
    assert_eq!(res["profile"], json!("custom"), "{res}");
    assert_eq!(res["pending"], json!(true), "{res}");
    assert_eq!(c.active, "custom");
    assert_eq!(c.cfg.profiles["custom"].smu.co_all, Some(-10));
    // The Keep/Revert previous is the profile that was active before the edit.
    call(&mut c, json!({"cmd": "revert"})).unwrap();
    assert_eq!(c.active, "balanced");
}

#[test]
fn edit_targets_named_profile() {
    let f = Fake::new("edit-named");
    let mut c = f.core();
    call(&mut c, json!({"cmd": "apply", "profile": "performance"})).unwrap();
    let res = call(&mut c, json!({"cmd": "edit", "profile": "custom", "changes": {"gpu.max_mhz": 2000}})).unwrap();
    assert_eq!(res["profile"], json!("custom"), "{res}");
    assert_eq!(res["pending"], json!(false), "{res}");
    assert_eq!(c.active, "performance", "a safe edit of an inactive profile is saved, not applied");
    assert_eq!(c.cfg.profiles["custom"].gpu.max_mhz, Some(2000));
    assert_eq!(r(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/tile0/gt0/freq0/max_freq"), "2400");
    // All or nothing: one bad key in the batch saves none of it.
    let err = call(&mut c, json!({"cmd": "edit", "profile": "custom", "changes": {"gpu.max_mhz": 2100, "gpu.bogus": 1}})).unwrap_err();
    assert!(err.contains("gpu.bogus"), "{err}");
    assert_eq!(c.cfg.profiles["custom"].gpu.max_mhz, Some(2000));
}

#[test]
fn gpu_power_risk_is_relative_to_stock() {
    use omarchy_oc::tune::key_is_risky;
    assert!(!key_is_risky("gpu.power_w", &json!(140.0), Some(150.0)));
    assert!(key_is_risky("gpu.power_w", &json!(160.0), Some(150.0)));
    assert!(key_is_risky("gpu.power_w", &json!("160"), Some(150.0)), "string values count too");
    assert!(key_is_risky("gpu.power_w", &json!(100.0), None), "unknown stock is risky");
    assert!(!key_is_risky("gpu.power_w", &json!("stock"), None), "clearing is never risky");
    assert!(key_is_risky("smu.co_all", &json!("-10"), Some(150.0)));
    assert!(key_is_risky("smu.stapm_w", &json!(90), Some(150.0)));
    assert!(key_is_risky("smu.co_cores.3", &json!(-5), Some(150.0)));
    assert!(!key_is_risky("smu.co_all", &json!(null), Some(150.0)));
    assert!(!key_is_risky("cpu.epp", &json!("power"), Some(150.0)));
    assert!(!key_is_risky("gpu.max_mhz", &json!(2000), Some(150.0)));
}

// ---- stability fingerprint covers only the risky settings ---------------

/// A kept, tested overclock profile; edits then go against it.
fn kept_profile(c: &mut Core) {
    call(c, json!({"cmd": "copy", "from": "max", "to": "my-max"})).unwrap();
    call(c, json!({"cmd": "apply", "profile": "my-max"})).unwrap();
    call(c, json!({"cmd": "confirm"})).unwrap();
    call(c, json!({"cmd": "stress-begin", "kind": "cpu"})).unwrap();
    call(c, json!({"cmd": "stress-end", "kind": "cpu", "passed": true, "errors": 0, "seconds": 60.0})).unwrap();
    let st = c.status(1);
    let p = st["profiles"].as_array().unwrap().iter().find(|p| p["name"] == "my-max").unwrap().clone();
    assert_eq!(p["confirmed"], json!(true), "{p}");
    assert_eq!(p["cpu_test"]["passed"], json!(true), "{p}");
}

fn profile_status(c: &Core, name: &str) -> Value {
    let st = c.status(1);
    st["profiles"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone()
}

#[test]
fn edit_safe_key_keeps_confirmed_and_results() {
    let f = Fake::new("edit-safe-kept");
    let mut c = f.core();
    kept_profile(&mut c);
    // Clocks, EPP, ASPM and fans do not affect stability: no new Keep/Revert.
    let res = call(&mut c, json!({"cmd": "edit", "profile": "my-max", "changes": {"cpu.epp": "power", "gpu.max_mhz": 2000, "platform.aspm": "powersave", "fans.profile": "silent"}})).unwrap();
    assert_eq!(res["pending"], json!(false), "{res}");
    assert_eq!(c.active, "my-max");
    let p = profile_status(&c, "my-max");
    assert_eq!(p["confirmed"], json!(true), "{p}");
    assert_eq!(p["cpu_test"]["passed"], json!(true), "{p}");
    assert_eq!(p["tune"]["cpu"]["epp"], json!("power"));
    assert!(!f.dir.join("state/dirty").exists(), "no revert window reopened");
}

#[test]
fn edit_risky_key_resets_confirmed() {
    let f = Fake::new("edit-risky-reset");
    let mut c = f.core();
    kept_profile(&mut c);
    let res = call(&mut c, json!({"cmd": "edit", "profile": "my-max", "changes": {"smu.co_all": "-15"}, "apply": true})).unwrap();
    assert_eq!(res["pending"], json!(true), "{res}");
    let p = profile_status(&c, "my-max");
    assert_eq!(p["confirmed"], json!(false), "{p}");
    assert_eq!(p["cpu_test"], Value::Null, "old test result no longer applies: {p}");
    call(&mut c, json!({"cmd": "confirm"})).unwrap();
    assert_eq!(profile_status(&c, "my-max")["confirmed"], json!(true));
    // Raising GPU power is risky as well.
    let res = call(&mut c, json!({"cmd": "edit", "profile": "my-max", "changes": {"gpu.power_w": 195}, "apply": true})).unwrap();
    assert_eq!(res["pending"], json!(true), "{res}");
    assert_eq!(profile_status(&c, "my-max")["confirmed"], json!(false));
    call(&mut c, json!({"cmd": "confirm"})).unwrap();
    // The fingerprint covers the whole gpu.power_w field: any change to it,
    // even down to a safe value, is a different overclock and so the kept
    // state no longer applies (the profile is still risky for its SMU limits).
    let res = call(&mut c, json!({"cmd": "edit", "profile": "my-max", "changes": {"gpu.power_w": 140}})).unwrap();
    assert_eq!(res["pending"], json!(true), "{res}");
    assert_eq!(profile_status(&c, "my-max")["confirmed"], json!(false));
}

#[test]
fn processes_lists_unique_names() {
    let f = Fake::new("procs");
    // Two bash, one blender, one kernel thread (empty cmdline); the fake's
    // non-numeric /proc entries (cpuinfo, stat, sys) are skipped too.
    for (pid, comm, cmd) in [("300", "bash", "/usr/bin/bash\0-l\0"), ("200", "blender", "/usr/bin/blender\0"), ("100", "bash", "bash\0")] {
        w(&f.root, &format!("/proc/{pid}/comm"), comm);
        w(&f.root, &format!("/proc/{pid}/cmdline"), cmd);
    }
    w(&f.root, "/proc/2/comm", "kthreadd");
    w(&f.root, "/proc/2/cmdline", "");
    let mut c = f.core();
    let res = call(&mut c, json!({"cmd": "processes"})).unwrap();
    assert_eq!(res, json!([{"name": "bash"}, {"name": "blender"}]), "{res}");
}

#[test]
fn rule_set_changes_profile_in_place() {
    let f = Fake::new("rule-set");
    let mut c = f.core();
    call(&mut c, json!({"cmd": "rule-add", "matcher": "steam-game", "profile": "max"})).unwrap();
    call(&mut c, json!({"cmd": "rule-add", "matcher": "process:blender", "profile": "balanced"})).unwrap();
    let res = call(&mut c, json!({"cmd": "rule-set", "index": 0, "profile": "performance"})).unwrap();
    assert_eq!(
        res["rules"],
        json!([{"matcher": "steam-game", "profile": "performance"}, {"matcher": "process:blender", "profile": "balanced"}]),
        "order kept, only the profile changes: {res}"
    );
    assert!(call(&mut c, json!({"cmd": "rule-set", "index": 2, "profile": "max"})).is_err(), "no such rule");
    assert!(call(&mut c, json!({"cmd": "rule-set", "index": 0, "profile": "nope"})).is_err(), "unknown profile");
    assert!(call(&mut c, json!({"cmd": "rule-set", "profile": "max"})).is_err(), "missing index");
    let st = c.status(1);
    assert_eq!(st["rules"][0]["profile"], json!("performance"));
}

// ---- same-boot daemon crash (C1) ---------------------------------------------

const GPU_PL: &str = "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/hwmon/hwmon2/power2_max";

#[test]
fn same_boot_crash_while_unconfirmed_starts_at_stock() {
    let f = Fake::new("sameboot-crash");
    {
        let mut c = f.core();
        let res = call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
        assert_eq!(res["pending"], json!(true));
        // The daemon dies here (panic, SIGKILL): no shutdown(); systemd
        // restarts it on the same boot.
    }
    assert_eq!(r(&f.root, GPU_PL), "190000000");
    let c = f.core();
    assert_eq!(c.active, "stock", "an unconfirmed overclock is never resumed");
    assert_eq!(r(&f.root, GPU_PL), "150000000", "startup puts stock back itself");
    let st = c.status(1);
    let alert = st["alert"].as_str().unwrap_or("");
    assert!(alert.contains("\"max\""), "alert names the profile: {alert}");
    assert!(!f.dir.join("state/dirty").exists());
    assert_eq!(st["pending"], Value::Null);
}

#[test]
fn same_boot_crash_under_test_starts_at_stock() {
    let f = Fake::new("sameboot-test");
    {
        let mut c = f.core();
        call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
        call(&mut c, json!({"cmd": "confirm"})).unwrap();
        call(&mut c, json!({"cmd": "stress-begin", "kind": "cpu"})).unwrap();
    }
    let c = f.core();
    assert_eq!(c.active, "stock", "a crash under test starts at stock");
    assert!(c.status(1)["alert"].as_str().unwrap_or("").contains("\"max\""));
}

#[test]
fn resume_refuses_unconfirmed_risky_profile() {
    let f = Fake::new("resume-refuse");
    {
        let mut c = f.core();
        call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
    }
    // Even without the dirty marker, run/active alone must not bring back
    // an overclock that was never kept.
    std::fs::remove_file(f.dir.join("state/dirty")).unwrap();
    let c = f.core();
    assert_eq!(c.active, "stock");
    assert_eq!(r(&f.root, GPU_PL), "150000000");
    let st = c.status(1);
    assert!(st["alert"].as_str().unwrap_or("").contains("\"max\""), "{}", st["alert"]);
}

#[test]
fn resume_reapplies_kept_risky_profile() {
    let f = Fake::new("resume-kept");
    {
        let mut c = f.core();
        call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
        call(&mut c, json!({"cmd": "confirm"})).unwrap();
    }
    // restore (ExecStopPost) keeps the resume marker: nothing was dirty.
    omarchy_oc::daemon::restore(omarchy_oc::util::Sys::new(&f.root), f.paths());
    assert!(f.dir.join("run/active").exists());
    let c = f.core();
    assert_eq!(c.active, "max", "a kept profile carries on after a restart");
    assert_eq!(r(&f.root, GPU_PL), "190000000");
    assert_eq!(c.status(1)["alert"], Value::Null);
}

#[test]
fn restore_drops_resume_marker_when_dirty() {
    let f = Fake::new("restore-dirty");
    {
        let mut c = f.core();
        call(&mut c, json!({"cmd": "apply", "profile": "max"})).unwrap();
    }
    assert!(f.dir.join("run/active").exists());
    let out = omarchy_oc::daemon::restore(omarchy_oc::util::Sys::new(&f.root), f.paths());
    assert!(!f.dir.join("run/active").exists(), "{out:?}");
    assert_eq!(r(&f.root, GPU_PL), "150000000");
}

// ---- watchdog fallback is never an overclock (C2) ------------------------------

#[test]
fn watchdog_rejects_risky_fallback() {
    let f = Fake::new("wd-risky");
    let mut c = f.core();
    let err = call(&mut c, json!({"cmd": "watchdog", "fallback": "max"})).unwrap_err();
    assert!(err.contains("overclock"), "{err}");
    assert_eq!(c.cfg.watchdog.fallback, "stock", "nothing saved");
    // Nothing else in a refused request is half-applied either.
    assert!(call(&mut c, json!({"cmd": "watchdog", "fallback": "max", "seconds": 9})).is_err());
    assert_eq!(c.cfg.watchdog.seconds, 5);
    let res = call(&mut c, json!({"cmd": "watchdog", "fallback": "eco"})).unwrap();
    assert_eq!(res["fallback"], json!("eco"));
}

#[test]
fn watchdog_falls_back_to_stock_when_fallback_became_risky() {
    let f = Fake::new("wd-became-risky");
    let mut c = f.core();
    // A safe fallback when chosen...
    call(&mut c, json!({"cmd": "watchdog", "fallback": "custom", "gpu_max_c": 60, "seconds": 1})).unwrap();
    // ...that later gained an overclock and is now the running profile.
    call(&mut c, json!({"cmd": "set", "profile": "custom", "key": "gpu.power_w", "value": "180"})).unwrap();
    call(&mut c, json!({"cmd": "apply", "profile": "custom"})).unwrap();
    call(&mut c, json!({"cmd": "confirm"})).unwrap();
    assert_eq!(r(&f.root, GPU_PL), "180000000");
    w(&f.root, "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0/hwmon/hwmon2/temp2_input", "70000");
    c.tick();
    assert_eq!(c.active, "stock", "the watchdog never falls back to an overclock");
    assert_eq!(r(&f.root, GPU_PL), "150000000");
    assert!(c.status(1)["alert"].as_str().unwrap_or("").contains("\"stock\""));
}

// ---- SMU restores that fail are not forgotten (I1) ----------------------------

use common::{FakeSmu, stock_limits};
use omarchy_oc::smu::{self, cmd};
use omarchy_oc::tune::SmuTune;

#[test]
fn smu_busy_is_retried_once() {
    let fake = FakeSmu::default();
    fake.state().script.insert(cmd::FAST_LIMIT, [0xFC].into());
    let t = SmuTune { fast_w: Some(90), ..Default::default() };
    let rep = smu::apply(&fake, &t, &SmuTune::default(), Some(&stock_limits()));
    assert!(rep.errors.is_empty(), "{:?}", rep.errors);
    assert_eq!(fake.count(cmd::FAST_LIMIT), 2, "busy once, then accepted");
    // Busy twice: an error, and only one retry.
    let fake = FakeSmu::default();
    fake.state().script.insert(cmd::FAST_LIMIT, [0xFC, 0xFC, 0xFC].into());
    let rep = smu::apply(&fake, &t, &SmuTune::default(), Some(&stock_limits()));
    assert_eq!(rep.errors.len(), 1, "{:?}", rep.errors);
    assert_eq!(fake.count(cmd::FAST_LIMIT), 2);
    assert_eq!(rep.applied.fast_w, Some(90), "maybe set: a restore must still try it");
    // Other failures are not retried.
    let fake = FakeSmu::default();
    fake.state().script.insert(cmd::FAST_LIMIT, [0xFF].into());
    smu::apply(&fake, &t, &SmuTune::default(), Some(&stock_limits()));
    assert_eq!(fake.count(cmd::FAST_LIMIT), 1);
}

#[test]
fn smu_failed_restore_stays_applied_and_needs_reboot() {
    let fake = FakeSmu::default();
    fake.state().always.insert(cmd::STAPM_LIMIT, 0xFF);
    fake.state().always.insert(cmd::CO_ALL, 0xFF);
    let prev = SmuTune { stapm_w: Some(50), fast_w: Some(60), co_all: Some(-10), ..Default::default() };
    let rep = smu::apply(&fake, &SmuTune::default(), &prev, Some(&stock_limits()));
    assert_eq!(rep.applied.stapm_w, Some(50), "kept for the next restore");
    assert_eq!(rep.applied.co_all, Some(-10), "kept for the next restore");
    assert_eq!(rep.applied.fast_w, None, "restored");
    assert!(rep.needs_reboot.contains(&"stapm".to_string()), "{:?}", rep.needs_reboot);
    assert!(rep.needs_reboot.contains(&"co_all".to_string()), "{:?}", rep.needs_reboot);
    assert!(rep.done.contains(&"fast".to_string()), "{:?}", rep.done);
    // Next time the SMU takes it: nothing left.
    fake.state().always.clear();
    let rep = smu::apply(&fake, &SmuTune::default(), &rep.applied, Some(&stock_limits()));
    assert!(rep.applied.is_empty(), "{:?}", rep.applied);
    assert!(rep.done.contains(&"stapm".to_string()) && rep.done.contains(&"co_all".to_string()), "{:?}", rep.done);
}

#[test]
fn daemon_keeps_failed_smu_restore_and_alerts() {
    let f = Fake::new("smu-restore-fail");
    let mut c = f.core();
    let fake = FakeSmu::with_table(stock_limits());
    c.set_smu(Box::new(fake.clone()), Some(stock_limits()));
    call(&mut c, json!({"cmd": "set", "profile": "custom", "key": "smu.stapm_w", "value": "50"})).unwrap();
    call(&mut c, json!({"cmd": "apply", "profile": "custom"})).unwrap();
    call(&mut c, json!({"cmd": "confirm"})).unwrap();
    fake.state().always.insert(cmd::STAPM_LIMIT, 0xFF);
    call(&mut c, json!({"cmd": "apply", "profile": "stock"})).unwrap();
    let st = c.status(1);
    assert_eq!(st["smu"]["applied"]["stapm_w"], json!(50), "{}", st["smu"]);
    assert_eq!(st["smu"]["needs_reboot"], json!(["stapm"]), "{}", st["smu"]);
    assert!(st["alert"].as_str().unwrap_or("").contains("stapm"), "{}", st["alert"]);
    let saved: Value = serde_json::from_slice(&std::fs::read(f.dir.join("run/smu-applied.json")).unwrap()).unwrap();
    assert_eq!(saved["stapm_w"], json!(50), "restore can retry it: {saved}");
    // The SMU recovers: the daemon's stop puts it back and forgets it.
    fake.state().always.clear();
    c.shutdown();
    let saved: Value = serde_json::from_slice(&std::fs::read(f.dir.join("run/smu-applied.json")).unwrap()).unwrap();
    assert_eq!(saved["stapm_w"], Value::Null, "{saved}");
    assert_eq!(fake.state().sent.last(), Some(&(cmd::STAPM_LIMIT, 65000)));
}

#[test]
fn restore_smu_deletes_record_only_when_all_restored() {
    let f = Fake::new("smu-restore-file");
    let p = f.paths();
    std::fs::create_dir_all(&p.run_dir).unwrap();
    let applied = SmuTune { stapm_w: Some(50), fast_w: Some(60), ..Default::default() };
    std::fs::write(p.run("smu-applied.json"), serde_json::to_vec(&applied).unwrap()).unwrap();
    std::fs::write(p.run("smu-stock.json"), serde_json::to_vec(&stock_limits()).unwrap()).unwrap();
    let fake = FakeSmu::default();
    fake.state().always.insert(cmd::STAPM_LIMIT, 0xFF);
    let out = omarchy_oc::daemon::restore_smu(&fake, &p);
    let left: SmuTune = serde_json::from_slice(&std::fs::read(p.run("smu-applied.json")).expect("kept")).unwrap();
    assert_eq!(left, SmuTune { stapm_w: Some(50), ..Default::default() }, "{out:?}");
    assert!(out.iter().any(|l| l.contains("stapm")), "{out:?}");
    fake.state().always.clear();
    let out = omarchy_oc::daemon::restore_smu(&fake, &p);
    assert!(!p.run("smu-applied.json").exists(), "{out:?}");
}

// ---- detect is passive: no SMU access at all (I2) ------------------------------

#[test]
fn passive_never_touches_the_smn_window() {
    let f = Fake::new("passive-smn");
    // The fake CPU is a Phoenix-family APU, so a managing daemon would open
    // the mailbox; give it a config space to scribble on.
    let cfgp = f.root.join("sys/bus/pci/devices/0000:00:00.0/config");
    std::fs::create_dir_all(cfgp.parent().unwrap()).unwrap();
    std::fs::write(&cfgp, vec![0u8; 256]).unwrap();
    let mut c = Core::passive(omarchy_oc::util::Sys::new(&f.root), f.paths());
    c.tick();
    assert_eq!(std::fs::read(&cfgp).unwrap(), vec![0u8; 256], "no SMN index/data access");
    assert!(!f.root.join("run/omarchy-oc/smn.lock").exists(), "the mailbox was never opened");
    let st = c.status(1);
    assert_eq!(st["caps"]["smu"], json!(false));
    assert!(st["caps"]["smu_error"].as_str().unwrap_or("").contains("read-only"), "{}", st["caps"]);
    assert!(!f.paths().run("info.json").exists(), "detect leaves the daemon's info.json alone");
}

#[test]
fn passive_without_config_space_reports_smu_unavailable() {
    let f = Fake::new("passive-nocfg");
    let c = Core::passive(omarchy_oc::util::Sys::new(&f.root), f.paths());
    let st = c.status(1);
    assert_eq!(st["caps"]["smu"], json!(false));
    let e = st["caps"]["smu_error"].as_str().unwrap_or("");
    assert!(!e.contains("host bridge"), "the config space was not even opened: {e}");
}
