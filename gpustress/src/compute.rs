//! Compute (ALU) stress test: two identical dispatches of a long FMA +
//! integer-hash kernel, compared bit-for-bit on the GPU, plus a CPU-computed
//! sample of the integer chain.

use crate::gpu::Gpu;
use crate::{NSLOTS, Slot};
use std::time::{Duration, Instant};

const WG: u32 = 256;
/// Invocations per dispatch.
const N: u32 = 4096 * WG;
const N_SEEDS: usize = 4;
const N_SAMPLES: u32 = 1024;
/// FLOPs per invocation per round: 16 vec4 FMAs = 64 FMAs = 128 FLOPs.
const FLOPS_PER_ROUND: f64 = 128.0;
/// Target GPU time of one stress dispatch.
const TARGET_DISPATCH: Duration = Duration::from_millis(20);
const NO_INJECT: u32 = u32::MAX;

pub struct ComputeTest {
    stress: wgpu::ComputePipeline,
    check: wgpu::ComputePipeline,
    params: Vec<[wgpu::Buffer; 2]>,
    samples: Vec<wgpu::Buffer>,
    stress_bg: Vec<[wgpu::BindGroup; 2]>,
    check_bg: Vec<Vec<wgpu::BindGroup>>,
    pub rounds: u32,
    cursor: usize,
    inject: bool,
}

pub fn mix32(x: u32) -> u32 {
    let mut h = x;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846ca68b);
    h ^= h >> 16;
    h
}

fn seed_value(s: usize) -> u32 {
    mix32(0xC0FFEE ^ (s as u32).wrapping_mul(0x9e3779b9))
}

/// CPU reference for the integer chain of invocation `gid` (no fault injected).
fn int_chain(gid: u32, seed: u32, rounds: u32) -> u32 {
    let mut x = mix32(gid ^ mix32(seed));
    for _ in 0..rounds {
        x = mix32(x.wrapping_add(0x9e3779b9));
    }
    x
}

fn sample_indices(seed: u32) -> impl Iterator<Item = u32> {
    let step = N / N_SAMPLES;
    (0..N_SAMPLES).map(move |k| k * step + mix32(k ^ seed) % step)
}

pub fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

pub fn uniform_entry(binding: u32, dynamic: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: dynamic,
            min_binding_size: None,
        },
        count: None,
    }
}

pub fn pipeline(
    gpu: &Gpu,
    src: &str,
    entry: &str,
    layout: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(entry),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
    let pl = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(entry),
            bind_group_layouts: &[Some(layout)],
            immediate_size: 0,
        });
    gpu.device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&pl),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
}

