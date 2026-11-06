//! # omarchy-oc-gpustress
//!
//! GPU stability tester / error detector for validating GPU overclocks (in the
//! spirit of OCCT's GPU error check). Part of omarchy-oc. Vulkan only (via
//! `wgpu`), prefers a discrete adapter.
//!
//! ## Usage
//!
//! ```text
//! omarchy-oc-gpustress [--seconds N] [--mode compute|vram|mixed] [--vram-percent P] [--json]
//! ```
//!
//! * `--seconds N`      test duration after setup (default 60)
//! * `--mode M`         `compute`, `vram` or `mixed` (default; alternates 5 s phases)
//! * `--vram-percent P` share of the VRAM heap to test (default 70, capped by
//!   the driver's VK_EXT_memory_budget budget minus 256 MiB)
//! * `--json`           one JSON object per line on stdout (setup notes go to stderr)
//!
//! ## Tests
//!
//! * **compute**: a 1M-invocation kernel runs, per invocation, an integer hash
//!   chain and 32 lanes of the chaotic map `y = fma(fma(y, A, B), y, c)` (all
//!   FMAs; conjugate to `z^2 + C` with C just above -2) for `rounds` rounds,
//!   calibrated at startup so one dispatch takes ~20 ms at full clocks. Every
//!   iteration runs the kernel twice with identical inputs and compares both
//!   outputs bit-for-bit on the GPU; in addition 1024 sampled integer chains per
//!   seed are checked against values computed on the CPU. Because the float map
//!   is chaotic and not sign-symmetric, a corrupted intermediate value changes
//!   the final bits. Floats are never compared against the CPU (FMA rounding
//!   may legitimately differ).
//! * **vram**: allocates ~P % of VRAM in up to 64 buffers (<= 1 GiB each, within
//!   `max_buffer_size` / `max_storage_buffer_binding_size`), fills everything with
//!   address-dependent patterns (walking ones, walking zeros, address ^ seed,
//!   its inverse, a hash of the address, its inverse; the seed rotates), then
//!   re-reads and verifies every word on the GPU, counting mismatches atomically.
//!
//! Dispatches stay ~20 ms (VRAM ones a few ms), batched into ~120 ms
//! submissions with two in flight, so the GPU never idles but the xe job
//! timeout is never approached and the desktop stays usable.
//!
//! ## Output
//!
//! Progress (about once per second), with `--json`:
//! `{"t":1.0,"mode":"mixed","phase":"compute","iterations":12,"errors":0,"gflops":9876.5,"vram_gbps":null,"adapter":"..."}`
//! `mode` is the requested mode, `phase` the test that was last submitted.
//! `gflops` / `vram_gbps` are the rates during that last interval (null when that
//! test did not run in it); `message` appears when errors were seen in it.
//! `iterations` counts compute iterations plus VRAM passes.
//! The final line adds `"done":true,"passed":bool,"interrupted":bool,
//! "compute_errors","vram_errors","compute_iterations","vram_passes","vram_tested_mib"`
//! (rates are then run averages) and `"message"` when something went wrong.
//!
//! Setup notes (adapter, VRAM sizing, calibration) go to stderr with `--json`.
//! `GPUSTRESS_DEBUG=1` additionally dumps the Vulkan memory heaps and budgets.
//!
//! Exit codes: 0 = passed, 1 = errors detected (including a GPU hang / device
//! loss during the test), 2 = could not run. SIGINT/SIGTERM stop the test and
//! still print the final line (a second signal exits immediately).

mod compute;
mod gpu;
mod vram;

use std::collections::VecDeque;
use std::io::Write;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const NSLOTS: usize = 2;
const BATCH_TARGET: Duration = Duration::from_millis(120);
const PHASE_LEN: f64 = 5.0;
const REPORT_EVERY: Duration = Duration::from_secs(1);

static STOP: AtomicBool = AtomicBool::new(false);
static PANIC_MSG: Mutex<Option<String>> = Mutex::new(None);

extern "C" fn on_signal(_: libc::c_int) {
    if STOP.swap(true, Ordering::SeqCst) {
        unsafe { libc::_exit(130) };
    }
}

