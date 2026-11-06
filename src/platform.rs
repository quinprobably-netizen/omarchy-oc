//! Platform: PCIe ASPM policy, board voltages and every temperature the
//! kernel exposes (motherboard, NVMe, chipset, NIC, Wi-Fi).

use crate::tune::{PlatformTune, Write};
use crate::util::{Sys, read_num, read_trim};
use serde_json::{Value, json};

const ASPM: &str = "/sys/module/pcie_aspm/parameters/policy";

pub struct Platform {
    sys: Sys,
    pub reading: Value,
    pub ident: Value,
}

/// Friendly names for hwmon chips.
fn chip_label(name: &str) -> String {
    match name {
        n if n.starts_with("it86") || n.starts_with("it87") || n.starts_with("nct") => "Motherboard".into(),
        "gigabyte_wmi" => "Motherboard (WMI)".into(),
        "nvme" => "NVMe".into(),
        "acpitz" => "ACPI zone".into(),
        n if n.contains("xhci") => "USB controller".into(),
        n if n.starts_with("r8169") || n.starts_with("igc") || n.starts_with("atlantic") => "Ethernet".into(),
        n if n.starts_with("iwlwifi") || n.starts_with("mt7") => "Wi-Fi".into(),
        n => n.into(),
    }
}

impl Platform {
    pub fn detect(sys: &Sys) -> Platform {
        let dmi = |f: &str| sys.read(format!("/sys/class/dmi/id/{f}"));
        let ident = json!({
            "board_vendor": dmi("board_vendor"),
            "board": dmi("board_name"),
            "bios_vendor": dmi("bios_vendor"),
            "bios_version": dmi("bios_version"),
            "bios_date": dmi("bios_date"),
            "kernel": sys.read("/proc/sys/kernel/osrelease"),
            "cmdline": sys.read("/proc/cmdline"),
            "lockdown": sys.read("/sys/kernel/security/lockdown"),
        });
        Platform { sys: sys.clone(), reading: Value::Null, ident }
    }

    pub fn sample(&mut self) {
        let mut temps = Vec::new();
        let mut volts = Vec::new();
        for h in self.sys.list("/sys/class/hwmon") {
            let Some(name) = read_trim(&h.join("name")) else { continue };
            // CPU and GPU temps are shown in their own sections.
            if matches!(name.as_str(), "k10temp" | "zenpower" | "coretemp" | "xe" | "amdgpu" | "spd5118" | "jc42") {
                continue;
            }
            let chip = chip_label(&name);
            for i in 1..=16 {
                let Some(v) = read_num::<f64>(&h.join(format!("temp{i}_input"))) else { continue };
                let label = read_trim(&h.join(format!("temp{i}_label"))).unwrap_or_else(|| format!("temp{i}"));
                // it87 reports -128 / 127 for unconnected thermistors.
                if !(-40_000.0..=125_000.0).contains(&v) {
                    continue;
                }
                temps.push(json!({"chip": chip, "sensor": label, "temp": v / 1000.0, "id": format!("{name}/temp{i}")}));
            }
            for i in 0..=16 {
                let Some(v) = read_num::<f64>(&h.join(format!("in{i}_input"))) else { continue };
                let label = read_trim(&h.join(format!("in{i}_label"))).unwrap_or_else(|| format!("in{i}"));
                volts.push(json!({"chip": chip, "sensor": label, "volts": v / 1000.0, "id": format!("{name}/in{i}")}));
            }
        }
        self.reading = json!({"temps": temps, "volts": volts});
    }

    pub fn knobs(&self) -> Value {
        let raw = self.sys.read(ASPM).unwrap_or_default();
        let active = raw.split_whitespace().find(|w| w.starts_with('[')).map(|w| w.trim_matches(['[', ']']).to_string());
        let choices: Vec<String> = raw.split_whitespace().map(|w| w.trim_matches(['[', ']']).to_string()).collect();
        json!({"aspm": active, "aspm_policies": choices})
    }

    pub fn plan(&self, t: &PlatformTune) -> Vec<Write> {
        if self.sys.exists(ASPM) { vec![Write::new(ASPM.into(), t.aspm.clone())] } else { vec![] }
    }

    pub fn knob_paths(&self) -> Vec<String> {
        self.plan(&PlatformTune::default()).into_iter().map(|w| w.path).collect()
    }
}

/// For "[default] performance" style files: the bracketed word.
pub fn bracketed(raw: &str) -> Option<String> {
    raw.split_whitespace().find(|w| w.starts_with('[')).map(|w| w.trim_matches(['[', ']']).to_string())
}
