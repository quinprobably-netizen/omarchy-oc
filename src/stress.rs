//! CPU stability test with error detection.
//!
//! Each worker runs a fixed set of deterministic work blocks (AVX-512 or
//! AVX2 FMA chains, integer hash chains, and a cache-busting memory walk)
//! and compares every result bit-for-bit against a golden value computed at
//! start. A wrong bit means the core computed something wrong: the classic
//! symptom of too much Curve Optimizer undervolt, which can pass for hours
//! without crashing. Errors are attributed to the core that made them, so
//! the per-core mode doubles as a Curve Optimizer tuner (like CoreCycler).

use serde::Serialize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const SEEDS: usize = 16;
const VEC_LEN: usize = 2048;
const MEM_WORDS: usize = 1 << 20; // 8 MiB per worker: spills L2, lives in L3

#[derive(Clone, Copy, Debug, Serialize, PartialEq)]
pub enum Isa {
    Avx512,
    Avx2,
    Scalar,
}

pub fn best_isa() -> Isa {
    #[cfg(target_arch = "x86_64")]
    {
        if is_x86_feature_detected!("avx512f") && is_x86_feature_detected!("fma") {
            return Isa::Avx512;
        }
        if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
            return Isa::Avx2;
        }
    }
    Isa::Scalar
}

#[inline(always)]
fn fma_body(seed: u64, rounds: usize) -> u64 {
    let mut a = [0f64; VEC_LEN];
    let mut x = seed | 1;
    for v in a.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *v = (x >> 11) as f64 / (1u64 << 53) as f64;
    }
    let (m, c) = (0.999_999_9_f64, 0.000_000_1_f64);
    for _ in 0..rounds {
        // Bounded: v -> v*m + c keeps v in [0, 1].
        for v in a.iter_mut() {
            *v = v.mul_add(m, c);
            *v = v.mul_add(m, c * 0.5);
        }
    }
    let mut h = 0xcbf29ce484222325u64;
    for v in a {
        h = (h ^ v.to_bits()).wrapping_mul(0x100000001b3);
    }
    h
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx512f,avx512dq,avx512vl,fma,avx2")]
unsafe fn fma_avx512(seed: u64, rounds: usize) -> u64 {
    fma_body(seed, rounds)
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn fma_avx2(seed: u64, rounds: usize) -> u64 {
    fma_body(seed, rounds)
}

fn fma_block(isa: Isa, seed: u64, rounds: usize) -> u64 {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        match isa {
            Isa::Avx512 => return fma_avx512(seed, rounds),
            Isa::Avx2 => return fma_avx2(seed, rounds),
            Isa::Scalar => {}
        }
    }
    fma_body(seed, rounds)
}

/// Integer multiply/rotate chains: hits the integer multipliers and the
/// branch-free ALU paths.
fn int_block(seed: u64, n: usize) -> u64 {
    let mut s = [seed, seed ^ 0x9E3779B97F4A7C15, seed.rotate_left(17), !seed];
    for i in 0..n {
        for (k, v) in s.iter_mut().enumerate() {
            *v = v.wrapping_mul(0x2545F4914F6CDD1D).rotate_left((k as u32 * 7 + i as u32) & 63) ^ (i as u64);
        }
        s.rotate_left(1);
    }
    s.iter().fold(0, |a, b| a ^ b)
}

/// Pseudo-random read-modify-write walk over a buffer bigger than L2.
fn mem_block(buf: &mut [u64], seed: u64, steps: usize) -> u64 {
    let n = buf.len();
    for (i, w) in buf.iter_mut().enumerate() {
        *w = (i as u64).wrapping_mul(0x9E3779B97F4A7C15) ^ seed;
    }
    let mut idx = (seed as usize) % n;
    let mut acc = seed;
    for _ in 0..steps {
        let v = buf[idx];
        acc = acc.wrapping_add(v).rotate_left(5);
        buf[idx] = v ^ acc;
        idx = (v as usize ^ (acc as usize)) % n;
    }
    acc ^ buf.iter().step_by(4099).fold(0, |a, b| a ^ b)
}

#[derive(Clone, Copy)]
pub struct Work {
    pub isa: Isa,
    pub fma_rounds: usize,
    pub int_iters: usize,
    pub mem_steps: usize,
}

