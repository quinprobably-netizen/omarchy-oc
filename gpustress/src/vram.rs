//! VRAM test: fill most of VRAM (split into chunk buffers) with
//! address-dependent rotating patterns, then verify every word on the GPU.

use crate::Slot;
use crate::compute::{mix32, pipeline, storage_entry, uniform_entry};
use crate::gpu::Gpu;

const WG: u32 = 256;
const MAX_WORKGROUPS: u32 = 8192;
/// Uniform entries are 256-byte aligned (dynamic offsets).
const ENTRY: u64 = 256;
pub const MAX_PASSES_PER_BATCH: u32 = 32;
const MAX_CHUNKS: usize = 64;
const N_PATTERNS: u64 = 6;
const NO_INJECT: u32 = u32::MAX;
const MIB: u64 = 1 << 20;

pub struct VramTest {
    fill: wgpu::ComputePipeline,
    verify: wgpu::ComputePipeline,
    params: wgpu::Buffer,
    chunks: Vec<(wgpu::Buffer, u64)>,
    /// bind group per chunk per slot
    bgs: Vec<Vec<wgpu::BindGroup>>,
    pass_counter: u64,
    inject: bool,
    pub total_bytes: u64,
    pub note: String,
}

impl VramTest {
    pub fn new(gpu: &Gpu, slots: &[Slot], percent: f64, inject: bool) -> Result<Self, String> {
        let dev = &gpu.device;
        let lim = &gpu.limits;

        // How much to allocate.
        let mut note = String::new();
        let mut target = match &gpu.vram {
            Some(v) => {
                let mut t = (v.heap_bytes as f64 * percent / 100.0) as u64;
                note.push_str(&format!("VRAM heap {} MiB", v.heap_bytes / MIB));
                if let Some(b) = v.budget_bytes {
                    note.push_str(&format!(", driver budget {} MiB", b / MIB));
                    let cap = b.saturating_sub(256 * MIB);
                    if t > cap {
                        t = cap;
                        note.push_str(" (capped to the driver budget: other processes hold VRAM)");
                    }
                }
                t
            }
            None => {
                note.push_str("VRAM size unknown, using buffer limits");
                (lim.max_buffer_size as f64 * percent / 100.0) as u64
            }
        };
        target = target.max(64 * MIB);

        // Chunk size: within buffer + binding limits, 1 GiB at most, and few enough chunks.
        let mut chunk = lim
            .max_buffer_size
            .min(lim.max_storage_buffer_binding_size)
            .min(1 << 30);
        chunk = chunk
            .max(target.div_ceil(MAX_CHUNKS as u64))
            .min(lim.max_buffer_size.min(lim.max_storage_buffer_binding_size));
        chunk &= !(MIB - 1);
        if chunk < MIB {
            return Err("adapter buffer limits too small for the VRAM test".into());
        }

        let layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vram"),
            entries: &[
                uniform_entry(0, true),
                storage_entry(1, false),
                storage_entry(2, false),
            ],
        });
        let src = include_str!("shaders/vram.wgsl");
        let fill = pipeline(gpu, src, "fill", &layout);
        let verify = pipeline(gpu, src, "verify", &layout);

        // Allocate chunks; stop early (and test what we got) on out-of-memory.
        let mut chunks = Vec::new();
        let mut allocated = 0u64;
        while allocated < target && chunks.len() < MAX_CHUNKS {
            let size = chunk.min(target - allocated).max(MIB) & !(MIB - 1);
            let scope = dev.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
            let buf = dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vram chunk"),
                size,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            if let Some(e) = pollster::block_on(scope.pop()) {
                note.push_str(&format!(
                    "; allocation stopped at {} MiB: {e}",
                    allocated / MIB
                ));
                break;
            }
            chunks.push((buf, size));
            allocated += size;
        }
        if chunks.is_empty() {
            return Err("could not allocate any VRAM test buffer".into());
        }
        note.push_str(&format!(
            "; testing {} MiB in {} buffer(s) of <= {} MiB",
            allocated / MIB,
            chunks.len(),
            chunk / MIB
        ));

        let params = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vram params"),
            size: slots.len() as u64 * MAX_PASSES_PER_BATCH as u64 * chunks.len() as u64 * ENTRY,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgs = chunks
            .iter()
            .map(|(buf, _)| {
                slots
                    .iter()
                    .map(|slot| {
                        dev.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("vram"),
                            layout: &layout,
                            entries: &[
                                wgpu::BindGroupEntry {
                                    binding: 0,
                                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                        buffer: &params,
                                        offset: 0,
                                        size: Some(std::num::NonZeroU64::new(16).unwrap()),
                                    }),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 1,
                                    resource: buf.as_entire_binding(),
                                },
                                wgpu::BindGroupEntry {
                                    binding: 2,
                                    resource: slot.err.as_entire_binding(),
                                },
                            ],
                        })
                    })
                    .collect()
            })
            .collect();

        Ok(VramTest {
            fill,
            verify,
            params,
            chunks,
            bgs,
            pass_counter: 0,
            inject,
            total_bytes: allocated,
            note,
        })
    }

    /// Bytes moved per pass (one full write + one full read).
    pub fn bytes_per_iter(&self) -> f64 {
        2.0 * self.total_bytes as f64
    }

    /// Encode `passes` full fill+verify passes; uniforms are written via the queue.
    pub fn encode(&mut self, gpu: &Gpu, enc: &mut wgpu::CommandEncoder, slot: usize, passes: u32) {
        let passes = passes.clamp(1, MAX_PASSES_PER_BATCH);
        let nchunks = self.chunks.len();
        let region = slot as u64 * MAX_PASSES_PER_BATCH as u64 * nchunks as u64 * ENTRY;

        let mut data = vec![0u8; passes as usize * nchunks * ENTRY as usize];
        for p in 0..passes as usize {
            let pc = self.pass_counter + p as u64;
            let pattern = (pc % N_PATTERNS) as u32;
            let seed = mix32(0x5eed_0000 ^ (pc / N_PATTERNS) as u32);
            let mut base = 0u64;
            for (c, (_, size)) in self.chunks.iter().enumerate() {
                let inject = if self.inject && c == 0 {
                    12345
                } else {
                    NO_INJECT
                };
                let vals = [(base / 16) as u32, pattern, seed, inject];
                let off = (p * nchunks + c) * ENTRY as usize;
                data[off..off + 16].copy_from_slice(bytemuck::cast_slice(&vals));
                base += size;
            }
        }
        gpu.queue.write_buffer(&self.params, region, &data);

        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("vram"),
            timestamp_writes: None,
        });
        for p in 0..passes as usize {
            for (pipe, _) in [(&self.fill, 0), (&self.verify, 1)] {
                pass.set_pipeline(pipe);
                for (c, (_, size)) in self.chunks.iter().enumerate() {
                    let off = region + ((p * nchunks + c) as u64) * ENTRY;
                    pass.set_bind_group(0, &self.bgs[c][slot], &[off as u32]);
                    let n = size / 16;
                    let wgs = (n.div_ceil(WG as u64) as u32).min(MAX_WORKGROUPS);
                    pass.dispatch_workgroups(wgs, 1, 1);
                }
            }
        }
        self.pass_counter += passes as u64;
    }
}
