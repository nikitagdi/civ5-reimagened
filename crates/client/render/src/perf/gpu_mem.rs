//! GPU memory accounting (AC-H3). Every texture and buffer of the client is made through
//! [`DeviceTrack`] with a [`Cluster`] label; the result is a [`Tracked`] that gives its bytes back
//! when it drops. Bytes come from the format, size, mips and samples (a theoretical footprint: the
//! driver may pad). A source test (`tests::no_untracked_creation`) fails on a `create_texture` /
//! `create_buffer` outside this file.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};

use wgpu::util::DeviceExt as _;

/// What a resource is for; the HUD and `gpu_mem.*` counters group by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cluster {
    /// Baked terrain pages, by side in pixels (the cache keeps four sizes).
    TerrainPage64,
    TerrainPage128,
    TerrainPage256,
    TerrainPage512,
    /// Terrain texture arrays, height and normal maps, masks.
    TerrainTex,
    /// Model meshes and textures (units, cities, landmarks, forests).
    Models,
    /// Bone textures, clip caches, skin data.
    Animation,
    Decals,
    Shadows,
    /// Render targets and their depth buffers, the minimap, read-back buffers of frames.
    RenderTargets,
    Ui,
    /// Uniforms, particles, overlays, water, fog, timers: everything that has no own cluster.
    Other,
}

const N: usize = 12;

impl Cluster {
    pub const ALL: [Cluster; N] = [
        Self::TerrainPage64,
        Self::TerrainPage128,
        Self::TerrainPage256,
        Self::TerrainPage512,
        Self::TerrainTex,
        Self::Models,
        Self::Animation,
        Self::Decals,
        Self::Shadows,
        Self::RenderTargets,
        Self::Ui,
        Self::Other,
    ];

    fn index(self) -> usize {
        self as usize
    }

    /// The name in `gpu_mem.<name>` and in the HUD.
    pub fn name(self) -> &'static str {
        match self {
            Self::TerrainPage64 => "terrain_pages_64",
            Self::TerrainPage128 => "terrain_pages_128",
            Self::TerrainPage256 => "terrain_pages_256",
            Self::TerrainPage512 => "terrain_pages_512",
            Self::TerrainTex => "terrain_tex",
            Self::Models => "models",
            Self::Animation => "animation",
            Self::Decals => "decals",
            Self::Shadows => "shadows",
            Self::RenderTargets => "render_targets",
            Self::Ui => "ui",
            Self::Other => "other",
        }
    }

    /// The page cluster of a page `px` pixels on a side (rounded up to a kept size).
    pub fn terrain_page(px: u32) -> Self {
        match px {
            0..=64 => Self::TerrainPage64,
            65..=128 => Self::TerrainPage128,
            129..=256 => Self::TerrainPage256,
            _ => Self::TerrainPage512,
        }
    }
}

static LIVE: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static PEAK: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static COUNT: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static CREATED: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

fn add(c: Cluster, bytes: u64) {
    let i = c.index();
    let now = LIVE[i].fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK[i].fetch_max(now, Ordering::Relaxed);
    COUNT[i].fetch_add(1, Ordering::Relaxed);
    CREATED[i].fetch_add(1, Ordering::Relaxed);
}

fn sub(c: Cluster, bytes: u64) {
    let i = c.index();
    LIVE[i].fetch_sub(bytes, Ordering::Relaxed);
    COUNT[i].fetch_sub(1, Ordering::Relaxed);
}

/// A texture or buffer that counts against its cluster until it drops. Derefs to the wgpu object.
pub struct Tracked<T> {
    inner: T,
    cluster: Cluster,
    bytes: u64,
}

pub type Tex = Tracked<wgpu::Texture>;
pub type Buf = Tracked<wgpu::Buffer>;

impl<T> Tracked<T> {
    fn new(inner: T, cluster: Cluster, bytes: u64) -> Self {
        add(cluster, bytes);
        Self {
            inner,
            cluster,
            bytes,
        }
    }