impl Default for Work {
    fn default() -> Self {
        // About 20–40 ms per block on a Zen 4 core.
        Work { isa: best_isa(), fma_rounds: 600, int_iters: 400_000, mem_steps: 300_000 }
    }
}

pub fn block(w: &Work, buf: &mut [u64], seed: u64) -> [u64; 3] {
    [fma_block(w.isa, seed, w.fma_rounds), int_block(seed, w.int_iters), mem_block(buf, seed, w.mem_steps)]
}

pub fn golden(w: &Work) -> Result<Vec<[u64; 3]>, String> {
    let mut buf = vec![0u64; MEM_WORDS];
    let a: Vec<[u64; 3]> = (0..SEEDS as u64).map(|s| block(w, &mut buf, s * 7919 + 1)).collect();
    let b: Vec<[u64; 3]> = (0..SEEDS as u64).map(|s| block(w, &mut buf, s * 7919 + 1)).collect();
    if a != b {
        return Err("reference results differ between two runs at light load: the CPU is already unstable".into());
    }
    Ok(a)
}

pub fn pin(cpu: u32) {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_SET(cpu as usize, &mut set);
        libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
    }
}

#[derive(Default)]
pub struct Counters {
    pub blocks: AtomicU64,
    pub errors: AtomicU64,
}

