//! The pages of the cache: a pool per page
//! size (64, 128, 256, 512) and the array textures that hold the pages.
//!
//! [`PagePools`] is the bookkeeping, the port of `Terrain::GetRenderTexture` and
//! `ReleaseRenderTexture`: a page goes out and comes back as a layer number; a size that has no
//! page left gives the next smaller one. The original fills the pools with all the textures at the
//! start; here a pool grows by one layer at a time up to the number its key says
//! (`Terrain64Chunks` ..), and "free" counts the layers that could still be made, so the automaton
//! of the cache sees the same numbers.
//!
//! [`PageArrays`] is the GPU side: `Rgba8Unorm` array textures of at most 256 layers with the full
//! chain of mips (the render targets of D3D9 with auto mips), made when a pool first needs the
//! layer. A diffuse page and a highlight page are two pages of the same pool: the original takes
//! both from the same free lists.

use super::settings_map::PAGE_SIZES;
use crate::perf::{Cluster, DeviceTrack, Tex};

/// Layers an array texture holds at most (`maxTextureArrayLayers` of WebGPU and WebGL2).
// re: "Страницы" (arrays of 256 layers per size)
pub const LAYERS_PER_ARRAY: u32 = 256;

/// Layers per array texture on `device`: [`LAYERS_PER_ARRAY`], or one on GL. The cell draw samples a
/// page as a plain 2D view of one layer, and the GL backend (WebGL2, native GL) has no texture views: a
/// sampled view always binds the whole texture, so a 2D view of an array texture reads nothing of the
/// page (wgpu-hal logs "non-zero array layer"; ANGLE drew the land as another texture). With one layer
/// a page array is a plain `TEXTURE_2D`.
pub fn layers_per_array_for(device: &wgpu::Device) -> u32 {
    if device.adapter_info().backend == wgpu::Backend::Gl {
        1
    } else {
        LAYERS_PER_ARRAY
    }
}

/// The format of a page (WT-D5; R5G6B5 is quantized in the shader if the capture shows it).
// SIMPLIFIED(SIMP-037): cache pages are stored uncompressed (Rgba8Unorm); the original compresses them with DXT when enabled (WT-12).
pub const PAGE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A page of the cache: its size and its layer in the pool of that size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageId {
    pub size: u32,
    pub layer: u32,
}

impl PageId {
    /// The array texture of the pool the page is in.
    pub fn array(&self) -> u32 {
        self.layer / LAYERS_PER_ARRAY
    }

    /// The layer of that array.
    pub fn slice(&self) -> u32 {
        self.layer % LAYERS_PER_ARRAY
    }
}

/// Mips of a page of `size` with the full chain down to 1 x 1.
pub fn mip_count(size: u32) -> u32 {
    size.max(1).ilog2() + 1
}

/// The pages of one size.
#[derive(Clone, Debug)]
pub struct PagePool {
    size: u32,
    limit: u32,
    created: u32,
    free: Vec<u32>,
}

impl PagePool {
    pub fn new(size: u32, limit: u32) -> Self {
        Self {
            size,
            limit,
            created: 0,
            free: Vec::new(),
        }
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    /// The most pages the pool holds.
    pub fn limit(&self) -> u32 {
        self.limit
    }

    /// Pages that were made so far.
    pub fn created(&self) -> u32 {
        self.created
    }

    /// Pages that are out.
    pub fn in_use(&self) -> u32 {
        self.created - self.free.len() as u32
    }

    /// Pages that can be had: the free ones and those that could still be made. This is the
    /// `size()` of the original's free list.
    pub fn available(&self) -> u32 {
        self.free.len() as u32 + self.limit.saturating_sub(self.created)
    }

    /// A page, the last one that came back first; `None` if the pool is out.
    // re: Terrain::GetRenderTexture (Terrain.cpp:697-730): `pPool->back(); pPool->pop_back()`
    pub fn get(&mut self) -> Option<PageId> {
        let layer = if let Some(l) = self.free.pop() {
            l
        } else if self.created < self.limit {
            self.created += 1;
            self.created - 1
        } else {
            return None;
        };
        Some(PageId {
            size: self.size,
            layer,
        })
    }

    /// Gives a page back. A page of another size or one the pool did not make is dropped.
    // re: Terrain::ReleaseRenderTexture (Terrain.cpp:775-797)
    pub fn release(&mut self, page: PageId) {
        if page.size == self.size && page.layer < self.created && !self.free.contains(&page.layer) {
            self.free.push(page.layer);
        }
    }

    /// A new limit (the key changed). Pages that are made stay.
    pub fn set_limit(&mut self, limit: u32) {
        self.limit = limit;
    }

    /// Array textures the pool needs for the pages it made.
    pub fn arrays_needed(&self) -> u32 {
        self.created.div_ceil(LAYERS_PER_ARRAY)
    }
}

/// The four pools.
#[derive(Clone, Debug)]
pub struct PagePools {
    pools: [PagePool; 4],
}

impl PagePools {
    /// The pools with the limits of `Terrain64Chunks` .. `Terrain512Chunks`.
    pub fn new(limits: [u32; 4]) -> Self {
        Self {
            pools: std::array::from_fn(|i| PagePool::new(PAGE_SIZES[i], limits[i])),
        }
    }

