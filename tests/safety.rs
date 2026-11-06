//! Daemon safety logic against a fake sysfs tree: apply, keep/revert,
//! crash guard, boot-apply refusal, restore to stock.

use omarchy_oc::daemon::{Core, Paths};
use omarchy_oc::util::Sys;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

fn w(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel.trim_start_matches('/'));
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, v).unwrap();
}

fn r(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel.trim_start_matches('/'))).unwrap().trim().to_string()
}

struct Fake {
    dir: PathBuf,
    root: PathBuf,
}

impl Fake {
    fn new(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("oc-safety-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let root = dir.join("root");
        let cpu = "/sys/devices/system/cpu";
        w(&root, "/proc/cpuinfo", "processor\t: 0\nvendor_id\t: AuthenticAMD\ncpu family\t: 25\nmodel\t\t: 120\nmodel name\t: AMD Ryzen 5 8500G\nphysical id\t: 0\ncore id\t\t: 0\n\nprocessor\t: 1\nphysical id\t: 0\ncore id\t\t: 1\n");
        w(&root, "/proc/stat", "cpu  1 0 1 10 0 0 0 0\ncpu0 1 0 1 10 0 0 0 0\ncpu1 1 0 1 10 0 0 0 0\n");
        w(&root, "/proc/sys/kernel/random/boot_id", "boot-1");
        w(&root, &format!("{cpu}/possible"), "0-1");
        w(&root, &format!("{cpu}/amd_pstate/status"), "active");
        w(&root, &format!("{cpu}/smt/control"), "on");
        w(&root, &format!("{cpu}/cpufreq/boost"), "1");
        for c in 0..2 {
            let p = format!("{cpu}/cpu{c}/cpufreq");
            w(&root, &format!("{cpu}/cpu{c}/topology/core_id"), &c.to_string());
            w(&root, &format!("{p}/scaling_governor"), "powersave");
            w(&root, &format!("{p}/scaling_available_governors"), "performance powersave");
            w(&root, &format!("{p}/energy_performance_preference"), "balance_performance");
            w(&root, &format!("{p}/energy_performance_available_preferences"), "default performance balance_performance balance_power power");
            w(&root, &format!("{p}/scaling_min_freq"), "400000");
            w(&root, &format!("{p}/scaling_max_freq"), "5000000");
            w(&root, &format!("{p}/cpuinfo_min_freq"), "400000");
            w(&root, &format!("{p}/cpuinfo_max_freq"), "5000000");
            w(&root, &format!("{p}/scaling_cur_freq"), "3000000");
            for (i, n) in ["POLL", "C1", "C2"].iter().enumerate() {
                w(&root, &format!("{cpu}/cpu{c}/cpuidle/state{i}/name"), n);
                w(&root, &format!("{cpu}/cpu{c}/cpuidle/state{i}/disable"), "0");
            }
        }
        w(&root, "/sys/module/pcie_aspm/parameters/policy", "[default] performance powersave powersupersave");
        // Arc GPU on xe.
        let dev = "/sys/devices/pci0000:00/0000:00:01.1/0000:03:00.0";
        let f = format!("{dev}/tile0/gt0/freq0");
        for (k, v) in [("min_freq", "600"), ("max_freq", "2400"), ("rp0_freq", "2400"), ("rpn_freq", "300"), ("rpe_freq", "600"), ("act_freq", "0"), ("cur_freq", "600")] {
            w(&root, &format!("{f}/{k}"), v);
        }
        w(&root, &format!("{f}/power_profile"), "[base] power_saving");
        let h = format!("{dev}/hwmon/hwmon2");
        w(&root, &format!("{h}/name"), "xe");
        w(&root, &format!("{h}/power2_label"), "pkg");
        w(&root, &format!("{h}/power2_max"), "150000000");
        w(&root, &format!("{h}/power2_max_interval"), "28000");
        w(&root, &format!("{h}/temp2_label"), "pkg");
        w(&root, &format!("{h}/temp2_input"), "50000");
        w(&root, &format!("{dev}/resource"), "0x00000000de000000 0x00000000deffffff 0x0\n0x0 0x0 0x0\n0x000000fa00000000 0x000000fbffffffff 0x0\n");
        fs::create_dir_all(root.join("sys/bus/pci/drivers/xe")).unwrap();
        std::os::unix::fs::symlink(root.join(dev.trim_start_matches('/')), root.join("sys/bus/pci/drivers/xe/0000:03:00.0")).unwrap();
        fs::write(dir.join("kmsg"), "").unwrap();
        Fake { dir, root }
    }

    fn paths(&self) -> Paths {
        Paths { run_dir: self.dir.join("run"), state_dir: self.dir.join("state"), group: "nonexistent-group".into(), kmsg: self.dir.join("kmsg") }
    }

    fn core(&self) -> Core {
        Core::new(Sys::new(&self.root), self.paths())
    }

    /// Simulate a reboot: new boot id, /run wiped, hardware back to stock.
    fn reboot(&self) {
        w(&self.root, "/proc/sys/kernel/random/boot_id", "boot-2");
        let _ = fs::remove_dir_all(self.dir.join("run"));
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

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
