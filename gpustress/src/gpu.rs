//! Device setup: Vulkan-only instance, discrete-adapter selection, VRAM heap
//! query, and non-panicking error / device-lost bookkeeping.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Errors reported asynchronously by wgpu (uncaptured errors, device loss).
#[derive(Default)]
pub struct ErrorState {
    pub lost: AtomicBool,
    pub messages: Mutex<Vec<String>>,
}

impl ErrorState {
    pub fn push(&self, msg: String) {
        if let Ok(mut m) = self.messages.lock()
            && m.len() < 16
        {
            m.push(msg);
        }
    }
    pub fn take(&self) -> Vec<String> {
        self.messages
            .lock()
            .map(|mut m| std::mem::take(&mut *m))
            .unwrap_or_default()
    }
    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }
}

pub struct VramInfo {
    /// Sum of DEVICE_LOCAL memory heaps.
    pub heap_bytes: u64,
    /// VK_EXT_memory_budget budget for those heaps, if the driver reports it.
    pub budget_bytes: Option<u64>,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub name: String,
    pub limits: wgpu::Limits,
    pub vram: Option<VramInfo>,
    pub errors: Arc<ErrorState>,
}

/// How long a single poll may block before we call it a GPU hang.
pub const HANG_TIMEOUT: Duration = Duration::from_secs(15);

fn rank(t: wgpu::DeviceType) -> Option<u8> {
    match t {
        wgpu::DeviceType::DiscreteGpu => Some(0),
        wgpu::DeviceType::IntegratedGpu => Some(1),
        wgpu::DeviceType::VirtualGpu => Some(2),
        wgpu::DeviceType::Other => Some(3),
        wgpu::DeviceType::Cpu => None, // llvmpipe/lavapipe: useless for OC testing
    }
}

pub fn init() -> Result<Gpu, String> {
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::VULKAN;
    desc.flags = wgpu::InstanceFlags::empty();
    let instance = wgpu::Instance::new(desc);

    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::VULKAN));
    let adapter = adapters
        .into_iter()
        .filter_map(|a| rank(a.get_info().device_type).map(|r| (r, a)))
        .min_by_key(|(r, _)| *r)
        .map(|(_, a)| a)
        .ok_or_else(|| "no Vulkan GPU adapter found".to_string())?;

    let info = adapter.get_info();
    let limits = adapter.limits();
    let vram = query_vram(&adapter);

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpustress"),
        required_features: wgpu::Features::empty(),
        required_limits: limits.clone(),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .map_err(|e| format!("request_device failed on {}: {e}", info.name))?;

    let errors = Arc::new(ErrorState::default());
    {
        let errs = errors.clone();
        device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
            errs.push(format!("wgpu error: {e}"));
        }));
    }
    {
        let errs = errors.clone();
        device.set_device_lost_callback(move |reason, msg| {
            if reason != wgpu::DeviceLostReason::Destroyed {
                errs.lost.store(true, Ordering::SeqCst);
                errs.push(format!("GPU device lost ({reason:?}): {msg}"));
            }
        });
    }

    Ok(Gpu {
        device,
        queue,
        name: info.name.clone(),
        limits,
        vram,
        errors,
    })
}

/// Read the Vulkan memory heaps (and VK_EXT_memory_budget, when available)
/// straight from the physical device; wgpu itself does not expose VRAM size.
fn query_vram(adapter: &wgpu::Adapter) -> Option<VramInfo> {
    use ash::vk;
    use wgpu::hal::api::Vulkan;

    // SAFETY: we only issue read-only property queries on the raw handles and
    // never destroy them.
    let hal = unsafe { adapter.as_hal::<Vulkan>() }?;
    let instance = hal.shared_instance().raw_instance();
    let pd = hal.raw_physical_device();

    let has_budget = unsafe { instance.enumerate_device_extension_properties(pd) }
        .map(|exts| {
            exts.iter()
                .any(|e| e.extension_name_as_c_str().ok() == Some(vk::EXT_MEMORY_BUDGET_NAME))
        })
        .unwrap_or(false);

    let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
    let mut props2 = vk::PhysicalDeviceMemoryProperties2::default();
    if has_budget {
        props2 = props2.push_next(&mut budget);
    }
    unsafe { instance.get_physical_device_memory_properties2(pd, &mut props2) };
    let props = props2.memory_properties;

    let (mut heap, mut bud) = (0u64, 0u64);
    for i in 0..props.memory_heap_count as usize {
        let h = props.memory_heaps[i];
        if std::env::var_os("GPUSTRESS_DEBUG").is_some() {
            eprintln!(
                "heap {i}: size {} flags {:?} budget {} usage {}",
                h.size, h.flags, budget.heap_budget[i], budget.heap_usage[i]
            );
        }
        if h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL) {
            heap += h.size;
            if has_budget {
                bud += budget.heap_budget[i];
            }
        }
    }
    if heap == 0 {
        return None;
    }
    Some(VramInfo {
        heap_bytes: heap,
        budget_bytes: (has_budget && bud > 0).then_some(bud),
    })
}

impl Gpu {
    /// Block until `idx` has completed; `Err` carries a hang/device-loss message.
    pub fn wait(&self, idx: wgpu::SubmissionIndex) -> Result<(), String> {
        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(idx),
            timeout: Some(HANG_TIMEOUT),
        }) {
            Ok(_) => {}
            Err(wgpu::PollError::Timeout) => {
                return Err(format!(
                    "GPU did not finish a batch within {}s (hang)",
                    HANG_TIMEOUT.as_secs()
                ));
            }
            Err(e) => return Err(format!("device poll failed: {e}")),
        }
        if self.errors.is_lost() {
            return Err("GPU device lost".into());
        }
        Ok(())
    }

    /// Register a map of a small MAP_READ buffer; call right after the
    /// submission that copies into it.
    pub fn start_read(&self, buf: &wgpu::Buffer) -> PendingRead {
        let done = Arc::new(Mutex::new(None));
        let d = done.clone();
        buf.map_async(wgpu::MapMode::Read, .., move |r| {
            *d.lock().unwrap() = Some(r.is_ok());
        });
        PendingRead {
            buf: buf.clone(),
            done,
        }
    }

    /// Read the mapped u32s once the owning submission has been waited on.
    pub fn finish_read(&self, p: PendingRead) -> Result<Vec<u32>, String> {
        let start = std::time::Instant::now();
        loop {
            if let Some(ok) = p.done.lock().unwrap().take() {
                if !ok {
                    return Err("result buffer map failed (device lost?)".into());
                }
                break;
            }
            if self.errors.is_lost() {
                return Err("GPU device lost".into());
            }
            if start.elapsed() > HANG_TIMEOUT {
                return Err("result readback never completed (hang)".into());
            }
            let _ = self.device.poll(wgpu::PollType::Poll);
            std::thread::sleep(Duration::from_micros(200));
        }
        let out = {
            let view = p
                .buf
                .get_mapped_range(..)
                .map_err(|e| format!("get_mapped_range failed: {e}"))?;
            bytemuck::cast_slice::<u8, u32>(&view).to_vec()
        };
        p.buf.unmap();
        Ok(out)
    }
}

pub struct PendingRead {
    buf: wgpu::Buffer,
    done: Arc<Mutex<Option<bool>>>,
}