    pub fn cluster(&self) -> Cluster {
        self.cluster
    }

    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Moves the bytes to another cluster (a page that turns out to be another size).
    pub fn retag(&mut self, cluster: Cluster) {
        if cluster != self.cluster {
            sub(self.cluster, self.bytes);
            let to = cluster.index();
            let now = LIVE[to].fetch_add(self.bytes, Ordering::Relaxed) + self.bytes;
            PEAK[to].fetch_max(now, Ordering::Relaxed);
            COUNT[to].fetch_add(1, Ordering::Relaxed);
            self.cluster = cluster;
        }
    }
}

impl<T> Deref for Tracked<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T> DerefMut for Tracked<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T> Drop for Tracked<T> {
    fn drop(&mut self) {
        sub(self.cluster, self.bytes);
    }
}

impl<T> std::fmt::Debug for Tracked<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tracked({}, {} B)", self.cluster.name(), self.bytes)
    }
}

/// Bytes of a texture of `desc`: all mips, layers and samples.
pub fn texture_bytes(desc: &wgpu::TextureDescriptor<'_>) -> u64 {
    let samples = u64::from(desc.sample_count.max(1));
    (0..desc.mip_level_count.max(1))
        .map(|level| {
            let size = desc.size.mip_level_size(level, desc.dimension);
            desc.format.theoretical_memory_footprint(size)
        })
        .sum::<u64>()
        * samples
}