    fn index(size: u32) -> Option<usize> {
        PAGE_SIZES.iter().position(|&s| s == size)
    }

    /// The pool of a size.
    pub fn pool(&self, size: u32) -> Option<&PagePool> {
        Self::index(size).map(|i| &self.pools[i])
    }

    pub fn pool_mut(&mut self, size: u32) -> Option<&mut PagePool> {
        Self::index(size).map(|i| &mut self.pools[i])
    }

    /// Pages that can be had of a size; 0 for a size that has no pool.
    pub fn available(&self, size: u32) -> u32 {
        self.pool(size).map_or(0, |p| p.available())
    }

    /// A page of `size`, or of the next smaller size that has one, down to 64. The size of the
    /// page is `PageId::size`. A size that has no pool gives nothing.
    // re: Terrain::GetRenderTexture (Terrain.cpp:697-730): "without DXT a missing page falls back to
    // the next smaller size"
    pub fn get(&mut self, size: u32) -> Option<PageId> {
        let mut size = size;
        loop {
            let pool = self.pool_mut(size)?;
            if let Some(page) = pool.get() {
                return Some(page);
            }
            size >>= 1;
        }
    }

    pub fn release(&mut self, page: PageId) {
        if let Some(pool) = self.pool_mut(page.size) {
            pool.release(page);
        }
    }

    /// `CachePageAvailable`: more than one page free (the cell needs two when it swaps pages).
    // re: Terrain::CachePageAvailable (Terrain.cpp:949-962)
    pub fn page_available(&self, size: u32) -> bool {
        self.available(size) > 1
    }

    /// New limits from the keys.
    pub fn set_limits(&mut self, limits: [u32; 4]) {
        for (p, l) in self.pools.iter_mut().zip(limits) {
            p.set_limit(l);
        }
    }

    /// Pages out of every pool.
    pub fn in_use(&self) -> u32 {
        self.pools.iter().map(|p| p.in_use()).sum()
    }

    /// Bytes the pages that were made take, with the mips (a third more).
    pub fn memory_bytes(&self) -> u64 {
        self.pools
            .iter()
            .map(|p| u64::from(p.created) * page_bytes(p.size))
            .sum()
    }
}

/// Bytes of a page with the full chain of mips.
pub fn page_bytes(size: u32) -> u64 {
    (0..mip_count(size))
        .map(|m| {
            let s = u64::from((size >> m).max(1));
            s * s * 4
        })
        .sum()
}

/// The fewest bytes an array texture of pages is made with (when the limit of its pool leaves less, the
/// layers past the limit are never handed out): a pool whose limit grows a little at a time (the live
/// cache follows the land the map has built) does not end up as hundreds of small textures.
const MIN_ARRAY_BYTES: u64 = 8 << 20;

/// The array textures of the pages.
///
/// The arrays of a pool hold consecutive layers; each one is made with the layers the limit of the
/// pool leaves at that moment (at most `layers_per_array`). A limit that grows later goes on in new
/// arrays: an array already made keeps its texture, so the pages in it and the groups the draw made
/// for them stay valid.
pub struct PageArrays {
    layers_per_array: u32,
    /// Per pool: the first layer of each array and the array.
    arrays: [Vec<(u32, Tex)>; 4],
}

impl PageArrays {
    /// Arrays of `layers_per_array` layers at most (256 for the game, fewer in tests).
    pub fn new(layers_per_array: u32) -> Self {
        Self {
            layers_per_array: layers_per_array.clamp(1, LAYERS_PER_ARRAY),
            arrays: Default::default(),
        }
    }