fn stopped() -> bool {
    STOP.load(Ordering::SeqCst)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Compute,
    Vram,
    Mixed,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Compute => "compute",
            Mode::Vram => "vram",
            Mode::Mixed => "mixed",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Compute,
    Vram,
}

struct Args {
    seconds: f64,
    mode: Mode,
    vram_percent: f64,
    json: bool,
    inject: bool,
}

const USAGE: &str = "usage: omarchy-oc-gpustress [--seconds N] [--mode compute|vram|mixed] [--vram-percent P] [--json]";

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        seconds: 60.0,
        mode: Mode::Mixed,
        vram_percent: 70.0,
        json: false,
        inject: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) => (k.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };
        let mut val = |name: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or(format!("{name} needs a value"))
        };
        match key.as_str() {
            "--seconds" | "-s" => {
                a.seconds = val("--seconds")?
                    .parse()
                    .map_err(|_| "bad --seconds".to_string())?;
                if !a.seconds.is_finite() || a.seconds <= 0.0 {
                    return Err("--seconds must be > 0".into());
                }
            }
            "--mode" | "-m" => {
                a.mode = match val("--mode")?.as_str() {
                    "compute" => Mode::Compute,
                    "vram" => Mode::Vram,
                    "mixed" => Mode::Mixed,
                    m => return Err(format!("unknown mode '{m}'")),
                }
            }
            "--vram-percent" => {
                a.vram_percent = val("--vram-percent")?
                    .parse()
                    .map_err(|_| "bad --vram-percent".to_string())?;
                if !(1.0..=95.0).contains(&a.vram_percent) {
                    return Err("--vram-percent must be within 1..95".into());
                }
            }
            "--json" => a.json = true,
            "--inject-fault" => a.inject = true, // hidden: self-test of error detection
            "--help" | "-h" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    Ok(a)
}

/// Per-submission error counters (+ their CPU readback buffer).
pub struct Slot {
    pub err: wgpu::Buffer,
    pub readback: wgpu::Buffer,
}

#[derive(Default)]
struct Stats {
    compute_iters: u64,
    vram_passes: u64,
    compute_errors: u64,
    vram_errors: u64,
    fatal_errors: u64,
    // current report window
    w_flops: f64,
    w_ctime: f64,
    w_bytes: f64,
    w_vtime: f64,
    w_ab: u64,
    w_cpu: u64,
    w_vram: u64,
    // whole run
    t_flops: f64,
    t_ctime: f64,
    t_bytes: f64,
    t_vtime: f64,
}

impl Stats {
    fn errors(&self) -> u64 {
        self.compute_errors + self.vram_errors + self.fatal_errors
    }
    fn iterations(&self) -> u64 {
        self.compute_iters + self.vram_passes
    }
}

struct Outcome {
    adapter: String,
    vram_tested: u64,
    /// Fatal problem: (message, counts as test error?)
    fatal: Option<(String, bool)>,
    interrupted: bool,
    elapsed: f64,
}

fn rate(work: f64, time: f64, scale: f64) -> Option<f64> {
    (time > 0.0 && work > 0.0).then(|| work / time / scale)
}