/// Run blocks pinned to `cpu` until `stop`, counting into `c`.
pub fn worker(cpu: u32, w: Work, gold: Arc<Vec<[u64; 3]>>, c: Arc<Counters>, stop: Arc<AtomicBool>, inject: bool) {
    pin(cpu);
    let mut buf = vec![0u64; MEM_WORDS];
    let mut i = (cpu as usize * 5) % SEEDS;
    while !stop.load(Ordering::Relaxed) {
        let seed = i as u64 * 7919 + 1;
        let mut r = block(&w, &mut buf, seed);
        if inject && i % 7 == 3 {
            r[0] ^= 1;
        }
        c.blocks.fetch_add(1, Ordering::Relaxed);
        if r != gold[i] {
            c.errors.fetch_add(1, Ordering::Relaxed);
        }
        i = (i + 1) % SEEDS;
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct CoreResult {
    pub core: u32,
    pub cpus: Vec<u32>,
    pub blocks: u64,
    pub errors: u64,
    pub seconds: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub t: f64,
    pub phase: String,
    pub blocks: u64,
    pub errors: u64,
    pub blocks_per_s: f64,
    pub cores: Vec<CoreResult>,
    pub done: bool,
    pub passed: Option<bool>,
    pub isa: Isa,
    pub message: Option<String>,
}

pub struct Plan {
    /// (core id, its logical CPUs)
    pub cores: Vec<(u32, Vec<u32>)>,
    pub seconds: f64,
    pub per_core: bool,
    pub per_core_seconds: f64,
    pub inject_fault: bool,
}

/// Run the test, calling `report` about once a second.
pub fn run(plan: &Plan, stop: Arc<AtomicBool>, mut report: impl FnMut(&Progress)) -> Progress {
    let w = Work::default();
    let t0 = Instant::now();
    let base = |phase: &str| Progress {
        t: 0.0,
        phase: phase.into(),
        blocks: 0,
        errors: 0,
        blocks_per_s: 0.0,
        cores: vec![],
        done: false,
        passed: None,
        isa: w.isa,
        message: None,
    };
    report(&base("reference"));
    let gold = match golden(&w) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            let mut p = base("reference");
            p.done = true;
            p.passed = Some(false);
            p.errors = 1;
            p.message = Some(e);
            report(&p);
            return p;
        }
    };
    // Phases: all threads at once, then (optionally) each core alone, where
    // single-core boost runs the highest clocks and lowest voltage margin.
    let mut phases: Vec<(String, Vec<(u32, Vec<u32>)>, f64)> = vec![("all cores".into(), plan.cores.clone(), plan.seconds)];
    if plan.per_core {
        for c in &plan.cores {
            phases.push((format!("core {}", c.0), vec![c.clone()], plan.per_core_seconds));
        }
    }
    let mut totals: Vec<CoreResult> =
        plan.cores.iter().map(|(core, cpus)| CoreResult { core: *core, cpus: cpus.clone(), blocks: 0, errors: 0, seconds: 0.0 }).collect();
    let mut last = base("all cores");
    for (name, cores, secs) in phases {
        if stop.load(Ordering::Relaxed) || secs <= 0.0 {
            continue;
        }
        let pstop = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();
        let mut per: Vec<(u32, Arc<Counters>)> = Vec::new();
        for (core, cpus) in &cores {
            // In the per-core phase only the first thread runs, so the core
            // gets its full single-thread boost.
            let use_cpus: Vec<u32> = if name == "all cores" { cpus.clone() } else { cpus.iter().take(1).copied().collect() };
            let c = Arc::new(Counters::default());
            per.push((*core, c.clone()));
            for cpu in use_cpus {
                let (g, c, s) = (gold.clone(), c.clone(), pstop.clone());
                let inject = plan.inject_fault;
                handles.push(std::thread::spawn(move || worker(cpu, w, g, c, s, inject)));
            }
        }
        let ps = Instant::now();
        let mut prev_blocks = 0;
        let mut prev_t = 0.0;
        while ps.elapsed().as_secs_f64() < secs && !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
            let el = ps.elapsed().as_secs_f64();
            if el - prev_t < 1.0 && el < secs {
                continue;
            }
            let blocks: u64 = per.iter().map(|(_, c)| c.blocks.load(Ordering::Relaxed)).sum();
            let mut p = base(&name);
            p.t = t0.elapsed().as_secs_f64();
            p.blocks_per_s = (blocks - prev_blocks) as f64 / (el - prev_t).max(0.001);
            prev_blocks = blocks;
            prev_t = el;
            p.cores = totals
                .iter()
                .map(|tot| {
                    let mut r = tot.clone();
                    if let Some((_, c)) = per.iter().find(|(id, _)| *id == tot.core) {
                        r.blocks += c.blocks.load(Ordering::Relaxed);
                        r.errors += c.errors.load(Ordering::Relaxed);
                    }
                    r
                })
                .collect();
            p.blocks = p.cores.iter().map(|c| c.blocks).sum();
            p.errors = p.cores.iter().map(|c| c.errors).sum();
            report(&p);
            last = p;
        }
        pstop.store(true, Ordering::Relaxed);
        for h in handles {
            let _ = h.join();
        }
        let el = ps.elapsed().as_secs_f64();
        for (core, c) in &per {
            if let Some(t) = totals.iter_mut().find(|t| t.core == *core) {
                t.blocks += c.blocks.load(Ordering::Relaxed);
                t.errors += c.errors.load(Ordering::Relaxed);
                t.seconds += el;
            }
        }
    }
    let mut p = base("done");
    p.t = t0.elapsed().as_secs_f64();
    p.cores = totals;
    p.blocks = p.cores.iter().map(|c| c.blocks).sum();
    p.errors = p.cores.iter().map(|c| c.errors).sum();
    p.blocks_per_s = last.blocks_per_s;
    p.done = true;
    p.passed = Some(p.errors == 0 && !stop.load(Ordering::Relaxed));
    if stop.load(Ordering::Relaxed) {
        p.message = Some("stopped before the end".into());
    } else if p.errors > 0 {
        let bad: Vec<String> = p.cores.iter().filter(|c| c.errors > 0).map(|c| format!("core {}", c.core)).collect();
        p.message = Some(format!("computation errors on {}: reduce the undervolt (Curve Optimizer) there", bad.join(", ")));
    }
    report(&p);
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_detects_faults() {
        let w = Work { isa: best_isa(), fma_rounds: 4, int_iters: 1000, mem_steps: 1000 };
        let mut buf = vec![0u64; 4096];
        assert_eq!(block(&w, &mut buf, 5), block(&w, &mut buf, 5));
        assert_ne!(block(&w, &mut buf, 5), block(&w, &mut buf, 6));
        // Scalar and SIMD builds of the same chain must agree bit-for-bit
        // (FMA is fused in both).
        let s = Work { isa: Isa::Scalar, ..w };
        if w.isa != Isa::Scalar {
            assert_eq!(fma_block(w.isa, 9, 3), fma_block(s.isa, 9, 3));
        }
    }

    #[test]
    fn short_run_with_injected_fault_fails() {
        let plan = Plan { cores: vec![(0, vec![0])], seconds: 1.5, per_core: false, per_core_seconds: 0.0, inject_fault: true };
        let r = run(&plan, Arc::new(AtomicBool::new(false)), |_| {});
        assert_eq!(r.passed, Some(false));
        assert!(r.errors > 0);
    }
}