    /// The layers the arrays of a pool hold.
    fn covered(arrays: &[(u32, Tex)]) -> u32 {
        arrays
            .last()
            .map_or(0, |(first, t)| first + t.depth_or_array_layers())
    }

    /// The array that holds a layer, and the layer in it.
    fn find(&self, page: PageId) -> Option<(&Tex, u32)> {
        let i = PagePools::index(page.size)?;
        let arrays = &self.arrays[i];
        let at = arrays.partition_point(|(first, _)| *first <= page.layer);
        let (first, t) = arrays.get(at.checked_sub(1)?)?;
        let slice = page.layer - first;
        (slice < t.depth_or_array_layers()).then_some((t, slice))
    }

    /// Makes the array textures the pools need for the pages they made. An array has
    /// `layers_per_array` layers, or the rest of the limit of the pool if that is less (but not less
    /// than [`MIN_ARRAY_BYTES`] would hold).
    pub fn ensure(&mut self, device: &wgpu::Device, pools: &PagePools) {
        for (i, &size) in PAGE_SIZES.iter().enumerate() {
            let Some(pool) = pools.pool(size) else {
                continue;
            };
            let per = self.layers_per_array;
            let least = ((MIN_ARRAY_BYTES / page_bytes(size)) as u32).clamp(1, per);
            while Self::covered(&self.arrays[i]) < pool.created() {
                let first = Self::covered(&self.arrays[i]);
                let layers = per.min(pool.limit().saturating_sub(first).max(least));
                self.arrays[i].push((
                    first,
                    device.tex(
                        &wgpu::TextureDescriptor {
                            label: Some(&format!("terrain pages {size}")),
                            size: wgpu::Extent3d {
                                width: size,
                                height: size,
                                depth_or_array_layers: layers,
                            },
                            mip_level_count: mip_count(size),
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: PAGE_FORMAT,
                            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                                | wgpu::TextureUsages::TEXTURE_BINDING
                                | wgpu::TextureUsages::COPY_SRC
                                | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        },
                        Cluster::terrain_page(size),
                    ),
                ));
            }
        }
    }

    /// The array that holds a page (its index among the arrays of the page's size) and the layer of
    /// the page in it, if the array was made: what the batched cell draw binds and reads.
    pub fn locate(&self, page: PageId) -> Option<(u32, u32)> {
        let i = PagePools::index(page.size)?;
        let arrays = &self.arrays[i];
        let at = arrays.partition_point(|(first, _)| *first <= page.layer);
        let index = at.checked_sub(1)?;
        let (first, t) = arrays.get(index)?;
        let slice = page.layer - first;
        (slice < t.depth_or_array_layers()).then_some((index as u32, slice))
    }

    /// The array texture that holds a page, if it was made.
    pub fn texture(&self, page: PageId) -> Option<&wgpu::Texture> {
        self.find(page).map(|(t, _)| &**t)
    }

    /// The layer of the page in its array texture.
    pub fn layer(&self, page: PageId) -> u32 {
        self.find(page)
            .map_or(page.layer % self.layers_per_array, |(_, slice)| slice)
    }

    /// Bytes the array textures that were made take, with their mips (`terrain.page_bytes`): the GPU
    /// memory of the pages, the layers not handed out yet included.
    pub fn memory_bytes(&self) -> u64 {
        PAGE_SIZES
            .iter()
            .zip(&self.arrays)
            .map(|(&size, arrays)| {
                arrays
                    .iter()
                    .map(|(_, t)| u64::from(t.depth_or_array_layers()) * page_bytes(size))
                    .sum::<u64>()
            })
            .sum()
    }

    /// Array textures made for a size.
    pub fn array_count(&self, size: u32) -> usize {
        PagePools::index(size).map_or(0, |i| self.arrays[i].len())
    }