fn jstr(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jnum(v: Option<f64>) -> String {
    match v {
        Some(x) if x.is_finite() => format!("{:.1}", x),
        _ => "null".into(),
    }
}

fn emit(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn report_progress(args: &Args, adapter: &str, t: f64, phase: &str, st: &mut Stats) {
    let gflops = rate(st.w_flops, st.w_ctime, 1e9);
    let gbps = rate(st.w_bytes, st.w_vtime, 1e9);
    let mut parts = Vec::new();
    if st.w_ab + st.w_cpu > 0 {
        parts.push(format!(
            "compute: {} mismatches (run A vs B: {}, vs CPU reference: {})",
            st.w_ab + st.w_cpu,
            st.w_ab,
            st.w_cpu
        ));
    }
    if st.w_vram > 0 {
        parts.push(format!("vram: {} mismatching words", st.w_vram));
    }
    let msg = parts.join("; ");
    (st.w_ab, st.w_cpu, st.w_vram) = (0, 0, 0);
    if args.json {
        let mut l = format!(
            "{{\"t\":{t:.2},\"mode\":{},\"phase\":{},\"iterations\":{},\"errors\":{},\"gflops\":{},\"vram_gbps\":{},\"adapter\":{}",
            jstr(args.mode.name()),
            jstr(phase),
            st.iterations(),
            st.errors(),
            jnum(gflops),
            jnum(gbps),
            jstr(adapter)
        );
        if !msg.is_empty() {
            l.push_str(&format!(",\"message\":{}", jstr(&msg)));
        }
        l.push('}');
        emit(&l);
    } else {
        let g = gflops.map_or("      -".into(), |g| format!("{g:7.0}"));
        let b = gbps.map_or("    -".into(), |b| format!("{b:5.0}"));
        emit(&format!(
            "[{t:6.1}s] {phase:<7} iter {:>6}  errors {:>4}  {g} GFLOPS  {b} GB/s{}",
            st.iterations(),
            st.errors(),
            if msg.is_empty() {
                String::new()
            } else {
                format!("  ! {msg}")
            }
        ));
    }
    st.w_flops = 0.0;
    st.w_ctime = 0.0;
    st.w_bytes = 0.0;
    st.w_vtime = 0.0;
}

fn report_final(args: &Args, st: &Stats, o: &Outcome) -> i32 {
    let errors = st.errors();
    let internal = matches!(o.fatal, Some((_, false)));
    let passed = errors == 0 && !internal;
    let code = if internal {
        2
    } else if errors > 0 {
        1
    } else {
        0
    };
    let gflops = rate(st.t_flops, st.t_ctime, 1e9);
    let gbps = rate(st.t_bytes, st.t_vtime, 1e9);
    let mut msgs = Vec::new();
    if let Some((m, _)) = &o.fatal {
        msgs.push(m.clone());
    }
    if st.compute_errors > 0 {
        msgs.push(format!("{} compute result mismatches", st.compute_errors));
    }
    if st.vram_errors > 0 {
        msgs.push(format!("{} VRAM word mismatches", st.vram_errors));
    }
    let msg = msgs.join("; ");
    if args.json {
        let mut l = format!(
            "{{\"t\":{:.2},\"mode\":{},\"iterations\":{},\"errors\":{},\"gflops\":{},\"vram_gbps\":{},\"adapter\":{},\"done\":true,\"passed\":{},\"interrupted\":{},\"compute_errors\":{},\"vram_errors\":{},\"compute_iterations\":{},\"vram_passes\":{},\"vram_tested_mib\":{}",
            o.elapsed,
            jstr(args.mode.name()),
            st.iterations(),
            errors,
            jnum(gflops),
            jnum(gbps),
            jstr(&o.adapter),
            passed,
            o.interrupted,
            st.compute_errors,
            st.vram_errors,
            st.compute_iters,
            st.vram_passes,
            o.vram_tested >> 20
        );
        if !msg.is_empty() {
            l.push_str(&format!(",\"message\":{}", jstr(&msg)));
        }
        l.push('}');
        emit(&l);
    } else {
        emit(&format!(
            "\n{} after {:.1}s on {}{}",
            if passed { "PASSED" } else { "FAILED" },
            o.elapsed,
            if o.adapter.is_empty() {
                "?"
            } else {
                &o.adapter
            },
            if o.interrupted { " (interrupted)" } else { "" }
        ));
        if args.mode != Mode::Vram {
            emit(&format!(
                "  compute: {} iterations, {} errors{}",
                st.compute_iters,
                st.compute_errors,
                gflops.map_or(String::new(), |g| format!(", avg {g:.0} GFLOPS"))
            ));
        }
        if args.mode != Mode::Compute {
            emit(&format!(
                "  vram:    {} passes over {} MiB, {} errors{}",
                st.vram_passes,
                o.vram_tested >> 20,
                st.vram_errors,
                gbps.map_or(String::new(), |b| format!(", avg {b:.0} GB/s"))
            ));
        }
        if !msg.is_empty() {
            emit(&format!("  {msg}"));
        }
    }
    code
}

fn note(args: &Args, msg: &str) {
    if args.json {
        eprintln!("gpustress: {msg}");
    } else {
        emit(msg);
    }
}

struct InFlight {
    idx: wgpu::SubmissionIndex,
    kind: Kind,
    iters: u32,
    submitted: Instant,
    read: gpu::PendingRead,
}

fn run(args: &Args, st: &mut Stats, o: &mut Outcome) {
    let g = match gpu::init() {
        Ok(g) => g,
        Err(e) => {
            o.fatal = Some((e, false));
            return;
        }
    };
    o.adapter = g.name.clone();
    note(args, &format!("adapter: {}", g.name));

    let slots: Vec<Slot> = (0..NSLOTS)
        .map(|_| Slot {
            err: g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("errors"),
                size: 16,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            readback: g.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("errors readback"),
                size: 16,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
        })
        .collect();

    let mut ctest = None;
    let mut vtest = None;
    if args.mode != Mode::Compute {
        match vram::VramTest::new(&g, &slots, args.vram_percent, args.inject) {
            Ok(v) => {
                note(args, &v.note);
                o.vram_tested = v.total_bytes;
                vtest = Some(v);
            }
            Err(e) => {
                o.fatal = Some((e, false));
                return;
            }
        }
    }
    if args.mode != Mode::Vram {
        let mut c = compute::ComputeTest::new(&g, &slots, args.inject);
        if let Err(e) = c.calibrate(&g, &stopped) {
            st.fatal_errors += 1;
            o.fatal = Some((format!("during calibration: {e}"), true));
            return;
        }
        note(args, &format!("compute: {}", c.describe()));
        ctest = Some(c);
    }
    if args.inject {
        note(args, "fault injection enabled: errors are expected");
    }
    let msgs = g.errors.take();
    if !msgs.is_empty() {
        let lost = g.errors.is_lost();
        if lost {
            st.fatal_errors += 1;
        }
        o.fatal = Some((format!("during setup: {}", msgs.join("; ")), lost));
        return;
    }

    let start = Instant::now();
    let duration = Duration::from_secs_f64(args.seconds);
    let mut last_report = Instant::now();
    let mut last_done = Instant::now();
    let mut inflight: VecDeque<InFlight> = VecDeque::new();
    let mut next_slot = 0usize;
    let mut iters = [2u32, 1u32]; // per batch: compute iterations, vram passes
    let mut phase = Kind::Compute;

    loop {
        let elapsed = start.elapsed();
        let finishing = stopped() || elapsed >= duration || o.fatal.is_some();

        if !finishing {
            phase = match args.mode {
                Mode::Compute => Kind::Compute,
                Mode::Vram => Kind::Vram,
                Mode::Mixed => {
                    if ((elapsed.as_secs_f64() / PHASE_LEN) as u64).is_multiple_of(2) {
                        Kind::Compute
                    } else {
                        Kind::Vram
                    }
                }
            };
            let slot = next_slot;
            next_slot = (next_slot + 1) % NSLOTS;
            let n = iters[phase as usize];
            let mut enc = g.device.create_command_encoder(&Default::default());
            enc.clear_buffer(&slots[slot].err, 0, None);
            match phase {
                Kind::Compute => ctest.as_mut().unwrap().encode(&mut enc, slot, n),
                Kind::Vram => vtest.as_mut().unwrap().encode(&g, &mut enc, slot, n),
            }
            enc.copy_buffer_to_buffer(&slots[slot].err, 0, &slots[slot].readback, 0, 16);
            let idx = g.queue.submit([enc.finish()]);
            let read = g.start_read(&slots[slot].readback);
            inflight.push_back(InFlight {
                idx,
                kind: phase,
                iters: n,
                submitted: Instant::now(),
                read,
            });
        }

        if inflight.len() >= NSLOTS || (finishing && !inflight.is_empty()) {
            let f = inflight.pop_front().unwrap();
            match g.wait(f.idx).and_then(|_| g.finish_read(f.read)) {
                Ok(c) => {
                    let now = Instant::now();
                    let dt = (now - f.submitted.max(last_done)).as_secs_f64();
                    last_done = now;
                    match f.kind {
                        Kind::Compute => {
                            let ct = ctest.as_ref().unwrap();
                            let e = c[0] as u64 + c[1] as u64;
                            st.w_ab += c[0] as u64;
                            st.w_cpu += c[1] as u64;
                            st.compute_errors += e;
                            st.compute_iters += f.iters as u64;
                            let w = ct.flops_per_iter() * f.iters as f64;
                            st.w_flops += w;
                            st.w_ctime += dt;
                            st.t_flops += w;
                            st.t_ctime += dt;
                        }
                        Kind::Vram => {
                            let vt = vtest.as_ref().unwrap();
                            let e = c[0] as u64;
                            st.w_vram += e;
                            st.vram_errors += e;
                            st.vram_passes += f.iters as u64;
                            let w = vt.bytes_per_iter() * f.iters as f64;
                            st.w_bytes += w;
                            st.w_vtime += dt;
                            st.t_bytes += w;
                            st.t_vtime += dt;
                        }
                    }
                    // Adapt batch size towards BATCH_TARGET of GPU time.
                    let k = f.kind as usize;
                    let per = dt / f.iters as f64;
                    let want = (BATCH_TARGET.as_secs_f64() / per.max(1e-6)).round() as u32;
                    let max = if f.kind == Kind::Vram {
                        vram::MAX_PASSES_PER_BATCH
                    } else {
                        64
                    };
                    iters[k] = ((iters[k] + want.clamp(1, max)) / 2).max(1);
                }
                Err(e) => {
                    st.fatal_errors += 1;
                    let mut m = e;
                    for extra in g.errors.take() {
                        m.push_str("; ");
                        m.push_str(&extra);
                    }
                    o.fatal = Some((m, true));
                    break;
                }
            }
        } else if finishing {
            break;
        }

        // Asynchronously reported problems.
        let msgs = g.errors.take();
        if !msgs.is_empty() {
            let lost = g.errors.is_lost();
            if lost {
                st.fatal_errors += 1;
            }
            o.fatal = Some((msgs.join("; "), lost));
        }

        if last_report.elapsed() >= REPORT_EVERY {
            last_report = Instant::now();
            let name = match phase {
                Kind::Compute => "compute",
                Kind::Vram => "vram",
            };
            report_progress(args, &o.adapter, start.elapsed().as_secs_f64(), name, st);
        }
    }
    o.elapsed = start.elapsed().as_secs_f64();
    o.interrupted = stopped();
    // Leak GPU objects: after a device loss their destructors may misbehave,
    // and the process exits right after the summary anyway.
    std::mem::forget(ctest);
    std::mem::forget(vtest);
    std::mem::forget(slots);
    std::mem::forget(g);
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("omarchy-oc-gpustress: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
    std::panic::set_hook(Box::new(|info| {
        let msg = match info.payload().downcast_ref::<&str>() {
            Some(s) => s.to_string(),
            None => info
                .payload()
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_else(|| info.to_string()),
        };
        if let Ok(mut p) = PANIC_MSG.lock() {
            *p = Some(msg);
        }
    }));

    let mut st = Stats::default();
    let mut o = Outcome {
        adapter: String::new(),
        vram_tested: 0,
        fatal: None,
        interrupted: false,
        elapsed: 0.0,
    };
    let started = Instant::now();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&args, &mut st, &mut o)));
    if r.is_err() {
        let m = PANIC_MSG
            .lock()
            .ok()
            .and_then(|mut p| p.take())
            .unwrap_or_else(|| "panic".into());
        let lost = m.to_lowercase().contains("lost") || m.to_lowercase().contains("hang");
        if lost {
            st.fatal_errors += 1;
        }
        o.fatal = Some((format!("GPU failure: {}", m.trim()), lost));
        if o.elapsed == 0.0 {
            o.elapsed = started.elapsed().as_secs_f64();
        }
        o.interrupted = stopped();
    }
    let code = report_final(&args, &st, &o);
    std::process::exit(code);
}
