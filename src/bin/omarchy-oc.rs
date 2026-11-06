//! omarchy-oc: control the omarchy-ocd overclocking daemon, and run
//! stability tests.

use omarchy_oc::stress;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const HELP: &str = "omarchy-oc: overclocking and tuning (CPU, SMU, GPU, memory, platform, fans)

  omarchy-oc                          one-line summary
  omarchy-oc status [--json]          live readings and current settings
  omarchy-oc info [--json]            hardware details (CPU, GPU, DIMMs, board, firmware)
  omarchy-oc watch                    live monitor (Ctrl-C to quit)

profiles
  omarchy-oc profiles                 list profiles (stock eco balanced performance max + yours)
  omarchy-oc apply <profile> [--keep] apply; risky settings revert unless confirmed
  omarchy-oc confirm | revert         keep or drop pending settings
  omarchy-oc show <profile>           print a profile's settings
  omarchy-oc set <profile> <key> <value|stock>
                                      e.g. set custom smu.co_all -15
                                           set custom smu.co_cores.2 -20
                                           set custom gpu.power_w 190
                                           set custom cpu.max_mhz 4800
  omarchy-oc keys                     every settable key with its current range
  omarchy-oc copy <from> <to> | rename <from> <to> | delete <profile>
  omarchy-oc edit <profile>           edit as JSON in $EDITOR
  omarchy-oc export <profile> [file]  | import <file> [name]

automation
  omarchy-oc boot on|off              apply the chosen profile at boot (tested profiles only)
  omarchy-oc auto on|off              switch profiles by running programs
  omarchy-oc rule add <matcher> <profile>   matcher: process:<glob> | steam-game | gamemode
  omarchy-oc rule del <n> | rules
  omarchy-oc watchdog [--on|--off] [--cpu-max C] [--gpu-max C] [--seconds N]
                      [--fallback P] [--errors on|off]
  omarchy-oc confirm-time <seconds>
  omarchy-oc metrics [--http on|off] [--listen 127.0.0.1:9877] | metrics print

stability
  omarchy-oc stress cpu [--seconds N] [--per-core] [--core-seconds N] [--json]
  omarchy-oc stress gpu [--seconds N] [--mode compute|vram|mixed] [--json]
  omarchy-oc stress all [--seconds N] [--json]
  omarchy-oc csv [file] [--seconds N]  export the metric history (1 h kept)
  omarchy-oc probe                    SMU / PM table diagnostics";

fn run_dir() -> PathBuf {
    PathBuf::from(std::env::var("OMARCHY_OC_RUN").unwrap_or_else(|_| "/run/omarchy-oc".into()))
}

fn die(msg: &str) -> ! {
    eprintln!("omarchy-oc: {msg}");
    std::process::exit(1)
}

fn try_call(req: Value) -> Result<Value, String> {
    let sock = run_dir().join("control.sock");
    let mut s = UnixStream::connect(&sock).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => "daemon not running (systemctl status omarchy-ocd)".to_string(),
        std::io::ErrorKind::PermissionDenied => "permission denied on the control socket (are you in the wheel group?)".to_string(),
        _ => format!("cannot reach daemon: {e}"),
    })?;
    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
    s.write_all(format!("{req}\n").as_bytes()).map_err(|e| format!("send failed: {e}"))?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).map_err(|e| format!("no reply: {e}"))?;
    let reply: Value = serde_json::from_str(&line).map_err(|_| "garbled reply from daemon".to_string())?;
    if reply["ok"] != json!(true) {
        return Err(reply["error"].as_str().unwrap_or("request failed").to_string());
    }
    Ok(reply["result"].clone())
}

fn call(req: Value) -> Value {
    try_call(req).unwrap_or_else(|e| die(&e))
}

