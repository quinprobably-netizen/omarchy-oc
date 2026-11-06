//! Prometheus / OpenMetrics text exporter: written to
//! /run/omarchy-oc/metrics.prom every second (node_exporter textfile
//! collector compatible) and optionally served on a loopback HTTP port.

use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', " ")
}

pub fn render(st: &Value) -> String {
    let mut o = String::new();
    let mut g = |name: &str, help: &str, labels: &str, v: Option<f64>| {
        if let Some(v) = v.filter(|v| v.is_finite()) {
            if !o.contains(&format!("# TYPE omarchy_oc_{name} ")) {
                o.push_str(&format!("# HELP omarchy_oc_{name} {help}\n# TYPE omarchy_oc_{name} gauge\n"));
            }
            o.push_str(&format!("omarchy_oc_{name}{labels} {v}\n"));
        }
    };
    let f = |v: &Value| v.as_f64();
    let c = &st["cpu"]["reading"];
    g("cpu_temperature_celsius", "CPU Tctl", "", f(&c["temp"]));
    g("cpu_package_watts", "CPU package power (RAPL)", "", f(&c["package_w"]));
    g("cpu_core_watts", "CPU core power (RAPL)", "", f(&c["core_w"]));
    g("cpu_utilization_percent", "CPU utilisation", "", f(&c["util"]));
    g("cpu_frequency_mhz_avg", "Average CPU frequency", "", f(&c["mhz_avg"]));
    g("cpu_frequency_mhz_max", "Highest CPU frequency", "", f(&c["mhz_max"]));
    if let Some(ts) = c["threads"].as_array() {
        for t in ts {
            let l = format!("{{cpu=\"{}\"}}", t["cpu"]);
            g("cpu_thread_frequency_mhz", "Per-thread frequency", &l, f(&t["mhz"]));
            g("cpu_thread_utilization_percent", "Per-thread utilisation", &l, f(&t["util"]));
        }
    }
    let s = &st["smu"]["limits"];
    for (k, help) in [
        ("stapm", "STAPM"),
        ("fast", "PPT fast"),
        ("slow", "PPT slow"),
        ("tdc", "TDC"),
        ("edc", "EDC"),
        ("soc_tdc", "SoC TDC"),
        ("soc_edc", "SoC EDC"),
        ("tctl", "Tctl"),
    ] {
        let l = format!("{{limit=\"{k}\"}}");
        g("smu_limit", &format!("SMU limit ({help} etc.)"), &l, f(&s[format!("{k}_limit")]));
        g("smu_value", "SMU current value against limit", &l, f(&s[format!("{k}_value")]));
    }
    let gr = &st["gpu"]["reading"];
    g("gpu_frequency_mhz", "GPU actual frequency", "", f(&gr["act_mhz"]));
    g("gpu_temperature_celsius", "GPU package temperature", "", f(&gr["temp"]));
    g("gpu_vram_temperature_celsius", "GPU VRAM temperature", "", f(&gr["vram_temp"]));
    g("gpu_power_watts", "GPU package power", "", f(&gr["power_w"]));
    g("gpu_busy_percent", "GPU busy (from GT idle residency)", "", f(&gr["busy"]));
    g("gpu_vram_used_mib", "VRAM used by DRM clients", "", f(&gr["vram_used_mib"]));
    g("gpu_throttled", "GPU throttle status", "", Some(if gr["throttled"] == Value::Bool(true) { 1.0 } else { 0.0 }));
    if let Some(fans) = gr["fans"].as_array() {
        for fan in fans {
            g("gpu_fan_rpm", "GPU fan speed", &format!("{{fan=\"{}\"}}", fan["fan"]), f(&fan["rpm"]));
        }
    }
    let gk = &st["gpu"]["knobs"];
    g("gpu_power_limit_watts", "GPU PL1", "", f(&gk["power_w"]));
    g("gpu_max_frequency_mhz", "GPU max frequency limit", "", f(&gk["max_mhz"]));
    let m = &st["mem"]["reading"];
    g("memory_used_percent", "RAM used", "", f(&m["used_pct"]));
    if let Some(ds) = m["dimm_temps"].as_array() {
        for d in ds {
            g("dimm_temperature_celsius", "DIMM temperature", &format!("{{dimm=\"{}\"}}", esc(d["dimm"].as_str().unwrap_or("?"))), f(&d["temp"]));
        }
    }
    if let Some(ts) = st["platform"]["reading"]["temps"].as_array() {
        for t in ts {
            let l = format!("{{chip=\"{}\",sensor=\"{}\"}}", esc(t["chip"].as_str().unwrap_or("")), esc(t["sensor"].as_str().unwrap_or("")));
            g("board_temperature_celsius", "Board/peripheral temperature", &l, f(&t["temp"]));
        }
    }
    if let Some(vs) = st["platform"]["reading"]["volts"].as_array() {
        for t in vs {
            let l = format!("{{chip=\"{}\",sensor=\"{}\"}}", esc(t["chip"].as_str().unwrap_or("")), esc(t["sensor"].as_str().unwrap_or("")));
            g("board_volts", "Board voltage rail (raw sensor reading)", &l, f(&t["volts"]));
        }
    }
    if let Some(fans) = st["fans"]["fans"].as_array() {
        for fan in fans {
            let l = format!("{{fan=\"{}\"}}", esc(fan["label"].as_str().unwrap_or("")));
            g("fan_rpm", "Fan speed (omarchy-fans)", &l, f(&fan["rpm"]));
        }
    }
    g("mce_total", "Machine check errors seen since daemon start", "", f(&st["health"]["mce"]));
    g("gpu_hangs_total", "GPU hangs/resets seen since daemon start", "", f(&st["health"]["gpu_hangs"]));
    let prof = st["active"].as_str().unwrap_or("");
    g("profile_info", "Active profile", &format!("{{profile=\"{}\"}}", esc(prof)), Some(1.0));
    o
}

/// Serve the latest rendering on `addr` (loopback only).
pub fn serve(addr: &str, latest: Arc<Mutex<String>>) -> std::io::Result<()> {
    let l = TcpListener::bind(addr)?;
    if !l.local_addr()?.ip().is_loopback() {
        return Err(std::io::Error::other("metrics listener must be on a loopback address"));
    }
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let latest = latest.clone();
            std::thread::spawn(move || {
                let mut s = s;
                let _ = s.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                let mut buf = [0u8; 1024];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let ok = req.starts_with("GET /metrics") || req.starts_with("GET / ");
                let body = if ok { latest.lock().map(|b| b.clone()).unwrap_or_default() } else { "not found\n".into() };
                let status = if ok { "200 OK" } else { "404 Not Found" };
                let _ = write!(
                    s,
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
        }
    });
    Ok(())
}