impl ComputeTest {
    pub fn new(gpu: &Gpu, slots: &[Slot], inject: bool) -> Self {
        let dev = &gpu.device;
        let stress_layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("stress"),
            entries: &[uniform_entry(0, false), storage_entry(1, false)],
        });
        let check_layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("check"),
            entries: &[
                storage_entry(0, true),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, false),
            ],
        });
        let stress = pipeline(
            gpu,
            include_str!("shaders/compute.wgsl"),
            "stress",
            &stress_layout,
        );
        let check = pipeline(
            gpu,
            include_str!("shaders/check.wgsl"),
            "check",
            &check_layout,
        );

        let out = |label| {
            dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: N as u64 * 8,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        };
        let outs = [out("result A"), out("result B")];
        let params: Vec<[wgpu::Buffer; 2]> = (0..N_SEEDS)
            .map(|_| {
                std::array::from_fn(|_| {
                    dev.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("stress params"),
                        size: 16,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    })
                })
            })
            .collect();
        let samples: Vec<wgpu::Buffer> = (0..N_SEEDS)
            .map(|_| {
                dev.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("cpu samples"),
                    size: N_SAMPLES as u64 * 8,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let stress_bg = params
            .iter()
            .map(|pp| {
                std::array::from_fn(|r| {
                    dev.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("stress"),
                        layout: &stress_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: pp[r].as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: outs[r].as_entire_binding(),
                            },
                        ],
                    })
                })
            })
            .collect();
        let check_bg = samples
            .iter()
            .map(|smp| {
                slots
                    .iter()
                    .map(|slot| {
                        dev.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("check"),
                            layout: &check_layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: outs[0].as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: outs[1].as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: smp.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 3,
                                    resource: slot.err.as_entire_binding(),
                                },
                            ],
                        })
                    })
                    .collect()
            })
            .collect();
        debug_assert_eq!(slots.len(), NSLOTS);

        let t = ComputeTest {
            stress,
            check,
            params,
            samples,
            stress_bg,
            check_bg,
            rounds: 256,
            cursor: 0,
            inject,
        };
        t.write_params(gpu, false);
        t
    }

    fn write_params(&self, gpu: &Gpu, with_inject: bool) {
        for (s, pp) in self.params.iter().enumerate() {
            let seed = seed_value(s);
            let first_sample = sample_indices(seed).next().unwrap();
            let (ia, fb) = if with_inject && self.inject {
                // Run A: corrupt the int chain of a CPU-sampled index.
                // Run B: corrupt one float lane of another index.
                (first_sample, N / 2 + 7)
            } else {
                (NO_INJECT, NO_INJECT)
            };
            let a = [seed, self.rounds, ia, NO_INJECT];
            let b = [seed, self.rounds, NO_INJECT, fb];
            gpu.queue.write_buffer(&pp[0], 0, bytemuck::cast_slice(&a));
            gpu.queue.write_buffer(&pp[1], 0, bytemuck::cast_slice(&b));
        }
    }

    /// Ramp the clocks and pick `rounds` so one dispatch takes ~TARGET_DISPATCH.
    pub fn calibrate(&mut self, gpu: &Gpu, stop: &dyn Fn() -> bool) -> Result<(), String> {
        let start = Instant::now();
        for i in 0..40 {
            self.write_params(gpu, false);
            let t0 = Instant::now();
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(&self.stress);
                pass.set_bind_group(0, &self.stress_bg[0][0], &[]);
                pass.dispatch_workgroups(N / WG, 1, 1);
            }
            let idx = gpu.queue.submit([enc.finish()]);
            gpu.wait(idx)?;
            let dt = t0.elapsed().as_secs_f64();
            let scale = (TARGET_DISPATCH.as_secs_f64() / dt.max(1e-4)).clamp(0.25, 4.0);
            self.rounds = ((self.rounds as f64 * scale) as u32).clamp(16, 1 << 16);
            if (i >= 4 && start.elapsed() > Duration::from_millis(1500)) || stop() {
                break;
            }
        }
        self.write_params(gpu, true);
        for (s, buf) in self.samples.iter().enumerate() {
            let seed = seed_value(s);
            let data: Vec<u32> = sample_indices(seed)
                .flat_map(|i| [i, int_chain(i, seed, self.rounds)])
                .collect();
            gpu.queue.write_buffer(buf, 0, bytemuck::cast_slice(&data));
        }
        Ok(())
    }

    /// Floating-point operations per iteration (both runs).
    pub fn flops_per_iter(&self) -> f64 {
        2.0 * N as f64 * self.rounds as f64 * FLOPS_PER_ROUND
    }

    pub fn encode(&mut self, enc: &mut wgpu::CommandEncoder, slot: usize, iters: u32) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("compute stress"),
            timestamp_writes: None,
        });
        for _ in 0..iters {
            let s = self.cursor % N_SEEDS;
            self.cursor += 1;
            pass.set_pipeline(&self.stress);
            for r in 0..2 {
                pass.set_bind_group(0, &self.stress_bg[s][r], &[]);
                pass.dispatch_workgroups(N / WG, 1, 1);
            }
            pass.set_pipeline(&self.check);
            pass.set_bind_group(0, &self.check_bg[s][slot], &[]);
            pass.dispatch_workgroups(N / WG, 1, 1);
        }
    }

    pub fn describe(&self) -> String {
        format!(
            "{} invocations x {} rounds per dispatch, 2 runs + check per iteration",
            N, self.rounds
        )
    }
}
