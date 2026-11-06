//! Daemon safety logic against a fake sysfs tree: apply, keep/revert,
//! crash guard, boot-apply refusal, restore to stock.

mod common;

use common::{Fake, r};
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