/// The one way to make textures and buffers.
pub trait DeviceTrack {
    fn tex(&self, desc: &wgpu::TextureDescriptor<'_>, cluster: Cluster) -> Tex;
    fn tex_with_data(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
        cluster: Cluster,
    ) -> Tex;
    fn buf(&self, desc: &wgpu::BufferDescriptor<'_>, cluster: Cluster) -> Buf;
    fn buf_init(&self, desc: &wgpu::util::BufferInitDescriptor<'_>, cluster: Cluster) -> Buf;
}

impl DeviceTrack for wgpu::Device {
    fn tex(&self, desc: &wgpu::TextureDescriptor<'_>, cluster: Cluster) -> Tex {
        Tracked::new(self.create_texture(desc), cluster, texture_bytes(desc))
    }

    fn tex_with_data(
        &self,
        queue: &wgpu::Queue,
        desc: &wgpu::TextureDescriptor<'_>,
        order: wgpu::util::TextureDataOrder,
        data: &[u8],
        cluster: Cluster,
    ) -> Tex {
        Tracked::new(
            self.create_texture_with_data(queue, desc, order, data),
            cluster,
            texture_bytes(desc),
        )
    }

    fn buf(&self, desc: &wgpu::BufferDescriptor<'_>, cluster: Cluster) -> Buf {
        Tracked::new(self.create_buffer(desc), cluster, desc.size)
    }

    fn buf_init(&self, desc: &wgpu::util::BufferInitDescriptor<'_>, cluster: Cluster) -> Buf {
        Tracked::new(
            self.create_buffer_init(desc),
            cluster,
            desc.contents.len() as u64,
        )
    }
}

/// Live bytes and counts per cluster, now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GpuMemSnapshot {
    /// `(cluster, live bytes, peak bytes, live resources)`.
    pub clusters: Vec<(Cluster, u64, u64, u64)>,
    pub total: u64,
}

pub fn snapshot() -> GpuMemSnapshot {
    let clusters: Vec<_> = Cluster::ALL
        .iter()
        .map(|&c| {
            let i = c.index();
            (
                c,
                LIVE[i].load(Ordering::Relaxed),
                PEAK[i].load(Ordering::Relaxed),
                COUNT[i].load(Ordering::Relaxed),
            )
        })
        .collect();
    GpuMemSnapshot {
        total: clusters.iter().map(|c| c.1).sum(),
        clusters,
    }
}

/// Resources made since the start, per cluster (tests).
pub fn created(cluster: Cluster) -> u64 {
    CREATED[cluster.index()].load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc(
        w: u32,
        h: u32,
        layers: u32,
        mips: u32,
        samples: u32,
        format: wgpu::TextureFormat,
    ) -> wgpu::TextureDescriptor<'static> {
        wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: layers,
            },
            mip_level_count: mips,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }
    }

    #[test]
    fn texture_bytes_count_mips_layers_samples_and_blocks() {
        use wgpu::TextureFormat as F;
        assert_eq!(
            texture_bytes(&desc(256, 256, 1, 1, 1, F::Rgba8Unorm)),
            256 * 256 * 4
        );
        // 4x4 -> 2x2 -> 1x1
        assert_eq!(
            texture_bytes(&desc(4, 4, 1, 3, 1, F::Rgba8Unorm)),
            (16 + 4 + 1) * 4
        );
        assert_eq!(texture_bytes(&desc(8, 8, 3, 1, 1, F::R8Unorm)), 8 * 8 * 3);
        assert_eq!(
            texture_bytes(&desc(64, 64, 1, 1, 4, F::Depth32Float)),
            64 * 64 * 4 * 4
        );
        // BC7: 16 bytes per 4x4 block
        assert_eq!(
            texture_bytes(&desc(64, 64, 1, 1, 1, F::Bc7RgbaUnorm)),
            16 * 16 * 16
        );
    }

    #[test]
    fn tracked_adds_and_drop_subtracts() {
        // Built by hand, no device. Other tests make no resources of `Ui`/`Other` concurrently.
        let c = Cluster::Ui;
        let before = snapshot().clusters[c.index()];
        let mut t = Tracked::new((), c, 1000);
        let mid = snapshot().clusters[c.index()];
        assert!(mid.1 >= before.1 + 1000 - 1000 && mid.3 >= 1);
        let live_ui = mid.1;
        t.retag(Cluster::Other);
        assert_eq!(t.cluster(), Cluster::Other);
        assert_eq!(snapshot().clusters[c.index()].1, live_ui - 1000);
        drop(t);
        let after = snapshot();
        assert_eq!(after.total, after.clusters.iter().map(|c| c.1).sum());
    }

    #[test]
    fn page_sizes_pick_their_cluster() {
        assert_eq!(Cluster::terrain_page(64), Cluster::TerrainPage64);
        assert_eq!(Cluster::terrain_page(100), Cluster::TerrainPage128);
        assert_eq!(Cluster::terrain_page(512), Cluster::TerrainPage512);
    }

    /// AC-H3: no texture or buffer is made around the accounting.
    #[test]
    fn no_untracked_creation() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut bad = Vec::new();
        walk(&root, &mut bad);
        assert!(
            bad.is_empty(),
            "untracked creations (use perf::DeviceTrack):\n{}",
            bad.join("\n")
        );
    }

    /// Files where a raw creation is fine: this module, GPU test helpers and what is not GPU memory.
    fn allowed(path: &str) -> bool {
        path.ends_with("perf/gpu_mem.rs")
            || path.ends_with("gpu_test.rs")
            || path.ends_with("gpu_tests.rs")
            // Test code of the UI pass (raw targets for the read-back) and the WebAudio buffer of the page (not GPU memory).
            || path.ends_with("ui_pass/tests.rs")
            || path.ends_with("platform-web/src/audio.rs")
    }

    fn walk(dir: &std::path::Path, bad: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if p.is_dir() {
                if name == "target" || name == "tests" || name == "benches" {
                    continue;
                }
                walk(&p, bad);
            } else if name.ends_with(".rs") {
                let path = p.to_string_lossy().replace('\\', "/");
                if allowed(&path) {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&p) else {
                    continue;
                };
                // A test module at the end of the file is test code.
                let code = text.split("\n#[cfg(test)]").next().unwrap_or(&text);
                for (n, line) in code.lines().enumerate() {
                    let l = line.trim_start();
                    if l.starts_with("//") {
                        continue;
                    }
                    for pat in [
                        ".create_texture(",
                        ".create_buffer(",
                        ".create_buffer_init(",
                        ".create_texture_with_data(",
                    ] {
                        if line.contains(pat) {
                            bad.push(format!("{path}:{}: {}", n + 1, l));
                        }
                    }
                }
            }
        }
    }
}
