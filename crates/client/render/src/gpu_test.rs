//! Native GPU test helpers: a device on a chosen backend, blocking, and texture readback.
//! A test that finds no adapter prints why it skips and returns.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    /// Last, so the device is gone before the next test may make its own.
    _lock: GpuLock,
}

static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

thread_local! {
    /// How many [`GpuLock`]s this thread holds (a test may hold a renderer and a device together).
    static HELD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A test's right to a GPU device. The test harness runs tests on parallel threads and several devices created and
/// polled at once hung the run on Windows (2026-11-01), as in `tools/shader-re/tests/common/mod.rs`; so one test
/// thread owns the GPU at a time, from the adapter request until its device is dropped. Re-entrant on a thread (a
/// second device in the same test does not deadlock).
pub struct GpuLock {
    _guard: Option<std::sync::MutexGuard<'static, ()>>,
    counted: bool,
}

impl GpuLock {
    /// Waits for the GPU. A test that panicked while it held the lock only poisons it; the GPU is still usable.
    pub fn acquire() -> GpuLock {
        let nested = HELD.with(|h| {
            let n = h.get();
            h.set(n + 1);
            n > 0
        });
        GpuLock {
            _guard: (!nested).then(|| GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner())),
            counted: true,
        }
    }

    /// A lock that holds nothing (a placeholder until the real one is taken).
    pub fn none() -> GpuLock {
        GpuLock {
            _guard: None,
            counted: false,
        }
    }
}

impl Drop for GpuLock {
    fn drop(&mut self) {
        if self.counted {
            HELD.with(|h| h.set(h.get().saturating_sub(1)));
        }
        // The guard (if this one holds it) goes after, with the fields.
    }
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

/// A device on `backends` with WebGL2 limits, taking from the adapter only the features in
/// `want` (so a test can ask for BC, or leave it out to exercise the no-BC path).
/// `None`, after printing the reason, when there is no adapter.
pub fn gpu(test: &str, backends: wgpu::Backends, want: wgpu::Features) -> Option<Gpu> {
    let lock = GpuLock::acquire();
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    let instance = wgpu::Instance::new(desc);
    let adapter = match block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    })) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("SKIP {test}: no adapter for {backends:?}: {e}");
            return None;
        }
    };
    let features = adapter.features() & want;
    if features != want {
        eprintln!(
            "SKIP {test}: adapter {} lacks {:?}",
            adapter.get_info().name,
            want - features
        );
        return None;
    }
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    match block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("test device"),
        required_features: features,
        required_limits: limits,
        ..Default::default()
    })) {
        Ok((device, queue)) => Some(Gpu {
            device,
            queue,
            info: adapter.get_info(),
            _lock: lock,
        }),
        Err(e) => {
            eprintln!("SKIP {test}: device request failed: {e}");
            None
        }
    }
}

/// Texels of one mip and layer of `texture`, `bytes_per_texel` each, rows tightly packed.
/// The texture needs `COPY_SRC`.
pub fn read_texture(
    gpu: &Gpu,
    texture: &wgpu::Texture,
    mip: u32,
    layer: u32,
    size: (u32, u32),
    bytes_per_texel: u32,
) -> Vec<u8> {
    let row = size.0 * bytes_per_texel;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: mip,
            origin: wgpu::Origin3d {
                x: 0,
                y: 0,
                z: layer,
            },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = buffer.get_mapped_range(..).expect("range");
    let mut out = Vec::with_capacity((row * size.1) as usize);
    for y in 0..size.1 as usize {
        out.extend_from_slice(&data[y * padded as usize..][..row as usize]);
    }
    out
}