    /// A view of one mip of one page, to render into.
    pub fn page_view(&self, page: PageId, mip: u32) -> Option<wgpu::TextureView> {
        let t = self.texture(page)?;
        Some(t.create_view(&wgpu::TextureViewDescriptor {
            label: Some("terrain page"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: mip,
            mip_level_count: Some(1),
            base_array_layer: self.layer(page),
            array_layer_count: Some(1),
            ..Default::default()
        }))
    }

    /// A view of the whole chain of mips of one page as a 2D texture, to sample from.
    pub fn page_view_all_mips(&self, page: PageId) -> Option<wgpu::TextureView> {
        let t = self.texture(page)?;
        Some(t.create_view(&wgpu::TextureViewDescriptor {
            label: Some("terrain page"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_mip_level: 0,
            mip_level_count: None,
            base_array_layer: self.layer(page),
            array_layer_count: Some(1),
            ..Default::default()
        }))
    }

    /// A view of a whole array texture, to sample from.
    pub fn array_view(&self, size: u32, array: usize) -> Option<wgpu::TextureView> {
        let i = PagePools::index(size)?;
        let (_, t) = self.arrays[i].get(array)?;
        Some(t.create_view(&wgpu::TextureViewDescriptor {
            label: Some("terrain page array"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu_test::gpu;

    #[test]
    fn a_page_knows_its_array_and_slice() {
        let p = PageId {
            size: 128,
            layer: 3 * 256 + 17,
        };
        assert_eq!((p.array(), p.slice()), (3, 17));
        assert_eq!(mip_count(64), 7);
        assert_eq!(mip_count(512), 10);
        // 64 x 64 x 4 and the mips: a third more.
        assert_eq!(page_bytes(64), 16_384 + 4_096 + 1_024 + 256 + 64 + 16 + 4);
    }

    #[test]
    fn a_pool_grows_to_its_limit_and_gives_the_last_page_back_first() {
        let mut p = PagePool::new(64, 3);
        assert_eq!((p.available(), p.created()), (3, 0));
        let a = p.get().unwrap();
        let b = p.get().unwrap();
        assert_eq!((a.layer, b.layer), (0, 1));
        assert_eq!((p.available(), p.in_use()), (1, 2));
        p.release(a);
        p.release(b);
        // The last one that came back is the first one out, as `back()` of the original's list.
        assert_eq!(p.get().unwrap(), b);
        assert_eq!(p.get().unwrap(), a);
        assert_eq!(p.get().unwrap().layer, 2);
        assert!(p.get().is_none());
        assert_eq!((p.available(), p.created(), p.arrays_needed()), (0, 3, 1));
    }

    #[test]
    fn a_page_goes_back_once_and_only_to_its_pool() {
        let mut p = PagePool::new(128, 4);
        let a = p.get().unwrap();
        p.release(a);
        p.release(a);
        assert_eq!(p.available(), 4);
        // Another size, or a page never made, is dropped.
        p.release(PageId { size: 64, layer: 0 });
        p.release(PageId {
            size: 128,
            layer: 3,
        });
        assert_eq!(p.available(), 4);
    }

    #[test]
    fn a_pool_that_is_out_gives_the_next_smaller_size() {
        let mut pools = PagePools::new([4, 4, 1, 1]);
        let big = pools.get(512).unwrap();
        assert_eq!(big.size, 512);
        // 512 is out: 256 answers, and when it is out too, 128, then 64.
        assert_eq!(pools.get(512).unwrap().size, 256);
        assert_eq!(pools.get(512).unwrap().size, 128);
        assert_eq!(pools.get(256).unwrap().size, 128);
        for _ in 0..2 {
            pools.get(128).unwrap();
        }
        assert_eq!(pools.get(512).unwrap().size, 64);
        // A size that is no size gives nothing.
        assert!(pools.get(100).is_none());
        // Out of everything: nothing, not an endless loop.
        while pools.get(512).is_some() {}
        assert!(pools.get(64).is_none());
        // The page comes back to its own pool.
        pools.release(big);
        assert_eq!(pools.available(512), 1);
        assert_eq!(pools.get(512).unwrap(), big);
    }

    #[test]
    fn available_counts_the_pages_that_could_still_be_made() {
        let mut pools = PagePools::new([10, 0, 2, 1]);
        assert_eq!(pools.available(64), 10);
        assert_eq!(pools.available(128), 0);
        assert!(pools.page_available(64));
        assert!(!pools.page_available(512));
        let a = pools.get(256).unwrap();
        // One page of 256 is left: more than one is needed for a swap.
        assert!(!pools.page_available(256));
        pools.release(a);
        assert!(pools.page_available(256));
        assert_eq!(pools.in_use(), 0);
        // A key that changes moves the limit; the pages that were made stay.
        pools.set_limits([2, 0, 0, 0]);
        assert_eq!(pools.available(64), 2);
        assert_eq!(pools.available(256), 1);
    }

    #[test]
    fn memory_follows_the_pages_that_were_made() {
        let mut pools = PagePools::new([100, 100, 100, 100]);
        assert_eq!(pools.memory_bytes(), 0);
        pools.get(64).unwrap();
        pools.get(512).unwrap();
        assert_eq!(pools.memory_bytes(), page_bytes(64) + page_bytes(512));
        // 512 x 512 x 4 is 1 MiB; with the mips about 1.33 MiB.
        assert!(page_bytes(512) > 1_398_000 && page_bytes(512) < 1_400_000);
    }

    #[test]
    fn the_arrays_are_made_when_a_pool_needs_them() {
        let Some(gpu) = gpu(
            "terrain pages",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut pools = PagePools::new([8, 8, 8, 8]);
        let mut arrays = PageArrays::new(4);
        arrays.ensure(&gpu.device, &pools);
        assert_eq!(arrays.array_count(64), 0);
        // Five pages of 64 need two arrays of four layers; the second has the layers that are left.
        let pages: Vec<PageId> = (0..5).map(|_| pools.get(64).unwrap()).collect();
        pools.get(512).unwrap();
        arrays.ensure(&gpu.device, &pools);
        assert_eq!(arrays.array_count(64), 2);
        assert_eq!(arrays.array_count(512), 1);
        assert_eq!(arrays.array_count(128), 0);
        let t = arrays.texture(pages[4]).unwrap();
        assert_eq!(
            (t.width(), t.height(), t.mip_level_count(), t.format()),
            (64, 64, 7, PAGE_FORMAT)
        );
        assert_eq!(t.depth_or_array_layers(), 4);
        assert_eq!(arrays.layer(pages[4]), 0);
        assert_eq!(arrays.layer(pages[3]), 3);
        assert!(arrays.page_view(pages[4], 2).is_some());
        assert!(arrays.array_view(64, 1).is_some());
        assert!(arrays.array_view(64, 2).is_none());
        // Ensuring again makes nothing new.
        arrays.ensure(&gpu.device, &pools);
        assert_eq!(arrays.array_count(64), 2);
        // The limit grows past what the arrays hold: the pages made before keep their arrays and
        // layers, the new ones go into a new array.
        let mut pools = PagePools::new([3, 0, 0, 0]);
        let mut arrays = PageArrays::new(4);
        let a: Vec<PageId> = (0..3).map(|_| pools.get(64).unwrap()).collect();
        arrays.ensure(&gpu.device, &pools);
        assert_eq!(arrays.array_count(64), 1);
        assert_eq!(arrays.layer(a[2]), 2);
        pools.set_limits([8, 0, 0, 0]);
        let b: Vec<PageId> = (0..5).map(|_| pools.get(64).unwrap()).collect();
        arrays.ensure(&gpu.device, &pools);
        assert_eq!(arrays.array_count(64), 2);
        // The first array has four layers (the least an array is made with here): page 3 is in it,
        // the next ones start the second array.
        assert_eq!(arrays.layer(b[0]), 3);
        assert!(arrays.texture(b[0]).unwrap() == arrays.texture(a[0]).unwrap());
        assert_eq!(arrays.layer(b[1]), 0);
        assert!(arrays.texture(b[1]).unwrap() != arrays.texture(a[0]).unwrap());
        assert_eq!(arrays.layer(b[4]), 3);
        assert_eq!(arrays.texture(b[4]).unwrap().depth_or_array_layers(), 4);
    }
}
