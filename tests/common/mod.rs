//! Fake sysfs tree shared by the integration tests.
#![allow(dead_code)]

use omarchy_oc::daemon::{Core, Paths};
use omarchy_oc::util::Sys;
use std::fs;
use std::path::{Path, PathBuf};

pub fn w(root: &Path, rel: &str, v: &str) {
    let p = root.join(rel.trim_start_matches('/'));
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, v).unwrap();
}

pub fn r(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel.trim_start_matches('/'))).unwrap().trim().to_string()
}

pub struct Fake {
    pub dir: PathBuf,
    pub root: PathBuf,
}

impl Fake {
    pub fn new(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("oc-test-{name}-{}", std::process::id()));
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

    pub fn paths(&self) -> Paths {
        Paths {
            run_dir: self.dir.join("run"),
            state_dir: self.dir.join("state"),
            group: "nonexistent-group".into(),
            kmsg: self.dir.join("kmsg"),
            bin_dir: self.dir.join("bin"),
        }
    }

    /// Install a fake test child (`omarchy-oc` or `omarchy-oc-gpustress`)
    /// in the bin dir the daemon spawns from: a shell script that gets
    /// `$OUT` pointing at the fake's directory.
    pub fn tester(&self, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let bin = self.dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let p = bin.join(name);
        fs::write(&p, format!("#!/bin/sh\nOUT={}\n{body}\n", self.dir.display())).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    pub fn core(&self) -> Core {
        Core::new(Sys::new(&self.root), self.paths())
    }

    /// Simulate a reboot: new boot id, /run wiped, hardware back to stock.
    pub fn reboot(&self) {
        w(&self.root, "/proc/sys/kernel/random/boot_id", "boot-2");
        let _ = fs::remove_dir_all(self.dir.join("run"));
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