fn read_status() -> Value {
    std::fs::read(run_dir().join("status.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| call(json!({"cmd": "status"})))
}

fn f(v: &Value, d: usize) -> String {
    v.as_f64().map(|x| format!("{x:.d$}")).unwrap_or_else(|| "—".into())
}

fn summary(st: &Value) -> String {
    let c = &st["cpu"]["reading"];
    let g = &st["gpu"]["reading"];
    let pend = if st["pending"].is_null() { String::new() } else { format!(" (unconfirmed, {}s)", st["pending"]["remaining"]) };
    format!(
        "{}{pend} | CPU {} MHz {}°C {} W | GPU {} MHz {}°C {} W",
        st["active"].as_str().unwrap_or("?"),
        f(&c["mhz_max"], 0),
        f(&c["temp"], 0),
        f(&c["package_w"], 0),
        f(&g["act_mhz"], 0),
        f(&g["temp"], 0),
        f(&g["power_w"], 0)
    )
}

fn show_status(st: &Value) {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    println!("profile {}   base {}   daemon {}", s(&st["active"]), s(&st["base"]), s(&st["version"]));
    if !st["pending"].is_null() {
        println!("  ! unconfirmed: reverts to \"{}\" in {} s (omarchy-oc confirm)", s(&st["pending"]["previous"]), st["pending"]["remaining"]);
    }
    if let Some(a) = st["alert"].as_str() {
        println!("  ! {a}");
    }
    if !s(&st["message"]).is_empty() {
        println!("  {}", s(&st["message"]));
    }
    let c = &st["cpu"];
    let r = &c["reading"];
    let k = &c["knobs"];
    println!("\ncpu  {}", s(&c["model"]));
    println!(
        "  {} °C   {} W package   {} W cores   util {} %   avg {} / max {} MHz",
        f(&r["temp"], 1),
        f(&r["package_w"], 1),
        f(&r["core_w"], 1),
        f(&r["util"], 0),
        f(&r["mhz_avg"], 0),
        f(&r["mhz_max"], 0)
    );
    println!(
        "  pstate {}  governor {}  epp {}  boost {}  smt {}  freq {}–{} MHz (hw {}–{})",
        s(&k["pstate_mode"]),
        s(&k["governor"]),
        s(&k["epp"]),
        k["boost"],
        s(&k["smt"]),
        f(&k["min_mhz"], 0),
        f(&k["max_mhz"], 0),
        f(&k["hw_min_mhz"], 0),
        f(&k["hw_max_mhz"], 0)
    );
    let l = &st["smu"]["limits"];
    if !l.is_null() {
        println!(
            "  smu  STAPM {}/{} W  fast {}/{} W  slow {}/{} W  TDC {}/{} A  EDC {}/{} A  Tctl {}/{} °C",
            f(&l["stapm_value"], 1),
            f(&l["stapm_limit"], 0),
            f(&l["fast_value"], 1),
            f(&l["fast_limit"], 0),
            f(&l["slow_value"], 1),
            f(&l["slow_limit"], 0),
            f(&l["tdc_value"], 1),
            f(&l["tdc_limit"], 0),
            f(&l["edc_value"], 1),
            f(&l["edc_limit"], 0),
            f(&l["tctl_value"], 1),
            f(&l["tctl_limit"], 0)
        );
    } else if st["caps"]["smu"] == json!(true) {
        println!("  smu  available (PM table unreadable: {})", s(&st["caps"]["pm_error"]));
    } else {
        println!("  smu  unavailable: {}", s(&st["caps"]["smu_error"]));
    }
    if let Some(nr) = st["smu"]["needs_reboot"].as_array().filter(|a| !a.is_empty()) {
        println!("  smu  reboot to restore stock: {}", nr.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "));
    }
    let g = &st["gpu"];
    if !g.is_null() {
        let r = &g["reading"];
        let k = &g["knobs"];
        println!("\ngpu  {}", s(&g["name"]));
        println!(
            "  {} MHz (req {})  {} °C  vram {} °C  {} W  busy {} %  vram used {} MiB  {} mV",
            f(&r["act_mhz"], 0),
            f(&r["cur_mhz"], 0),
            f(&r["temp"], 0),
            f(&r["vram_temp"], 0),
            f(&r["power_w"], 1),
            f(&r["busy"], 0),
            f(&r["vram_used_mib"], 0),
            f(&r["voltage_mv"], 0)
        );
        println!(
            "  freq {}–{} MHz (hw {}–{})  PL1 {} W / {} ms  profile {}",
            f(&k["min_mhz"], 0),
            f(&k["max_mhz"], 0),
            f(&k["rpn_mhz"], 0),
            f(&k["rp0_mhz"], 0),
            f(&k["power_w"], 0),
            f(&k["power_window_ms"], 0),
            s(&k["power_profile"])
        );
        if r["throttled"] == json!(true) {
            println!("  throttled: {}", r["throttle_reasons"]);
        }
        if let Some(ps) = g["procs"].as_array().filter(|a| !a.is_empty()) {
            let top: Vec<String> = ps.iter().take(5).map(|p| format!("{} {}% {} MiB", s(&p["name"]), f(&p["busy"], 0), f(&p["vram_mib"], 0))).collect();
            println!("  clients: {}", top.join(" · "));
        }
    }
    let m = &st["mem"]["reading"];
    println!("\nmemory  {} %", f(&m["used_pct"], 0));
    if let Some(ds) = m["dimm_temps"].as_array() {
        for d in ds {
            println!("  {} {} °C", s(&d["dimm"]), f(&d["temp"], 1));
        }
    }
    let pk = &st["platform"]["knobs"];
    println!("\nplatform  aspm {}", s(&pk["aspm"]));
    if let Some(ts) = st["platform"]["reading"]["temps"].as_array() {
        let line: Vec<String> = ts.iter().map(|t| format!("{} {} {}°", s(&t["chip"]), s(&t["sensor"]), f(&t["temp"], 0))).collect();
        println!("  {}", line.join(" · "));
    }
    let fans = &st["fans"];
    if fans["available"] == json!(true) {
        let line: Vec<String> = fans["fans"].as_array().map(|a| a.iter().map(|x| format!("{} {}", s(&x["label"]), x["rpm"])).collect()).unwrap_or_default();
        println!("\nfans  profile {}  {}", s(&fans["profile"]), line.join(" · "));
    }
    println!("\nhealth  machine checks {}  gpu hangs {}", st["health"]["mce"], st["health"]["gpu_hangs"]);
}

fn profiles(st: &Value) {
    for p in st["profiles"].as_array().cloned().unwrap_or_default() {
        let name = p["name"].as_str().unwrap_or("");
        let mark = if st["active"] == p["name"] { "*" } else { " " };
        let mut tags = Vec::new();
        if p["builtin"] == json!(true) {
            tags.push("built-in".to_string());
        }
        if p["risky"] == json!(true) {
            tags.push(if p["confirmed"] == json!(true) { "overclock, kept".into() } else { "overclock, untested".into() });
        }
        for (k, label) in [("cpu_test", "cpu test"), ("gpu_test", "gpu test")] {
            if !p[k].is_null() {
                tags.push(format!("{label} {}", if p[k]["passed"] == json!(true) { "passed" } else { "FAILED" }));
            }
        }
        if p["crashes"].as_u64().unwrap_or(0) > 0 {
            tags.push(format!("{} crash(es)", p["crashes"]));
        }
        println!("{mark} {name:<14} {}", tags.join(", "));
    }
}

fn keys() {
    println!(
        "cpu.pstate_mode     active | passive | guided
cpu.governor        performance | powersave (passive: schedutil, ondemand, …)
cpu.epp             performance | balance_performance | balance_power | power
cpu.boost           true | false
cpu.min_mhz         MHz
cpu.max_mhz         MHz (all cores)
cpu.max_mhz_dense   MHz (compact Zen 4c cores only)
cpu.smt             true | false
cpu.idle_max        deepest C-state allowed: POLL | C1 | C2 | C3
smu.stapm_w         sustained power limit, W (10–200)
smu.fast_w          PPT fast limit, W
smu.slow_w          PPT slow limit, W
smu.tctl_c          temperature limit, °C (60–100)
smu.tdc_a / smu.edc_a           core current limits, A
smu.soc_tdc_a / smu.soc_edc_a   SoC current limits, A
smu.co_all          Curve Optimizer all cores (-50…30; negative = undervolt)
smu.co_cores.<n>    Curve Optimizer for physical core n
gpu.min_mhz / gpu.max_mhz   frequency range (within hardware RPn–RP0)
gpu.power_w         PL1 sustained power limit, W
gpu.power_window_ms PL1 averaging window
gpu.power_profile   base | power_saving
platform.aspm       default | performance | powersave | powersupersave
fans.profile        omarchy-fans profile: silent | balanced | performance | full | custom
(value \"stock\" clears a key)"
    );
}

fn on_off(v: Option<&String>) -> bool {
    match v.map(String::as_str) {
        Some("on" | "true" | "yes" | "1") => true,
        Some("off" | "false" | "no" | "0") => false,
        _ => die("expected on or off"),
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn on_sigint(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

/// Started by the daemon (`OMARCHY_OC_CHILD=1`): it tracks and records the
/// run itself, so the tester must not call stress-begin / stress-end.
fn daemon_child() -> bool {
    std::env::var("OMARCHY_OC_CHILD").is_ok_and(|v| v == "1")
}

/// Tell the daemon a test starts, unless the daemon started us.
fn begin_tracking(kind: &str, json_out: bool) -> Result<Value, String> {
    if daemon_child() {
        return Err("run by the daemon".into());
    }
    // The pid lets the daemon drop the run if this process dies unseen.
    let tracked = try_call(json!({"cmd": "stress-begin", "kind": kind, "pid": std::process::id()}));
    if let Err(e) = &tracked
        && !json_out
    {
        eprintln!("note: result not recorded ({e})");
    }
    tracked
}

fn stress_cpu(args: &[String], json_out: bool) -> (bool, u64) {
    let seconds: f64 = flag(args, "--seconds").and_then(|s| s.parse().ok()).unwrap_or(120.0);
    let per_core = args.iter().any(|a| a == "--per-core");
    let core_seconds: f64 = flag(args, "--core-seconds").and_then(|s| s.parse().ok()).unwrap_or(30.0);
    let sys = omarchy_oc::util::Sys::from_env();
    let cpu = omarchy_oc::cpu::Cpu::detect(&sys);
    let mut cores: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
    for t in &cpu.threads {
        if sys.exists(format!("/sys/devices/system/cpu/cpu{}/online", t.cpu)) && sys.read(format!("/sys/devices/system/cpu/cpu{}/online", t.cpu)).as_deref() == Some("0") {
            continue;
        }
        cores.entry(t.core).or_default().push(t.cpu);
    }
    let plan = stress::Plan {
        cores: cores.into_iter().collect(),
        seconds,
        per_core,
        per_core_seconds: core_seconds,
        inject_fault: args.iter().any(|a| a == "--inject-fault"),
    };
    let tracked = begin_tracking("cpu", json_out);
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    std::thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        s2.store(true, Ordering::Relaxed);
    });
    let res = stress::run(&plan, stop, |p| {
        if json_out {
            let mut v = serde_json::to_value(p).unwrap_or_default();
            v["kind"] = json!("cpu");
            println!("{v}");
        } else if p.done {
            println!(
                "\ncpu: {} — {} blocks, {} errors in {:.0} s ({:?}){}",
                if p.passed == Some(true) { "PASSED" } else { "FAILED" },
                p.blocks,
                p.errors,
                p.t,
                p.isa,
                p.message.as_ref().map(|m| format!("\n  {m}")).unwrap_or_default()
            );
            for c in &p.cores {
                if per_core || c.errors > 0 {
                    println!("  core {:>2} (cpu {:?}): {} blocks, {} errors", c.core, c.cpus, c.blocks, c.errors);
                }
            }
        } else {
            print!("\r{:<10} {:>5.0} s  {:>8} blocks  {:>6.0}/s  errors {}   ", p.phase, p.t, p.blocks, p.blocks_per_s, p.errors);
            let _ = std::io::stdout().flush();
        }
    });
    let passed = res.passed == Some(true);
    if tracked.is_ok() {
        let bad: Vec<String> = res.cores.iter().filter(|c| c.errors > 0).map(|c| c.core.to_string()).collect();
        let detail = if bad.is_empty() { format!("{:?}, {} blocks", res.isa, res.blocks) } else { format!("errors on core {}", bad.join(", ")) };
        let _ = try_call(json!({"cmd": "stress-end", "kind": "cpu", "passed": passed, "errors": res.errors, "seconds": res.t,
                                "detail": detail, "completed": res.message.as_deref() != Some("stopped before the end")}));
    }
    (passed, res.errors)
}

fn gpustress_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let side = exe.with_file_name("omarchy-oc-gpustress");
        if side.exists() {
            return side;
        }
    }
    PathBuf::from("omarchy-oc-gpustress")
}

fn stress_gpu(args: &[String], json_out: bool) -> (bool, u64) {
    let seconds = flag(args, "--seconds").unwrap_or_else(|| "120".into());
    let mut cmd = std::process::Command::new(gpustress_bin());
    cmd.arg("--seconds").arg(&seconds).arg("--json").stdout(std::process::Stdio::piped());
    if let Some(m) = flag(args, "--mode") {
        cmd.arg("--mode").arg(m);
    }
    if args.iter().any(|a| a == "--inject-fault") {
        cmd.arg("--inject-fault");
    }
    let tracked = begin_tracking("gpu", json_out);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            if tracked.is_ok() {
                let _ = try_call(json!({"cmd": "stress-end", "kind": "gpu", "passed": false, "errors": 0, "completed": false, "detail": format!("could not start: {e}")}));
            }
            die(&format!("omarchy-oc-gpustress: {e}"))
        }
    };
    let pid = child.id() as i32;
    std::thread::spawn(move || {
        while !STOP.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
        }
        unsafe { libc::kill(pid, libc::SIGINT) };
    });
    let mut last = Value::Null;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let Ok(mut v) = serde_json::from_str::<Value>(&line) else { continue };
            v["kind"] = json!("gpu");
            if json_out {
                println!("{v}");
            } else if v["done"] == json!(true) {
                println!(
                    "\ngpu: {} — {} iterations, {} errors{}",
                    if v["passed"] == json!(true) { "PASSED" } else { "FAILED" },
                    v["iterations"],
                    v["errors"],
                    v["message"].as_str().map(|m| format!("\n  {m}")).unwrap_or_default()
                );
            } else {
                print!(
                    "\r{:<8} {:>5.0} s  {:>8} iter  {} GFLOPS  {} GB/s  errors {}   ",
                    v["mode"].as_str().unwrap_or(""),
                    v["t"].as_f64().unwrap_or(0.0),
                    v["iterations"],
                    f(&v["gflops"], 0),
                    f(&v["vram_gbps"], 0),
                    v["errors"]
                );
                let _ = std::io::stdout().flush();
            }
            last = v;
        }
    }
    let status = child.wait().ok().and_then(|s| s.code()).unwrap_or(2);
    let passed = status == 0 && last["passed"] == json!(true);
    let errors = last["errors"].as_u64().unwrap_or(0);
    if tracked.is_ok() {
        let detail = last["message"].as_str().map(String::from).unwrap_or_else(|| {
            format!("{} GFLOPS, {} GB/s", f(&last["gflops"], 0), f(&last["vram_gbps"], 0))
        });
        let _ = try_call(json!({"cmd": "stress-end", "kind": "gpu", "passed": passed, "errors": errors,
                                "seconds": last["t"].as_f64(), "detail": detail, "completed": !STOP.load(Ordering::Relaxed) && status != 2}));
    }
    (passed, errors)
}

fn watch() {
    loop {
        let st = read_status();
        print!("\x1b[2J\x1b[H");
        show_status(&st);
        let _ = std::io::stdout().flush();
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_default();
    let rest = &args[args.len().min(1)..];
    let json_out = args.iter().any(|a| a == "--json");
    match cmd.as_str() {
        "" => println!("{}", summary(&read_status())),
        "-h" | "--help" | "help" => println!("{HELP}"),
        "status" => {
            let st = read_status();
            if json_out { println!("{}", serde_json::to_string_pretty(&st).unwrap_or_default()) } else { show_status(&st) }
        }
        "info" => {
            let info: Value = std::fs::read(run_dir().join("info.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_else(|| call(json!({"cmd": "info"})));
            println!("{}", serde_json::to_string_pretty(&info).unwrap_or_default());
        }
        "watch" => watch(),
        "profiles" => profiles(&read_status()),
        "keys" => keys(),
        "apply" => {
            let p = rest.first().unwrap_or_else(|| die("apply <profile>"));
            let r = call(json!({"cmd": "apply", "profile": p}));
            for c in r["report"]["changed"].as_array().cloned().unwrap_or_default() {
                println!("  {}", c.as_str().unwrap_or(""));
            }
            for e in r["report"]["errors"].as_array().cloned().unwrap_or_default() {
                eprintln!("  error: {}", e.as_str().unwrap_or(""));
            }
            for n in r["report"]["notes"].as_array().cloned().unwrap_or_default() {
                println!("  note: {}", n.as_str().unwrap_or(""));
            }
            if r["pending"] == json!(true) {
                if rest.iter().any(|a| a == "--keep") {
                    call(json!({"cmd": "confirm"}));
                    println!("Applied and kept \"{p}\".");
                } else {
                    let st = read_status();
                    println!("Applied \"{p}\". These are overclock settings: run `omarchy-oc confirm` within {} s to keep them, or they revert.", st["confirm_seconds"]);
                }
            } else {
                println!("Applied \"{p}\".");
            }
        }
        "confirm" => {
            call(json!({"cmd": "confirm"}));
            println!("Kept.");
        }
        "revert" => {
            call(json!({"cmd": "revert"}));
            println!("Reverted.");
        }
        "show" => {
            let p = rest.first().unwrap_or_else(|| die("show <profile>"));
            let r = call(json!({"cmd": "export", "profile": p}));
            println!("{}", serde_json::to_string_pretty(&r["tune"]).unwrap_or_default());
        }
        "set" => {
            let (Some(p), Some(k)) = (rest.first(), rest.get(1)) else { die("set <profile> <key> <value>") };
            let v = rest.get(2).cloned().unwrap_or_default();
            let r = call(json!({"cmd": "set", "profile": p, "key": k, "value": v}));
            if r["pending"] == json!(true) {
                println!("Set and re-applied; confirm within the revert window: omarchy-oc confirm");
            }
        }
        "copy" | "rename" => {
            let (Some(a), Some(b)) = (rest.first(), rest.get(1)) else { die(&format!("{cmd} <from> <to>")) };
            call(json!({"cmd": cmd, "from": a, "to": b}));
        }
        "delete" => {
            let p = rest.first().unwrap_or_else(|| die("delete <profile>"));
            call(json!({"cmd": "delete", "profile": p}));
        }
        "export" => {
            let p = rest.first().unwrap_or_else(|| die("export <profile> [file]"));
            let r = serde_json::to_string_pretty(&call(json!({"cmd": "export", "profile": p}))).unwrap_or_default();
            match rest.get(1) {
                Some(file) => std::fs::write(file, r + "\n").unwrap_or_else(|e| die(&e.to_string())),
                None => println!("{r}"),
            }
        }
        "import" => {
            let file = rest.first().unwrap_or_else(|| die("import <file> [name]"));
            let v: Value = serde_json::from_slice(&std::fs::read(file).unwrap_or_else(|e| die(&e.to_string()))).unwrap_or_else(|_| die("not JSON"));
            let name = rest.get(1).cloned().or_else(|| v["name"].as_str().map(String::from)).unwrap_or_else(|| die("name it: import <file> <name>"));
            let tune = if v.get("tune").is_some() { v["tune"].clone() } else { v };
            call(json!({"cmd": "save", "profile": name, "tune": tune}));
            println!("Imported \"{name}\".");
        }
        "edit" => {
            let p = rest.first().unwrap_or_else(|| die("edit <profile>"));
            let r = call(json!({"cmd": "export", "profile": p}));
            let tmp = std::env::temp_dir().join(format!("omarchy-oc-{p}-{}.json", std::process::id()));
            std::fs::write(&tmp, serde_json::to_string_pretty(&r["tune"]).unwrap_or_default()).unwrap_or_else(|e| die(&e.to_string()));
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nvim".into());
            let ok = std::process::Command::new(&editor).arg(&tmp).status().map(|s| s.success()).unwrap_or(false);
            if !ok {
                die("editor failed");
            }
            let tune: Value = serde_json::from_slice(&std::fs::read(&tmp).unwrap_or_default()).unwrap_or_else(|_| die("not valid JSON; nothing saved"));
            let _ = std::fs::remove_file(&tmp);
            call(json!({"cmd": "save", "profile": p, "tune": tune}));
            println!("Saved \"{p}\".");
        }
        "boot" => {
            call(json!({"cmd": "boot", "enabled": on_off(rest.first())}));
        }
        "auto" => {
            call(json!({"cmd": "auto", "enabled": on_off(rest.first())}));
        }
        "rules" => {
            for (i, r) in read_status()["rules"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                println!("{i}: {} -> {}", r["matcher"].as_str().unwrap_or(""), r["profile"].as_str().unwrap_or(""));
            }
        }
        "rule" => match rest.first().map(String::as_str) {
            Some("add") => {
                let (Some(m), Some(p)) = (rest.get(1), rest.get(2)) else { die("rule add <matcher> <profile>") };
                call(json!({"cmd": "rule-add", "matcher": m, "profile": p}));
            }
            Some("del") => {
                let i: u64 = rest.get(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| die("rule del <n>"));
                call(json!({"cmd": "rule-del", "index": i}));
            }
            _ => die("rule add|del"),
        },
        "watchdog" => {
            let mut req = json!({"cmd": "watchdog"});
            if rest.iter().any(|a| a == "--on") {
                req["enabled"] = json!(true);
            }
            if rest.iter().any(|a| a == "--off") {
                req["enabled"] = json!(false);
            }
            for (fl, key) in [("--cpu-max", "cpu_max_c"), ("--gpu-max", "gpu_max_c"), ("--seconds", "seconds")] {
                if let Some(v) = flag(rest, fl).and_then(|v| v.parse::<f64>().ok()) {
                    req[key] = if key == "seconds" { json!(v as u64) } else { json!(v) };
                }
            }
            if let Some(v) = flag(rest, "--fallback") {
                req["fallback"] = json!(v);
            }
            if let Some(v) = flag(rest, "--errors") {
                req["on_errors"] = json!(on_off(Some(&v)));
            }
            println!("{}", serde_json::to_string_pretty(&call(req)).unwrap_or_default());
        }
        "confirm-time" => {
            let s: u64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or_else(|| die("confirm-time <seconds>"));
            call(json!({"cmd": "confirm-seconds", "seconds": s}));
        }
        "metrics" => {
            if rest.first().map(String::as_str) == Some("print") {
                print!("{}", std::fs::read_to_string(run_dir().join("metrics.prom")).unwrap_or_else(|e| die(&e.to_string())));
            } else {
                let mut req = json!({"cmd": "metrics"});
                if let Some(v) = flag(rest, "--http") {
                    req["http"] = json!(on_off(Some(&v)));
                }
                if let Some(v) = flag(rest, "--listen") {
                    req["listen"] = json!(v);
                }
                println!("{}", serde_json::to_string_pretty(&call(req)).unwrap_or_default());
            }
        }
        "csv" => {
            let mut req = json!({"cmd": "csv"});
            if let Some(s) = flag(rest, "--seconds").and_then(|s| s.parse::<f64>().ok()) {
                req["seconds"] = json!(s);
            }
            let csv = call(req).as_str().unwrap_or("").to_string();
            match rest.first().filter(|a| !a.starts_with("--")) {
                Some(file) => {
                    std::fs::write(file, &csv).unwrap_or_else(|e| die(&e.to_string()));
                    println!("Wrote {} rows to {file}", csv.lines().count().saturating_sub(1));
                }
                None => print!("{csv}"),
            }
        }
        "probe" => println!("{}", serde_json::to_string_pretty(&call(json!({"cmd": "probe"}))).unwrap_or_default()),
        "stress" => {
            unsafe {
                libc::signal(libc::SIGINT, on_sigint as *const () as libc::sighandler_t);
                libc::signal(libc::SIGTERM, on_sigint as *const () as libc::sighandler_t);
            }
            let kind = rest.first().map(String::as_str).unwrap_or("cpu");
            let (ok, _) = match kind {
                "cpu" => stress_cpu(rest, json_out),
                "gpu" => stress_gpu(rest, json_out),
                "all" => {
                    let (a, e1) = stress_cpu(rest, json_out);
                    if STOP.load(Ordering::Relaxed) {
                        (false, e1)
                    } else {
                        let (b, e2) = stress_gpu(rest, json_out);
                        (a && b, e1 + e2)
                    }
                }
                _ => die("stress cpu|gpu|all"),
            };
            std::process::exit(if ok { 0 } else { 1 });
        }
        other => die(&format!("unknown command \"{other}\" (see omarchy-oc --help)")),
    }
}
