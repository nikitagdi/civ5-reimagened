//! The cache of cells: which cell has which page,
//! which cell is baked next, who gives up a page when the pool is empty. A port of the automaton
//! of `Terrain::LaunchJobs`, `Terrain::FreeTiles`, `TerrainCell::Build`,
//! `TerrainCell::RequestCacheTexture`, `RequestUncompressedTextures` and
//! `Terrain::NonBlockingTextureInit`, without threads: the work goes in a list of [`BakeJob`] that
//! the bake (R2.9b) takes and reports done, a frame at a time.
//!
//! States of a cell (spec table 3.1): 0 none, 1 cache only, 2 rebuild, 3 full, 4 mesh only,
//! 5 mesh and cache. Here: [`CellState`].
//!
//! Open (RW-5/RW-6): `g_iCacheSize` (the size of the cache pages; the argument of [`TerrainCache::new`]),
//! `g_iMaxTileSize`, `g_bBlockOnLoad`, and what `SortLRU` compares exactly. The order of the
//! eviction here is "the page unused for longest first, the near page before the cache page".

use std::collections::VecDeque;

use super::cells::{StatsClass, TerrainCell, page_size_for_width};
use super::pages::{PageId, PagePools};
use super::settings_map::TerrainSettings;

/// Attempts a request makes to get a page, freeing some after each.
// re: TerrainCell::RequestCacheTexture (TerrainCell.cpp:1468-1510: up to 20 attempts)
pub const MAX_ATTEMPTS: u32 = 20;
/// Pages freed after a failed attempt.
// re: TerrainCell::RequestCacheTexture (TerrainCell.cpp:1468-1510: a free of 16)
pub const FREE_PER_ATTEMPT: usize = 16;
/// Cells the loader of the first frame takes per call.
// re: Terrain::NonBlockingTextureInit (Terrain.cpp:1310)
pub const INIT_CELLS_PER_CALL: usize = 4;

/// The state of the automaton of a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CellState {
    /// 0: nothing.
    #[default]
    None = 0,
    /// 1: the cache page only.
    CacheOnly = 1,
    /// 2: the pages exist and are baked again (the cell was invalidated).
    Rebuild = 2,
    /// 3: the cache page and the near page (compressed path).
    Full = 3,
    /// 4: only the mesh: no page could be had.
    MeshOnly = 4,
    /// 5: the mesh and the cache page, the near page too when textures are not compressed.
    MeshAndCache = 5,
}

/// Which page of the cell a job bakes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageKind {
    /// The cache page of `g_iCacheSize` (or larger).
    Cache,
    /// The near ("uncompressed") page for the cells close to the camera.
    Near,
}

/// A page to bake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BakeJob {
    pub cell: usize,
    pub kind: PageKind,
    pub page: PageId,
    /// The highlight page that goes with the page (a cell with a highlight), baked in the same job.
    pub spec: Option<PageId>,
}

/// What the view says about a cell this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellView {
    pub cell: usize,
    /// The width of the cell on the screen in pixels (`CalcMaxWidth`).
    pub width_px: f32,
    /// How far the cell is from where the view looks next (the middle of the screen moved ahead along
    /// the motion of the camera), in clip units: the jobs of the nearest cells are baked first.
    pub dist: f32,
    /// Just outside the view (the ring the camera scrolls into next): the cell gets its cache page
    /// before it is seen, and no near page.
    pub ring: bool,
}

/// Why the pages of a cell are baked again (the counters `terrain.rebake.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rebake {
    /// The field under the cell was built again (a new basis record, a neighbour built).
    Field,
    /// A decal over the cell was made or taken away.
    Decal,
    /// The pages were baked with another setting (`[DisableSystems] Decal`).
    Settings,
}

/// What the cache did, for the counters (`terrain.*`, AC-H8); every number grows from the start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheCounters {
    /// Pages taken from a cell for another one, by size (64, 128, 256, 512).
    pub evicted: [u64; 4],
    /// Near pages given back while the cell keeps its cache page.
    pub demoted: u64,
    /// Pages that were drawn and were baked again.
    pub rebaked: u64,
    /// Cells asked to bake again, by [`Rebake`] (field, decal, settings).
    pub rebake_cause: [u64; 3],
    /// Jobs of the last launch whose cell is in view.
    pub jobs_visible: usize,
    /// Highlight pages of the base layer taken from cells out of view for cells in view, and given
    /// back to a cell in view (its page baked again with the highlight): `terrain.spec.*`.
    pub spec_dropped: u64,
    pub spec_restored: u64,
    /// Base pages baked in the background, for cells out of view (`terrain.background.paged_total`).
    pub background_baked: u64,
    /// Near pages of cells in view swapped for a page of the size their width now asks for
    /// ([`TerrainCache::resize_near`]).
    pub resized: u64,
}

fn size_slot(size: u32) -> usize {
    super::settings_map::PAGE_SIZES
        .iter()
        .position(|&s| s == size)
        .unwrap_or(0)
}

/// What the cache keeps of one cell.
#[derive(Clone, Debug, Default)]
pub struct CacheEntry {
    pub state: CellState,
    pub cache_page: Option<PageId>,
    pub near_page: Option<PageId>,
    /// The highlight pages that go with them, for a cell with a highlight (`TerrainCell::has_spec`): a
    /// second page of the same size from the same pool, as the original takes both from one free list.
    pub cache_spec: Option<PageId>,
    pub near_spec: Option<PageId>,
    /// The highlight page of the cache page is baked: a cell that gets its highlight page back while
    /// its cache page is drawn is drawn without the highlight until the job is done.
    pub cache_spec_ready: bool,
    /// The highlight page of the cache page was given to a cell in view: the cell gets one back when it
    /// is seen.
    spec_lent: bool,
    /// The cache page was asked for in the background (the cell out of view).
    background: bool,
    /// The cache page is baked (it is drawn).
    pub cache_ready: bool,
    pub near_ready: bool,
    /// The page is baked but its content is older than the cell: it is drawn until the job that
    /// bakes it again is done (the job writes the page whole in one frame, before the draw reads it).
    pub cache_stale: bool,
    pub near_stale: bool,
    /// The near page of the size the cell had before it was resized in view, and its highlight page
    /// ([`TerrainCache::resize_near`]): drawn until the job of the new one is done, then given back.
    near_old: Option<(PageId, Option<PageId>)>,
    /// A job for each page is out and not reported.
    cache_out: bool,
    near_out: bool,
    /// Frame of the last time the cell was wanted.
    last_used: u64,
}

impl CacheEntry {
    /// The cell can be drawn from its page.
    pub fn is_drawable(&self) -> bool {
        self.cache_ready || self.near_ready
    }

    pub fn has_pending(&self) -> bool {
        self.cache_out || self.near_out
    }

    /// The page the cell is drawn from and its highlight page: the near page once it is baked, the near
    /// page of the old size while the new one bakes, else the cache page once it is baked (its highlight
    /// page once that is baked too).
    pub fn drawn_page(&self) -> Option<(PageId, Option<PageId>)> {
        if self.near_ready {
            self.near_page.map(|p| (p, self.near_spec))
        } else if let Some(old) = self.near_old {
            Some(old)
        } else if self.cache_ready {
            self.cache_page
                .map(|p| (p, self.cache_spec.filter(|_| self.cache_spec_ready)))
        } else {
            None
        }
    }
}

/// The cache of all cells.
pub struct TerrainCache {
    pools: PagePools,
    entries: Vec<CacheEntry>,
    cache_size: u32,
    /// The textures are not compressed (WebGL2 and the default of this port): a cell near the
    /// camera has the cache page and the near page.
    non_dxt: bool,
    frame: u64,
    /// The cells the loader has looked at.
    init_cursor: usize,
    jobs: VecDeque<BakeJob>,
    /// Pages freed since the start: for the tests and the log.
    evicted: u64,
    /// The cells that want a highlight page (`TerrainCell::has_spec` as the last launch saw it).
    wants_spec: Vec<bool>,
    /// The distance of each cell in view from where the view looks next ([`CellView::dist`]), as the
    /// last launch saw it; the order of the jobs.
    dist: Vec<f32>,
    counters: CacheCounters,
    /// The limits of the keys (`Terrain64Chunks` ..).
    floors: [u32; 4],
    /// The pools follow the map and the view (the live cache); `None`: the fixed limits of the keys.
    auto: Option<AutoPools>,
    /// Highlight pages of the size of the cache pages that cells hold.
    cache_specs: u32,
    /// The biggest page a cell may ask for (the terrain detail): a cell that wants more gets this one.
    max_page: u32,
    /// The most memory the pages take when the pools follow the map (the terrain detail).
    budget: u64,
}

/// The most GPU memory the pages of the cache take when the pools follow the map (B3, the user's
/// decision of 2026-10-29: the pools scale with the map, under a hard ceiling of 2 GB).
pub const POOL_BUDGET_BYTES: u64 = 2 << 30;

/// Views of near pages a near pool holds ([`AutoPools`]): the view and the cells it has just left.
const NEAR_VIEWS: u32 = 2;

/// What the pools are sized from when they follow the map ([`TerrainCache::set_auto_size`]).
///
/// - The pool of the cache pages (64) has room for the whole base layer of the map: one page for every
///   candidate cell, and a small margin. A base page is never taken back (no cell out of view loses
///   its page and is baked again when it is seen), on every map size; the key `Terrain64Chunks` was
///   sized for a standard map and a huge one has more land than that (B3: 4096 pages full, the loader
///   and the view took each other's pages for ever). The pages are made as the cells are seen (in
///   view or in the ring around it), not for the whole map at the start.
/// - The highlight pages of the base layer (a second page for most cells: on a huge map all open,
///   20 000 pages, 440 MB) are for the view: twice the most the view and its ring have wanted in a
///   frame, and 64. A cell out of view gives its highlight page to a cell in view when they run out
///   (drawn without it, it is out of view); a cell that comes back into view gets one again and its
///   page is baked again with it.
/// - The near pools (128, 256, 512) hold what the view asks for: [`NEAR_VIEWS`] times the most near
///   pages of each size the cells in view have wanted in a frame, and a quarter more; never less than
///   the key.
///
/// With the share of one view (and a half for the highlights) a pan at the far zoom of a huge map took
/// the pages of the cells it had just left for the ones it came to, and baked them again on the way
/// back: a thousand near pages and a thousand highlights in a few seconds, TerrainBake 10 ms a frame.
/// The view and the ring around it (a view half again as wide, and ahead of the camera) are about
/// twice the cells of the view.
///
/// The pools are made a page at a time as they are needed, so a limit is a ceiling, not memory taken.
#[derive(Clone, Copy, Debug)]
struct AutoPools {
    floors: [u32; 4],
    /// Candidate cells of the map, and of them the ones with a highlight.
    base_need: u32,
    spec_cells: u32,
    /// The most highlight pages of the cache size the view and its ring have wanted in a frame.
    spec_view: u32,
    near_need: [u32; 4],
}

impl AutoPools {
    /// Highlight pages of the size of the cache pages the cells may hold.
    fn spec_budget(&self) -> u32 {
        (2 * self.spec_view + 64).min(self.spec_cells)
    }
}

/// The limits of [`AutoPools`]: the base pool first, the near pools in what is left of `budget`
/// (bytes). A pool of a size above `max_page` has no pages at all.
fn auto_limits(auto: &AutoPools, cache_size: u32, budget: u64, max_page: u32) -> [u32; 4] {
    use super::pages::page_bytes;
    let sizes = super::settings_map::PAGE_SIZES;
    let base_slot = size_slot(cache_size);
    let mut limits = [0u32; 4];
    let base = auto.base_need + (auto.base_need / 16).max(32) + auto.spec_budget();
    let base = base.next_multiple_of(32);
    let base_cap = (budget / page_bytes(sizes[base_slot])) as u32;
    limits[base_slot] = base.min(base_cap);
    let left = budget - u64::from(limits[base_slot]) * page_bytes(sizes[base_slot]);
    let mut near_bytes = 0u64;
    for i in 0..4 {
        if i == base_slot {
            continue;
        }
        if sizes[i] > max_page {
            continue;
        }
        if sizes[i] < cache_size {
            limits[i] = auto.floors[i];
        } else {
            let want = NEAR_VIEWS * auto.near_need[i] + auto.near_need[i] / 4 + 8;
            limits[i] = want.max(auto.floors[i]);
        }
        near_bytes += u64::from(limits[i]) * page_bytes(sizes[i]);
    }
    if near_bytes > left {
        // Over the ceiling: the near pools give up the same share each.
        let keep = left as f64 / near_bytes as f64;
        for (i, l) in limits.iter_mut().enumerate() {
            if i != base_slot {
                *l = (f64::from(*l) * keep).floor() as u32;
            }
        }
    }
    limits
}

impl TerrainCache {
    /// A cache for `cells` cells, with the limits of the pools from the keys.
    pub fn new(cells: usize, settings: &TerrainSettings, cache_size: u32, non_dxt: bool) -> Self {
        Self {
            floors: limits_of(settings),
            auto: None,
            pools: PagePools::new(limits_of(settings)),
            entries: vec![CacheEntry::default(); cells],
            cache_size: cache_size.max(64),
            non_dxt,
            frame: 0,
            init_cursor: 0,
            jobs: VecDeque::new(),
            evicted: 0,
            wants_spec: vec![false; cells],
            dist: vec![f32::MAX; cells],
            counters: CacheCounters::default(),
            cache_specs: 0,
            max_page: settings.max_page().max(cache_size),
            budget: settings.pool_budget_bytes(),
        }
    }

    /// The size of the page a cell of this width on the screen gets: what it asks for, within the
    /// terrain detail ([`Self::max_page`]) and not below the cache pages.
    fn page_size(&self, width_px: f32) -> u32 {
        page_size_for_width(width_px, self.cache_size)
            .min(self.max_page)
            .max(self.cache_size)
    }

    /// The biggest page a cell may get (the terrain detail).
    pub fn max_page(&self) -> u32 {
        self.max_page
    }

    /// The most memory the pages may take when the pools follow the map (the terrain detail).
    pub fn budget_bytes(&self) -> u64 {
        self.budget
    }

    pub fn counters(&self) -> &CacheCounters {
        &self.counters
    }

    /// The cell is in the view of the last launch.
    pub fn in_view(&self, cell: usize) -> bool {
        self.entries[cell].last_used == self.frame && self.frame > 0
    }

    /// Takes which cells want a highlight page from the cells (before their pages are asked for), and,
    /// when the pools follow the map, the pages of the base layer the map needs.
    fn see_spec(&mut self, cells: &[TerrainCell]) {
        self.wants_spec.resize(cells.len(), false);
        let (mut base, mut spec) = (0u32, 0u32);
        for (w, c) in self.wants_spec.iter_mut().zip(cells) {
            *w = c.has_spec;
            if Self::is_candidate(c) {
                base += 1;
                spec += u32::from(c.has_spec);
            }
        }
        if let Some(auto) = self.auto.as_mut()
            && (base > auto.base_need || spec > auto.spec_cells)
        {
            auto.base_need = auto.base_need.max(base);
            auto.spec_cells = auto.spec_cells.max(spec);
            self.fit_limits();
        }
    }

    /// The pools follow the map and the view ([`AutoPools`]) instead of the fixed limits of the keys.
    /// The base layer is then made as the cells are seen: the loader of the first frame
    /// ([`Self::init_step`]) serves nothing, and the cells without a page out of view are no work.
    pub fn set_auto_size(&mut self, on: bool) {
        self.auto = on.then_some(AutoPools {
            floors: self.floors,
            base_need: 0,
            spec_cells: 0,
            spec_view: 0,
            near_need: [0; 4],
        });
        if on {
            self.fit_limits();
        } else {
            self.pools.set_limits(self.floors);
        }
    }

    /// The limits of the pools in force (`terrain.pool_limit.*`).
    pub fn limits(&self) -> [u32; 4] {
        std::array::from_fn(|i| {
            self.pools
                .pool(super::settings_map::PAGE_SIZES[i])
                .map_or(0, |p| p.limit())
        })
    }

    /// The limits of the pools from what the map and the view need, within [`POOL_BUDGET_BYTES`].
    fn fit_limits(&mut self) {
        let Some(auto) = self.auto.as_ref() else {
            return;
        };
        let limits = auto_limits(auto, self.cache_size, self.budget, self.max_page);
        // A limit is never taken below the pages the pool has made (they stay; the bookkeeping of the
        // free pages counts from it).
        let made: [u32; 4] = std::array::from_fn(|i| {
            self.pools
                .pool(super::settings_map::PAGE_SIZES[i])
                .map_or(0, |p| p.created())
        });
        self.pools
            .set_limits(std::array::from_fn(|i| limits[i].max(made[i])));
    }

    /// The near pages the cells in view ask for this frame, by size: the near pools grow to hold them.
    fn see_view_need(&mut self, cells: &[TerrainCell], view: &[CellView]) {
        if self.auto.is_none() {
            return;
        }
        let mut need = [0u32; 4];
        let mut spec_view = 0u32;
        for v in view {
            if !Self::is_candidate(&cells[v.cell]) {
                continue;
            }
            spec_view += u32::from(cells[v.cell].has_spec);
            if v.ring {
                continue;
            }
            let size = self.page_size(v.width_px);
            if size > self.cache_size {
                need[size_slot(size)] += 1 + u32::from(cells[v.cell].has_spec);
            }
        }
        let auto = self.auto.as_mut().expect("checked");
        let mut grew = spec_view > auto.spec_view;
        auto.spec_view = auto.spec_view.max(spec_view);
        for (have, want) in auto.near_need.iter_mut().zip(need) {
            if want > *have {
                *have = want;
                grew = true;
            }
        }
        if grew {
            self.fit_limits();
        }
    }

    /// The highlight page for a cell that wants one: a page of exactly the size of its page, if the
    /// pool has one free (no eviction for it; without it the cell is drawn without the highlight).
    /// When the pools follow the map, a highlight page of the cache size comes from the share of the
    /// view ([`AutoPools::spec_budget`]): past it, the cell out of view that was seen longest ago gives
    /// its own.
    fn spec_page_for(&mut self, cell: usize, page: PageId) -> Option<PageId> {
        if !self.wants_spec.get(cell).copied().unwrap_or(false) {
            return None;
        }
        let base = page.size == self.cache_size;
        if base
            && let Some(budget) = self.auto.as_ref().map(AutoPools::spec_budget)
            && self.cache_specs >= budget
            && !self.drop_spec(cell)
        {
            return None;
        }
        let got = self.pools.pool_mut(page.size)?.get();
        if base && got.is_some() {
            self.cache_specs += 1;
        }
        got
    }

    /// The highlight page of the cache page of the cell out of view seen longest ago goes back to the
    /// pool; the cell is drawn without it (it is out of view) and gets one again when it is seen.
    fn drop_spec(&mut self, keep: usize) -> bool {
        let frame = self.frame;
        let victim = self
            .entries
            .iter()
            .enumerate()
            .filter(|(i, e)| {
                *i != keep && e.cache_spec.is_some() && !e.cache_out && e.last_used < frame
            })
            .min_by_key(|(i, e)| (e.last_used, *i))
            .map(|(i, _)| i);
        let Some(i) = victim else {
            return false;
        };
        self.release_cache_spec(i);
        self.entries[i].spec_lent = true;
        self.counters.spec_dropped += 1;
        true
    }

    fn release_cache_spec(&mut self, cell: usize) {
        let e = &mut self.entries[cell];
        if let Some(s) = e.cache_spec.take() {
            self.pools.release(s);
            self.cache_specs = self.cache_specs.saturating_sub(1);
        }
        e.cache_spec_ready = false;
    }

    /// A cell in view whose cache page lost its highlight page gets one again, and its page is baked
    /// again with it (the page stays drawn, without the highlight, until the job is done). Returns
    /// whether a job was made.
    fn restore_spec(&mut self, cell: usize) -> bool {
        let e = &self.entries[cell];
        let Some(page) = e.cache_page else {
            return false;
        };
        if self.auto.is_none()
            || !e.spec_lent
            || e.cache_spec.is_some()
            || e.cache_out
            || !self.wants_spec.get(cell).copied().unwrap_or(false)
        {
            return false;
        }
        let Some(spec) = self.spec_page_for(cell, page) else {
            return false;
        };
        let e = &mut self.entries[cell];
        e.cache_spec = Some(spec);
        e.cache_spec_ready = false;
        e.spec_lent = false;
        e.cache_stale = e.cache_ready;
        e.state = CellState::Rebuild;
        self.counters.spec_restored += 1;
        self.push_job(cell, PageKind::Cache, page);
        true
    }

    /// The base page of a cell out of view, baked in the background (after the jobs of the view) when
    /// the pool has one free: no page is taken from another cell for it. Its highlight page only when
    /// the share of the view has one free; otherwise the cell gets it when it is seen. Returns whether a
    /// job was made.
    pub fn request_background(&mut self, cell: usize) -> bool {
        let e = &self.entries[cell];
        if e.cache_page.is_some() || e.cache_out || !self.pools.page_available(self.cache_size) {
            return false;
        }
        let Some(page) = self.pools.get(self.cache_size) else {
            return false;
        };
        let wants = self.wants_spec.get(cell).copied().unwrap_or(false);
        let room = self
            .auto
            .as_ref()
            .is_none_or(|a| self.cache_specs < a.spec_budget());
        let spec = if wants && room {
            self.spec_page_for(cell, page)
        } else {
            None
        };
        let e = &mut self.entries[cell];
        e.cache_page = Some(page);
        e.cache_spec = spec;
        e.spec_lent = wants && spec.is_none();
        e.background = true;
        e.state = CellState::CacheOnly;
        self.push_job(cell, PageKind::Cache, page);
        true
    }

    /// Jobs of cells out of the last view that wait (the background ones among them).
    pub fn jobs_out_of_view(&self) -> usize {
        self.jobs
            .iter()
            .filter(|j| self.entries[j.cell].last_used != self.frame)
            .count()
    }

    /// Highlight pages of the cache size the cells may hold when the pools follow the map
    /// (`terrain.spec.budget`).
    pub fn spec_budget(&self) -> Option<u32> {
        self.auto.as_ref().map(AutoPools::spec_budget)
    }

    /// Highlight pages of the cache size the cells hold (`terrain.spec.pages`).
    pub fn cache_specs(&self) -> u32 {
        self.cache_specs
    }

    pub fn pools(&self) -> &PagePools {
        &self.pools
    }

    pub fn pools_mut(&mut self) -> &mut PagePools {
        &mut self.pools
    }

    pub fn entry(&self, cell: usize) -> &CacheEntry {
        &self.entries[cell]
    }

    pub fn cache_size(&self) -> u32 {
        self.cache_size
    }

    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// A key changed: the pools get their new limits (pages above them are not taken back).
    pub fn set_settings(&mut self, settings: &TerrainSettings) {
        self.floors = limits_of(settings);
        if let Some(auto) = self.auto.as_mut() {
            auto.floors = self.floors;
            self.fit_limits();
        } else {
            self.pools.set_limits(self.floors);
        }
    }

    /// Whether the original draws the cell: land, with an open hex.
    // re: Terrain::LaunchJobs (Terrain.cpp:976-985)
    fn is_candidate(cell: &TerrainCell) -> bool {
        cell.stats.class == StatsClass::Land && cell.is_open()
    }

    /// The cells the first frame waits for: candidates without the cache page.
    pub fn init_wanted(&self, cells: &[TerrainCell]) -> usize {
        cells.iter().filter(|c| Self::is_candidate(c)).count()
    }

    fn page_for(&mut self, size: u32) -> Option<PageId> {
        for _ in 0..MAX_ATTEMPTS {
            if self.pools.page_available(size) {
                return self.pools.get(size);
            }
            // One base page at a time: each one is a cell to bake again when it is seen.
            if self.free_tiles(size, 1, None) == 0 {
                break;
            }
        }
        self.pools.get(size)
    }

    /// Frees pages of `size` of the cells out of view, unused for longest. Only the pool of that size
    /// gains from it, so only its pages go: for a near size the near page is given back and the cell
    /// keeps its cache page (`demote`); for the size of the cache pages the cache page of a cell out of
    /// view goes (the base layer gives way only when its own pool is too small for the land). Returns
    /// the number freed. A cell in `keep` is not touched.
    // re: Terrain::FreeTiles (Terrain.cpp:888-963): the original frees any page, the near ones first; a
    // page of another size than the one asked for freed nothing the request could use and cost the cell
    // its page (B3, the cells seen again were grey)
    pub fn free_tiles(&mut self, size: u32, count: usize, keep: Option<usize>) -> usize {
        let frame = self.frame;
        let near = size > self.cache_size;
        let mut order: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                let page = if near { e.near_page } else { e.cache_page };
                Some(i) != keep
                    && e.last_used < frame
                    && !e.has_pending()
                    && page.is_some_and(|p| p.size == size)
            })
            .collect();
        order.sort_by_key(|&i| (self.entries[i].last_used, i));
        let mut freed = 0;
        for i in order.into_iter().take(count) {
            if near {
                self.demote(i);
            } else {
                if let Some(p) = self.entries[i].cache_page.take() {
                    self.pools.release(p);
                    self.release_cache_spec(i);
                }
                let e = &mut self.entries[i];
                e.cache_ready = false;
                e.cache_stale = false;
                e.state = if e.near_page.is_some() {
                    CellState::MeshAndCache
                } else {
                    CellState::None
                };
            }
            self.evicted += 1;
            self.counters.evicted[size_slot(size)] += 1;
            freed += 1;
        }
        freed
    }

    fn push_job(&mut self, cell: usize, kind: PageKind, page: PageId) {
        let e = &mut self.entries[cell];
        let spec = match kind {
            PageKind::Cache => {
                e.cache_out = true;
                e.cache_spec
            }
            PageKind::Near => {
                e.near_out = true;
                e.near_spec
            }
        };
        self.jobs.push_back(BakeJob {
            cell,
            kind,
            page,
            spec,
        });
    }

    /// The cache page of a cell: size `g_iCacheSize`.
    // re: TerrainCell::RequestCacheTexture (TerrainCell.cpp:1468-1510)
    pub fn request_cache_texture(&mut self, cell: usize) -> bool {
        let e = &self.entries[cell];
        if e.cache_page.is_some() {
            return true;
        }
        let size = self.cache_size;
        match self.page_for(size) {
            Some(page) => {
                self.entries[cell].cache_page = Some(page);
                self.entries[cell].cache_spec = self.spec_page_for(cell, page);
                self.entries[cell].spec_lent = false;
                self.entries[cell].state = CellState::CacheOnly;
                self.push_job(cell, PageKind::Cache, page);
                true
            }
            None => {
                self.entries[cell].state = CellState::MeshOnly;
                false
            }
        }
    }

    /// The near page of a cell, of the size its width on the screen asks (less when the pool of
    /// that size is empty: [`PagePools::get`] falls back to the next smaller one).
    // re: TerrainCell::RequestUncompressedTextures (TerrainCell.cpp:1512-1591)
    pub fn request_uncompressed_textures(&mut self, cell: usize, width_px: f32) -> bool {
        if self.entries[cell].near_page.is_some() {
            return true;
        }
        let size = self.page_size(width_px);
        if size <= self.cache_size {
            // The cache page serves.
            return true;
        }
        // The page must not be taken from the cell that asks.
        match self.page_for_keeping(size, cell) {
            Some(page) => {
                self.entries[cell].near_page = Some(page);
                self.entries[cell].near_spec = self.spec_page_for(cell, page);
                self.entries[cell].state = if self.non_dxt {
                    CellState::MeshAndCache
                } else {
                    CellState::Full
                };
                self.push_job(cell, PageKind::Near, page);
                true
            }
            None => false,
        }
    }

    /// A cell in view whose baked near page is not of the size its width asks for now gets one of that
    /// size when its pool has a page free (no page is taken from another cell for it). The old page is
    /// drawn until the new one is baked, then goes back to its pool. A cell that now asks for no more
    /// than the cache page gives its near page back and is drawn from the cache page. Without this a
    /// cell kept the page it got when it came into view for as long as it stayed in view: walked up to
    /// by a low eye (a camera of a mod) or zoomed in on, it was drawn from a page of a far cell, the
    /// ground at the feet a page of 256 or 128 magnified tens of times. Returns whether the cell was
    /// resized (a job made, or the near page given back).
    // re: Terrain::LaunchJobs (Terrain.cpp:1027-1035): a cell of status 5 whose `m_uPageSize` is not
    // `m_uMipSizeNeeded` goes back to the build when `CachePageAvailable(m_uMipSizeNeeded)`; its next
    // `RequestUncompressedTextures` (TerrainCell.cpp:1512-1576) takes a page of the new size
    pub fn resize_near(&mut self, cell: usize, width_px: f32) -> bool {
        let Some((want, _)) = self.resize_wanted(cell, width_px) else {
            return false;
        };
        if want <= self.cache_size {
            self.demote(cell);
            self.counters.resized += 1;
            return true;
        }
        let Some(page) = self.pools.pool_mut(want).and_then(|p| p.get()) else {
            return false;
        };
        let spec = self.spec_page_for(cell, page);
        let e = &mut self.entries[cell];
        e.near_old = e.near_page.take().map(|p| (p, e.near_spec.take()));
        e.near_page = Some(page);
        e.near_spec = spec;
        e.near_ready = false;
        e.near_stale = false;
        self.counters.resized += 1;
        self.push_job(cell, PageKind::Near, page);
        true
    }

    /// The size a cell of this width would be resized to by [`Self::resize_near`] now, and whether that
    /// is bigger than its page: `None` when its near page is of that size, not baked yet, has a job out,
    /// or the pool of that size has no page free (`CachePageAvailable`).
    fn resize_wanted(&self, cell: usize, width_px: f32) -> Option<(u32, bool)> {
        let want = self.page_size(width_px);
        let e = &self.entries[cell];
        let have = e.near_page?;
        if have.size == want || !e.near_ready || e.has_pending() || e.near_old.is_some() {
            return None;
        }
        let ok = if want <= self.cache_size {
            e.cache_ready && !e.cache_stale
        } else {
            self.pools.page_available(want)
        };
        ok.then_some((want, want > have.size))
    }

    /// The near page of the old size the cell was drawn from while the new one baked goes back to its
    /// pool.
    fn release_near_old(&mut self, cell: usize) {
        if let Some((p, s)) = self.entries[cell].near_old.take() {
            self.pools.release(p);
            if let Some(s) = s {
                self.pools.release(s);
            }
        }
    }

    /// A near page of `size` or, when that pool has none to give (none free, none out of view to take
    /// back), of the next smaller size that has one; never of the size of the cache pages, which would
    /// be the cache page again and only take a page of the base layer.
    fn page_for_keeping(&mut self, size: u32, keep: usize) -> Option<PageId> {
        let mut size = size;
        while size > self.cache_size {
            for _ in 0..MAX_ATTEMPTS {
                if self.pools.page_available(size) {
                    break;
                }
                if self.free_tiles(size, FREE_PER_ATTEMPT, Some(keep)) == 0 {
                    break;
                }
            }
            // The last of the pool too.
            if let Some(page) = self.pools.pool_mut(size).and_then(|p| p.get()) {
                return Some(page);
            }
            size >>= 1;
        }
        None
    }

    /// One frame of the automaton: the cells in `view` (visible, in the order of the original:
    /// the biggest on the screen first) that are candidates get their pages, at most `budget` cells
    /// per call. The cells not in `view` keep what they have, and age.
    // re: Terrain::LaunchJobs (Terrain.cpp:965-1130)
    pub fn launch_jobs(&mut self, cells: &[TerrainCell], view: &[CellView], budget: usize) {
        self.frame += 1;
        self.see_spec(cells);
        self.see_view_need(cells, view);
        let mut order: Vec<&CellView> = view
            .iter()
            .filter(|v| Self::is_candidate(&cells[v.cell]))
            .collect();
        // The cells with nothing to draw first, then the ones nearest to where the view looks next.
        order.sort_by(|a, b| {
            let (ha, hb) = (
                self.entries[a.cell].is_drawable(),
                self.entries[b.cell].is_drawable(),
            );
            ha.cmp(&hb)
                .then(a.dist.total_cmp(&b.dist))
                .then(a.cell.cmp(&b.cell))
        });
        self.dist.resize(self.entries.len(), f32::MAX);
        for v in &order {
            self.entries[v.cell].last_used = self.frame;
            self.dist[v.cell] = v.dist;
        }
        // The near jobs of the cells that left the view are not baked: the page goes back to its pool
        // and the cell is drawn from its cache page when it comes back.
        let gone: Vec<usize> = self
            .jobs
            .iter()
            .filter(|j| j.kind == PageKind::Near && self.entries[j.cell].last_used < self.frame)
            .map(|j| j.cell)
            .collect();
        for cell in gone {
            if self.entries[cell].near_page.is_some() {
                self.demote(cell);
            }
        }
        // The original frees a reserve of 8 pages here (Terrain.cpp:965-1130); that took the base pages of
        // the cells out of view whenever the pool of 64 was nearly full, and they were baked again when
        // the camera came back (AC-T3). A page of the base layer is taken back only when a request finds
        // its pool empty (`page_for`).
        let mut spent = 0;
        let mut resized = false;
        for v in &order {
            if spent >= budget {
                break;
            }
            let cell = v.cell;
            let before = self.jobs.len();
            let mut ok =
                self.entries[cell].cache_page.is_some() || self.request_cache_texture(cell);
            if ok && !v.ring {
                if self.entries[cell].near_page.is_none() {
                    ok = self.request_uncompressed_textures(cell, v.width_px);
                } else {
                    resized |= self.resize_near(cell, v.width_px);
                }
            }
            // Drawn from its cache page: the highlight of the cache page, if it gave it away.
            if ok && self.entries[cell].near_page.is_none() {
                self.restore_spec(cell);
            }
            if self.jobs.len() != before {
                spent += 1;
            }
        }
        // One page made bigger a frame past the budget, for the widest cell that wants one. The cells
        // with nothing to draw come first in the order, the ring around the view among them, and while
        // the camera moves the ring always has some: the budget went to them and a cell at the feet of a
        // walking low eye kept its small page (T27c). A page made smaller only saves memory: it waits for
        // the budget.
        // re: Terrain::LaunchJobs (Terrain.cpp:1027-1081) resizes within the launch budget only; it has
        // no ring
        if !resized
            && let Some((cell, width)) = order
                .iter()
                .filter(|v| !v.ring)
                .filter(|v| {
                    self.resize_wanted(v.cell, v.width_px)
                        .is_some_and(|(_, grows)| grows)
                })
                .max_by(|a, b| a.width_px.total_cmp(&b.width_px).then(b.cell.cmp(&a.cell)))
                .map(|v| (v.cell, v.width_px))
        {
            self.resize_near(cell, width);
        }
        self.sort_jobs();
    }

    /// The order the bake takes the jobs in: the cache page of a cell in view that has nothing to draw,
    /// then the other pages of the cells in view, nearest to where the view looks next first; then the
    /// rest (the loader, the cells out of view) in the order they came.
    fn sort_jobs(&mut self) {
        let frame = self.frame;
        let entries = &self.entries;
        let dist = &self.dist;
        let key = |j: &BakeJob| {
            let e = &entries[j.cell];
            let class = if e.last_used != frame {
                3u8
            } else if !e.is_drawable() {
                u8::from(j.kind == PageKind::Near)
            } else {
                2
            };
            let d = if class < 3 {
                dist.get(j.cell).copied().unwrap_or(f32::MAX)
            } else {
                f32::MAX
            };
            (class, d)
        };
        self.jobs.make_contiguous().sort_by(|a, b| {
            let (ka, kb) = (key(a), key(b));
            ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1))
        });
        self.counters.jobs_visible = self
            .jobs
            .iter()
            .filter(|j| entries[j.cell].last_used == frame)
            .count();
    }

    /// The loader of the first frame: the next `INIT_CELLS_PER_CALL` candidates that have no cache
    /// page get one. Returns whether it has looked at every cell.
    // re: Terrain::NonBlockingTextureInit (Terrain.cpp:1310-1350)
    pub fn init_step(&mut self, cells: &[TerrainCell]) -> bool {
        self.see_spec(cells);
        // The pools follow the map: the base layer is made as the cells are seen. Baking the whole map
        // at the start was ten thousand pages on a huge map all open (two minutes of the load, and the
        // pages of the cells never seen held for nothing).
        if self.auto.is_some() {
            self.init_cursor = cells.len();
            return true;
        }
        let mut taken = 0;
        while self.init_cursor < cells.len() && taken < INIT_CELLS_PER_CALL {
            let i = self.init_cursor;
            self.init_cursor += 1;
            if Self::is_candidate(&cells[i]) {
                self.entries[i].last_used = self.frame;
                if self.request_cache_texture(i) {
                    taken += 1;
                }
            }
        }
        self.init_cursor >= cells.len()
    }

    /// Starts the loader again (a new map).
    pub fn restart_init(&mut self) {
        self.init_cursor = 0;
    }

    /// The jobs the bake has not taken.
    pub fn jobs(&self) -> impl Iterator<Item = &BakeJob> {
        self.jobs.iter()
    }

    /// The next job, in the order they were made.
    pub fn take_job(&mut self) -> Option<BakeJob> {
        self.jobs.pop_front()
    }

    /// The bake has put the page in place.
    pub fn complete(&mut self, job: BakeJob) {
        let e = &mut self.entries[job.cell];
        let again = match job.kind {
            PageKind::Cache => {
                e.cache_out = false;
                let again = e.cache_ready && e.cache_stale;
                if e.cache_page == Some(job.page) {
                    e.cache_ready = true;
                    e.cache_stale = false;
                    e.cache_spec_ready = e.cache_spec.is_some() && job.spec == e.cache_spec;
                }
                again
            }
            PageKind::Near => {
                e.near_out = false;
                let again = e.near_ready && e.near_stale;
                if e.near_page == Some(job.page) {
                    e.near_ready = true;
                    e.near_stale = false;
                    // The page of the old size was drawn until now.
                    if let Some((p, s)) = e.near_old.take() {
                        self.pools.release(p);
                        if let Some(s) = s {
                            self.pools.release(s);
                        }
                    }
                }
                again
            }
        };
        if again {
            self.counters.rebaked += 1;
        }
        if job.kind == PageKind::Cache && std::mem::take(&mut e.background) {
            self.counters.background_baked += 1;
        }
        if e.state == CellState::Rebuild && !e.has_pending() {
            e.state = if e.near_page.is_some() {
                if self.non_dxt {
                    CellState::MeshAndCache
                } else {
                    CellState::Full
                }
            } else {
                CellState::CacheOnly
            };
        }
    }

    /// The content of a cell changed: the pages it has are baked again. A page that was baked stays
    /// drawn, marked stale, until its job is done: the cell does not go blank for the frames the job
    /// waits (B3, AC-T1).
    // re: TerrainCell::Build with the state "rebuild" (TerrainCell.cpp:1593-1627)
    pub fn invalidate(&mut self, cell: usize, why: Rebake) {
        let (cache, near) = {
            let e = &mut self.entries[cell];
            if e.cache_page.is_none() && e.near_page.is_none() {
                return;
            }
            self.counters.rebake_cause[why as usize] += 1;
            e.state = CellState::Rebuild;
            e.cache_stale = e.cache_ready;
            e.near_stale = e.near_ready;
            (
                if e.cache_out { None } else { e.cache_page },
                if e.near_out { None } else { e.near_page },
            )
        };
        if let Some(p) = cache {
            self.push_job(cell, PageKind::Cache, p);
        }
        if let Some(p) = near {
            self.push_job(cell, PageKind::Near, p);
        }
    }

    /// The near page goes back to the pool and the cell is drawn from its cache page
    /// ("demote": the cell is far now).
    pub fn demote(&mut self, cell: usize) {
        self.release_near_old(cell);
        let e = &mut self.entries[cell];
        if let Some(p) = e.near_page.take() {
            self.pools.release(p);
            if let Some(s) = e.near_spec.take() {
                self.pools.release(s);
            }
            e.near_ready = false;
            e.near_stale = false;
            e.near_out = false;
            self.jobs
                .retain(|j| !(j.cell == cell && j.kind == PageKind::Near));
            if e.cache_page.is_some() {
                e.state = CellState::CacheOnly;
            }
            self.counters.demoted += 1;
        }
    }

    /// Work left: jobs not done, and candidates the loader has not served.
    pub fn pending_work(&self, cells: &[TerrainCell]) -> usize {
        let out = self.entries.iter().filter(|e| e.has_pending()).count();
        // Once pages have been freed the pools are full: a cell without a page is then one the cache gave up
        // (a map with more land than pages, seen from one side: S3), and the loader serves it only when it is
        // in view. It is not waited for.
        let unserved = if self.auto.is_some() {
            // The pools follow the map: the base layer is made as the cells are seen, so the work is
            // the cells of the last view (and its ring) that have no page yet, or that lost the
            // highlight of their cache page and wait for it.
            let frame = self.frame;
            cells
                .iter()
                .enumerate()
                .filter(|(i, c)| {
                    let e = &self.entries[*i];
                    Self::is_candidate(c)
                        && frame > 0
                        && e.last_used == frame
                        && !e.has_pending()
                        && (e.cache_page.is_none()
                            || (e.near_page.is_none()
                                && e.spec_lent
                                && e.cache_spec.is_none()
                                && self.wants_spec.get(*i).copied().unwrap_or(false)))
                })
                .count()
        } else if self.evicted > 0 {
            0
        } else {
            cells
                .iter()
                .enumerate()
                .filter(|(i, c)| {
                    Self::is_candidate(c) && self.entries[*i].cache_page.is_none() && {
                        // A cell that got no page (MeshOnly) is not waited for: nothing will come.
                        self.entries[*i].state != CellState::MeshOnly
                    }
                })
                .count()
        };
        out + unserved
    }

    /// 0..=1: the share of the candidates whose cache page is baked.
    pub fn load_progress(&self, cells: &[TerrainCell]) -> f32 {
        let total = cells.iter().filter(|c| Self::is_candidate(c)).count();
        if total == 0 {
            return 1.0;
        }
        let done = cells
            .iter()
            .enumerate()
            .filter(|(i, c)| Self::is_candidate(c) && self.entries[*i].cache_ready)
            .count();
        done as f32 / total as f32
    }
}

/// The limits of the pools: by size 64, 128, 256, 512.
fn limits_of(s: &TerrainSettings) -> [u32; 4] {
    let p = super::settings_map::PAGE_SIZES;
    [
        s.page_limit(p[0]),
        s.page_limit(p[1]),
        s.page_limit(p[2]),
        s.page_limit(p[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::terrain::cells::{CellBounds, CellBox, CellStats, TextureSet};
    use crate::world::terrain::sampler::TerrainFrame;

    fn land_cell(index: usize, open: bool) -> TerrainCell {
        let frame = TerrainFrame::new(8, 8, false);
        let rect = CellBox {
            x: 0,
            y: 0,
            w: 65,
            h: 65,
        };
        TerrainCell {
            index,
            rect,
            bounds: CellBounds::new(&frame, &rect),
            hexes: vec![(0, 0)],
            class_mask: 0,
            fow_flags: u8::from(open),
            stats: CellStats::default(),
            textures: TextureSet::default(),
            force_spec: false,
            has_spec: false,
        }
    }

    fn cells(n: usize) -> Vec<TerrainCell> {
        (0..n).map(|i| land_cell(i, true)).collect()
    }

    fn settings(chunks: [u32; 4]) -> TerrainSettings {
        TerrainSettings {
            chunks,
            ..TerrainSettings::default()
        }
    }

    fn drain(cache: &mut TerrainCache) -> usize {
        let mut n = 0;
        while let Some(job) = cache.take_job() {
            cache.complete(job);
            n += 1;
        }
        n
    }

    fn view(widths: &[f32]) -> Vec<CellView> {
        widths
            .iter()
            .enumerate()
            .map(|(cell, &width_px)| CellView {
                cell,
                width_px,
                // The widest is the nearest.
                dist: 1.0 / (width_px + 1.0),
                ring: false,
            })
            .collect()
    }

    #[test]
    fn the_loader_takes_four_cells_a_call_and_the_work_ends() {
        let cs = cells(10);
        let mut cache = TerrainCache::new(10, &settings([64, 64, 64, 64]), 64, true);
        assert_eq!(cache.pending_work(&cs), 10);
        assert_eq!(cache.load_progress(&cs), 0.0);
        let mut calls = 0;
        loop {
            calls += 1;
            let done = cache.init_step(&cs);
            let jobs = cache.jobs().count();
            assert!(jobs <= INIT_CELLS_PER_CALL, "{jobs}");
            drain(&mut cache);
            if done {
                break;
            }
        }
        assert_eq!(calls, 3);
        assert_eq!(cache.pending_work(&cs), 0);
        assert_eq!(cache.load_progress(&cs), 1.0);
        assert!(
            cs.iter()
                .all(|c| cache.entry(c.index).state == CellState::CacheOnly)
        );
    }

    /// A cell with a highlight takes a second page of the size of its page from the same pool; the job
    /// carries it, and both go back to the pool when the cell gives up the page.
    #[test]
    fn a_cell_with_a_highlight_takes_and_gives_back_two_pages() {
        let mut cs = cells(2);
        cs[1].has_spec = true;
        let mut cache = TerrainCache::new(2, &settings([8, 8, 8, 8]), 64, true);
        while !cache.init_step(&cs) {}
        let jobs: Vec<BakeJob> = cache.jobs().copied().collect();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].spec, None);
        let spec = jobs[1].spec.expect("the highlight page");
        assert_eq!(spec.size, jobs[1].page.size);
        assert_ne!(spec, jobs[1].page);
        assert_eq!(cache.entry(1).cache_spec, Some(spec));
        assert_eq!(cache.pools().available(64), 8 - 3);
        drain(&mut cache);
        // Not wanted any more (an older frame), the pages stay (the base layer); when the pool is asked
        // for more than it has, the cell gives back its page and the highlight page with it.
        cache.launch_jobs(&cs, &[], 0);
        assert!(cache.entry(1).cache_page.is_some());
        assert_eq!(cache.free_tiles(64, 2, None), 2);
        assert_eq!(cache.entry(1).cache_page, None);
        assert_eq!(cache.entry(1).cache_spec, None);
        assert_eq!(cache.pools().available(64), 8);
    }

    /// A view of the cells `range`, all at `width` pixels.
    fn view_of(range: std::ops::Range<usize>, width: f32) -> Vec<CellView> {
        range
            .map(|cell| CellView {
                cell,
                width_px: width,
                dist: cell as f32,
                ring: false,
            })
            .collect()
    }

    /// Launches and bakes until the view has nothing left to do.
    fn settle(cache: &mut TerrainCache, cs: &[TerrainCell], view: &[CellView]) {
        for _ in 0..40 {
            cache.launch_jobs(cs, view, 64);
            drain(cache);
        }
        assert_eq!(cache.pending_work(cs), 0);
    }

    /// B3: with the pools following the map, a map with more land than the key's pages (a huge map at
    /// the key of a standard one) keeps the base page of every cell it has seen; nothing is baked for
    /// the cells never seen; the highlight pages are for the view: a cell out of view gives its own to
    /// one in view, and gets one back (its page baked again) when it is seen again.
    #[test]
    fn pools_that_follow_the_map_keep_the_base_layer_and_lend_the_highlights() {
        let n = 300;
        let mut cs = cells(n);
        for c in &mut cs {
            c.has_spec = true;
        }
        // The key holds 64 pages; the map needs 300 base pages.
        let mut cache = TerrainCache::new(n, &settings([64, 8, 4, 2]), 64, true);
        cache.set_auto_size(true);
        // The loader bakes nothing, and nothing is work before a view.
        assert!(cache.init_step(&cs));
        assert_eq!((cache.jobs().count(), cache.pending_work(&cs)), (0, 0));
        // A third of the map in view: its cells get their pages and their highlights.
        settle(&mut cache, &cs, &view_of(0..100, 30.0));
        assert!((0..100).all(|i| {
            let e = cache.entry(i);
            e.cache_ready && e.cache_spec.is_some() && e.cache_spec_ready
        }));
        assert!((100..n).all(|i| cache.entry(i).cache_page.is_none()));
        assert_eq!(cache.spec_budget(), Some(2 * 100 + 64));
        // The camera goes over the rest of the map: past the share of the view, the cells seen first
        // give their highlight pages; no base page is taken back.
        settle(&mut cache, &cs, &view_of(100..200, 30.0));
        settle(&mut cache, &cs, &view_of(200..300, 30.0));
        assert_eq!(cache.counters().evicted, [0; 4]);
        assert!(cs.iter().all(|c| cache.entry(c.index).cache_ready));
        assert!(cache.cache_specs() <= 264);
        assert!(cache.counters().spec_dropped > 0);
        assert!(cache.entry(0).cache_spec.is_none() && !cache.entry(0).cache_spec_ready);
        assert!(cache.pools().pool(64).unwrap().in_use() <= 300 + 264);
        // Back to the start: the cells there that gave theirs get a highlight again, their pages baked
        // again.
        let baked = cache.counters().spec_restored;
        settle(&mut cache, &cs, &view_of(0..100, 30.0));
        assert!(cache.counters().spec_restored >= baked + 36);
        assert!((0..100).all(|i| cache.entry(i).cache_spec_ready));
        assert_eq!(cache.counters().evicted, [0; 4]);
        // Seen again as it is, nothing is baked.
        cache.launch_jobs(&cs, &view_of(0..100, 30.0), 64);
        assert_eq!(cache.jobs().count(), 0);
        let limits = cache.limits();
        assert!(
            limits[0] >= 300 + 264 && limits[0] <= 300 + 264 + 64,
            "{limits:?}"
        );
        // Every cell in view, at the width of a 128 page: the pool of 128 grows to hold them.
        let wide = vec![100.0; n];
        for _ in 0..40 {
            cache.launch_jobs(&cs, &view(&wide), 64);
            drain(&mut cache);
        }
        assert_eq!(cache.counters().evicted, [0; 4]);
        assert_eq!(cache.counters().demoted, 0);
        assert!(cs.iter().all(|c| cache.entry(c.index).near_ready));
        assert!(cache.limits()[1] >= 400, "{:?}", cache.limits());
        // Seen again, nothing is baked.
        cache.launch_jobs(&cs, &view(&wide), 64);
        assert_eq!(cache.jobs().count(), 0);
        // A pool the view does not ask for keeps a few pages over the key, and the fixed limits come
        // back without the map.
        assert_eq!(cache.limits()[2..], [8, 8]);
        cache.set_auto_size(false);
        assert_eq!(cache.limits()[0], 64);
    }

    /// The base pages of the cells out of view are baked in the background when the pool has one free:
    /// after the jobs of the view, a highlight only while the share of the view has one free (the cell
    /// gets it when it is seen), and never taking a page from another cell.
    #[test]
    fn the_background_bakes_base_pages_after_the_view() {
        let n = 400;
        let mut cs = cells(n);
        for c in &mut cs {
            c.has_spec = true;
        }
        let mut cache = TerrainCache::new(n, &settings([64, 8, 4, 2]), 64, true);
        cache.set_auto_size(true);
        settle(&mut cache, &cs, &view_of(0..20, 30.0));
        let budget = cache.spec_budget().unwrap();
        assert_eq!(budget, 2 * 20 + 64);
        // The background: one job at a time, out of view, after the view's.
        for i in 20..n {
            assert!(cache.request_background(i));
            assert!(!cache.request_background(i), "once");
            assert_eq!(cache.jobs_out_of_view(), 1);
            drain(&mut cache);
        }
        assert_eq!(cache.counters().background_baked, (n - 20) as u64);
        assert!(cs.iter().all(|c| cache.entry(c.index).cache_ready));
        assert_eq!(cache.counters().evicted, [0; 4]);
        assert_eq!(cache.counters().spec_dropped, 0);
        assert!(cache.cache_specs() <= budget);
        // A cell baked without its highlight gets it when it is seen.
        let lent = (20..n)
            .find(|&i| cache.entry(i).cache_spec.is_none())
            .unwrap();
        settle(&mut cache, &cs, &view_of(lent..lent + 1, 30.0));
        assert!(cache.entry(lent).cache_spec_ready);
        assert!(cache.counters().spec_restored >= 1);
    }

    /// The ceiling of 2 GB: the near pools give way first, the base layer is cut only past it alone.
    #[test]
    fn the_pools_stay_under_the_ceiling() {
        use crate::world::terrain::pages::page_bytes;
        let bytes = |l: [u32; 4]| -> u64 {
            l.iter()
                .zip(super::super::settings_map::PAGE_SIZES)
                .map(|(&n, s)| u64::from(n) * page_bytes(s))
                .sum()
        };
        let auto = AutoPools {
            floors: [4096, 512, 256, 64],
            base_need: 20_000,
            spec_cells: 20_000,
            spec_view: 3000,
            near_need: [0, 4000, 2000, 1000],
        };
        let l = auto_limits(&auto, 64, POOL_BUDGET_BYTES, 512);
        assert!(bytes(l) <= POOL_BUDGET_BYTES, "{l:?}");
        assert!(l[0] >= 20_000);
        let huge = AutoPools {
            base_need: 200_000,
            ..auto
        };
        let l = auto_limits(&huge, 64, POOL_BUDGET_BYTES, 512);
        assert!(bytes(l) <= POOL_BUDGET_BYTES, "{l:?}");
        // A standard map, a 1080p view: what it needs, no less than the keys.
        // 2000 + 125 + the highlights of 600 cells of the view and the ring (2 x 600 + 64); two views
        // of the 300 pages of 128 and a quarter (683).
        let standard = AutoPools {
            base_need: 2000,
            spec_cells: 1900,
            spec_view: 600,
            near_need: [0, 300, 60, 10],
            ..auto
        };
        assert_eq!(
            auto_limits(&standard, 64, POOL_BUDGET_BYTES, 512),
            [3392, 683, 256, 64]
        );
    }

    /// The terrain detail: no pool above the biggest page, and the pages within the budget of the level.
    #[test]
    fn the_pools_obey_the_page_budget_of_the_detail() {
        use crate::terrain_detail::TerrainDetail;
        use crate::world::terrain::pages::page_bytes;
        let bytes = |l: [u32; 4]| -> u64 {
            l.iter()
                .zip(super::super::settings_map::PAGE_SIZES)
                .map(|(&n, s)| u64::from(n) * page_bytes(s))
                .sum()
        };
        let view = AutoPools {
            floors: [4096, 512, 256, 64],
            base_need: 6000,
            spec_cells: 5000,
            spec_view: 600,
            near_need: [0, 900, 600, 300],
        };
        for d in TerrainDetail::ALL {
            let l = auto_limits(&view, 64, d.pool_budget_bytes(), d.max_page());
            assert!(
                bytes(l) <= d.pool_budget_bytes(),
                "{d:?}: {l:?} is {} bytes",
                bytes(l)
            );
            for (n, s) in l.iter().zip(super::super::settings_map::PAGE_SIZES) {
                assert!(s <= d.max_page() || *n == 0, "{d:?}: pages of {s}");
            }
        }
        // Medium has no 512 pages and Low no 256 either; High keeps both.
        let at = |d: TerrainDetail| auto_limits(&view, 64, d.pool_budget_bytes(), d.max_page());
        assert_eq!(at(TerrainDetail::Medium)[3], 0);
        assert!(at(TerrainDetail::Medium)[2] > 0);
        assert_eq!(at(TerrainDetail::Low)[2..], [0, 0]);
        assert!(at(TerrainDetail::High)[3] > 0);
    }

    /// A cell that wants a page above the detail gets the biggest one the detail allows, and a page is
    /// never missing for it.
    #[test]
    fn a_wide_cell_gets_a_coarser_page_under_a_lower_detail_and_never_a_hole() {
        use crate::terrain_detail::TerrainDetail;
        let cs = cells(4);
        for (d, want) in [
            (TerrainDetail::High, 512),
            (TerrainDetail::Medium, 256),
            (TerrainDetail::Low, 128),
        ] {
            let s = settings([64, 64, 64, 64]).with_detail(d);
            let mut cache = TerrainCache::new(4, &s, 64, true);
            assert_eq!(cache.page_size(900.0), want, "{d:?}");
            assert_eq!(cache.page_size(10.0), 64, "{d:?}");
            settle(&mut cache, &cs, &view_of(0..4, 900.0));
            let near = cache.entry(0).near_page.expect("a near page");
            assert_eq!(near.size, want, "{d:?}");
            assert!(cache.entry(0).is_drawable(), "{d:?}");
        }
    }

    #[test]
    fn cells_that_are_closed_or_not_land_are_not_loaded() {
        let mut cs = cells(4);
        cs[1].fow_flags = 0;
        cs[2].stats.class = StatsClass::Low;
        let mut cache = TerrainCache::new(4, &settings([64, 64, 64, 64]), 64, true);
        assert_eq!(cache.init_wanted(&cs), 2);
        while !cache.init_step(&cs) {}
        drain(&mut cache);
        assert_eq!(cache.pending_work(&cs), 0);
        assert!(cache.entry(1).cache_page.is_none() && cache.entry(2).cache_page.is_none());
        assert!(cache.entry(0).cache_page.is_some() && cache.entry(3).cache_page.is_some());
        // No candidates at all: the progress is complete.
        let none: Vec<TerrainCell> = Vec::new();
        assert_eq!(cache.load_progress(&none), 1.0);
    }

    #[test]
    fn a_near_cell_gets_the_page_its_width_asks_and_a_far_one_keeps_the_cache_page() {
        let cs = cells(3);
        let mut cache = TerrainCache::new(3, &settings([64, 64, 64, 64]), 64, true);
        cache.launch_jobs(&cs, &view(&[300.0, 200.0, 10.0]), 8);
        let jobs: Vec<BakeJob> = cache.jobs().copied().collect();
        // Two jobs each for the near cells (cache and near), one for the far one.
        assert_eq!(jobs.len(), 5);
        assert_eq!(cache.entry(0).near_page.map(|p| p.size), Some(512));
        assert_eq!(cache.entry(1).near_page.map(|p| p.size), Some(256));
        assert!(cache.entry(2).near_page.is_none());
        assert_eq!(cache.entry(2).cache_page.map(|p| p.size), Some(64));
        assert_eq!(cache.entry(0).state, CellState::MeshAndCache);
        drain(&mut cache);
        assert!(cache.entry(0).near_ready && cache.entry(0).is_drawable());
        // Compressed textures: the near cell is "full".
        let mut cache = TerrainCache::new(3, &settings([64, 64, 64, 64]), 64, false);
        cache.launch_jobs(&cs, &view(&[300.0, 10.0, 10.0]), 8);
        assert_eq!(cache.entry(0).state, CellState::Full);
    }

    #[test]
    fn a_shortage_of_512_gives_256() {
        let cs = cells(2);
        // One 512 page in the pool, taken by the first cell; the second gets a 256.
        let mut cache = TerrainCache::new(2, &settings([8, 8, 8, 1]), 64, true);
        cache.launch_jobs(&cs, &view(&[400.0, 390.0]), 8);
        assert_eq!(cache.entry(0).near_page.map(|p| p.size), Some(512));
        assert_eq!(cache.entry(1).near_page.map(|p| p.size), Some(256));
        drain(&mut cache);
    }

    #[test]
    fn the_budget_of_candidates_per_call_is_the_key() {
        let cs = cells(10);
        let mut cache = TerrainCache::new(10, &settings([64, 64, 64, 64]), 64, true);
        cache.launch_jobs(&cs, &view(&[10.0; 10]), 2);
        let cells_with_job: std::collections::BTreeSet<usize> =
            cache.jobs().map(|j| j.cell).collect();
        assert_eq!(cells_with_job.len(), 2);
        drain(&mut cache);
        cache.launch_jobs(&cs, &view(&[10.0; 10]), 1);
        assert_eq!(
            cache
                .jobs()
                .map(|j| j.cell)
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1
        );
    }

    #[test]
    fn the_biggest_on_the_screen_comes_first() {
        let cs = cells(4);
        let mut cache = TerrainCache::new(4, &settings([64, 64, 64, 64]), 64, true);
        cache.launch_jobs(&cs, &view(&[10.0, 90.0, 20.0, 50.0]), 2);
        let first: Vec<usize> = cache.jobs().map(|j| j.cell).collect();
        assert!(first.contains(&1) && first.contains(&3), "{first:?}");
        assert!(!first.contains(&0) && !first.contains(&2));
    }

    #[test]
    fn lru_gives_up_the_pages_of_the_cells_not_seen_for_longest() {
        let cs = cells(4);
        // Room for 2 pages of 64 in the pool (the reserve of 8 would ask for more).
        let mut cache = TerrainCache::new(4, &settings([2, 2, 2, 2]), 64, true);
        cache.launch_jobs(&cs, &view(&[10.0, 10.0, 0.0, 0.0]), 8);
        drain(&mut cache);
        assert!(cache.entry(0).cache_page.is_some() && cache.entry(1).cache_page.is_some());
        // Cell 0 is seen again, then the view moves to cells 2 and 3 only.
        let mut v = view(&[10.0, 0.0, 10.0, 10.0]);
        v.retain(|c| c.cell != 1);
        cache.launch_jobs(&cs, &v[..1], 8);
        drain(&mut cache);
        let v = [CellView {
            cell: 2,
            width_px: 10.0,
            dist: 0.1,
            ring: false,
        }];
        cache.launch_jobs(&cs, &v, 8);
        drain(&mut cache);
        // Cell 1 (the least recently used) lost its page, cell 0 may have lost it for 2 as well,
        // but 2 has one.
        assert!(cache.entry(2).cache_page.is_some());
        assert!(cache.entry(1).cache_page.is_none());
        assert_eq!(cache.entry(1).state, CellState::None);
        assert!(cache.evicted() >= 1);
        // The cell asking is never the one robbed.
        assert!(cache.entry(2).cache_ready);
    }

    #[test]
    fn a_cell_with_pending_work_keeps_its_pages() {
        let cs = cells(2);
        let mut cache = TerrainCache::new(2, &settings([1, 1, 1, 1]), 64, true);
        cache.launch_jobs(&cs, &view(&[10.0, 0.0])[..1], 8);
        // The job for cell 0 is out. Cell 1 asks, nothing can be freed: mesh only.
        cache.launch_jobs(
            &cs,
            &[CellView {
                cell: 1,
                width_px: 10.0,
                dist: 0.1,
                ring: false,
            }],
            8,
        );
        assert!(cache.entry(0).cache_page.is_some());
        assert_eq!(cache.entry(1).state, CellState::MeshOnly);
        assert!(cache.entry(1).cache_page.is_none());
        // A cell that got nothing is not waited for.
        drain(&mut cache);
        assert_eq!(cache.pending_work(&cs), 0);
    }

    #[test]
    fn an_invalidated_cell_is_baked_again_into_the_same_page() {
        let cs = cells(1);
        let mut cache = TerrainCache::new(1, &settings([4, 4, 4, 4]), 64, true);
        cache.launch_jobs(&cs, &view(&[200.0]), 8);
        drain(&mut cache);
        let (cp, np) = (cache.entry(0).cache_page, cache.entry(0).near_page);
        assert!(cache.entry(0).cache_ready && cache.entry(0).near_ready);
        cache.invalidate(0, Rebake::Field);
        assert_eq!(cache.entry(0).state, CellState::Rebuild);
        // The old pages stay drawn until the new ones are baked (AC-T1).
        assert!(cache.entry(0).is_drawable());
        assert!(cache.entry(0).cache_stale && cache.entry(0).near_stale);
        assert_eq!(cache.counters().rebake_cause, [1, 0, 0]);
        let jobs: Vec<BakeJob> = cache.jobs().copied().collect();
        assert_eq!(jobs.len(), 2);
        assert!(
            jobs.iter()
                .any(|j| j.kind == PageKind::Cache && Some(j.page) == cp)
        );
        assert!(
            jobs.iter()
                .any(|j| j.kind == PageKind::Near && Some(j.page) == np)
        );
        // A second invalidation while the jobs are out adds none.
        cache.invalidate(0, Rebake::Field);
        assert_eq!(cache.jobs().count(), 2);
        drain(&mut cache);
        assert_eq!(cache.entry(0).state, CellState::MeshAndCache);
        assert!(!cache.entry(0).cache_stale && !cache.entry(0).near_stale);
        assert_eq!(cache.counters().rebaked, 2);
        // A cell without pages is not made a job by invalidation.
        let mut empty = TerrainCache::new(1, &settings([4, 4, 4, 4]), 64, true);
        empty.invalidate(0, Rebake::Field);
        assert_eq!(empty.jobs().count(), 0);
    }

    #[test]
    fn demote_returns_the_near_page_to_the_pool() {
        let cs = cells(1);
        let mut cache = TerrainCache::new(1, &settings([4, 4, 4, 4]), 64, true);
        cache.launch_jobs(&cs, &view(&[200.0]), 8);
        let before = cache.pools().in_use();
        assert_eq!(before, 2);
        drain(&mut cache);
        cache.demote(0);
        assert_eq!(cache.pools().in_use(), 1);
        assert_eq!(cache.entry(0).state, CellState::CacheOnly);
        assert!(cache.entry(0).cache_ready && !cache.entry(0).near_ready);
        // A near job that was not taken goes with it.
        cache.launch_jobs(&cs, &view(&[200.0]), 8);
        assert_eq!(cache.jobs().count(), 1);
        cache.demote(0);
        assert_eq!(cache.jobs().count(), 0);
    }

    /// A near page of 512 for a cell in view is taken back from a cell out of view, which keeps its cache
    /// page: no page of 64 goes for it (B3: the base layer was robbed for the big pages).
    #[test]
    fn a_big_page_is_taken_from_its_own_pool_and_the_base_layer_stays() {
        let cs = cells(2);
        let mut cache = TerrainCache::new(2, &settings([8, 0, 0, 2]), 64, true);
        cache.launch_jobs(&cs, &view(&[400.0, 0.0])[..1], 8);
        drain(&mut cache);
        assert_eq!(cache.entry(0).near_page.map(|p| p.size), Some(512));
        // Only cell 1 in view now, as wide; the pool of 512 has one page left (a swap needs two).
        let v = [CellView {
            cell: 1,
            width_px: 400.0,
            dist: 0.0,
            ring: false,
        }];
        cache.launch_jobs(&cs, &v, 8);
        drain(&mut cache);
        assert_eq!(cache.entry(1).near_page.map(|p| p.size), Some(512));
        assert!(cache.entry(0).cache_ready, "the base page stays");
        assert!(
            cache.entry(0).near_page.is_none(),
            "the near page went back"
        );
        assert_eq!(cache.counters().evicted, [0, 0, 0, 1]);
        assert_eq!(cache.counters().demoted, 1);
    }

    /// With no near page to be had, the cell keeps its cache page alone: a near page of 64 would be a
    /// second copy of it out of the base layer.
    #[test]
    fn a_near_page_is_never_of_the_size_of_the_cache_pages() {
        let cs = cells(1);
        let mut cache = TerrainCache::new(1, &settings([8, 0, 0, 0]), 64, true);
        cache.launch_jobs(&cs, &view(&[400.0]), 8);
        assert!(cache.entry(0).near_page.is_none());
        assert_eq!(cache.jobs().count(), 1);
        assert_eq!(cache.pools().pool(64).unwrap().in_use(), 1);
    }

    /// A cell in view with nothing to draw is baked first, then the cells in view nearest the middle;
    /// the near jobs of the cells that left the view are dropped.
    #[test]
    fn the_holes_in_view_come_first_and_jobs_out_of_view_are_dropped() {
        let cs = cells(4);
        let mut cache = TerrainCache::new(4, &settings([16, 16, 16, 16]), 64, true);
        let near = |cell, dist| CellView {
            cell,
            width_px: 10.0,
            dist,
            ring: false,
        };
        cache.launch_jobs(&cs, &[near(0, 0.1), near(1, 0.2)], 8);
        drain(&mut cache);
        cache.invalidate(0, Rebake::Decal);
        cache.invalidate(1, Rebake::Decal);
        // Cell 3 asks for a near page, then leaves the view at once.
        cache.launch_jobs(
            &cs,
            &[CellView {
                cell: 3,
                width_px: 300.0,
                dist: 0.3,
                ring: false,
            }],
            8,
        );
        assert!(cache.entry(3).near_page.is_some());
        // Cell 2 comes into view with nothing, the farthest from the middle.
        cache.launch_jobs(&cs, &[near(1, 0.2), near(0, 0.5), near(2, 0.9)], 8);
        let order: Vec<(usize, PageKind)> = cache.jobs().map(|j| (j.cell, j.kind)).collect();
        assert_eq!(order[0], (2, PageKind::Cache), "{order:?}");
        assert_eq!(order[1], (1, PageKind::Cache), "{order:?}");
        assert_eq!(order[2], (0, PageKind::Cache), "{order:?}");
        assert!(
            !order.iter().any(|&(c, k)| c == 3 && k == PageKind::Near),
            "{order:?}"
        );
        assert!(cache.entry(3).near_page.is_none());
        assert_eq!(cache.counters().jobs_visible, 3);
    }

    #[test]
    fn the_limits_follow_the_keys() {
        let mut cache = TerrainCache::new(1, &settings([4, 3, 2, 1]), 64, true);
        assert_eq!(cache.pools().available(64), 4);
        assert_eq!(cache.pools().available(512), 1);
        cache.set_settings(&settings([8, 3, 2, 1]));
        assert_eq!(cache.pools().available(64), 8);
    }

    /// T27c: a cell that stays in view while its width grows (walked up to by a low eye, zoomed in on)
    /// gets the page its width now asks for; the old page is drawn until the new one is baked, then goes
    /// back to its pool. A cell that shrinks below the cache page gives its near page back.
    #[test]
    fn a_cell_in_view_gets_the_page_of_its_new_width() {
        let cs = cells(1);
        let mut cache = TerrainCache::new(1, &settings([8, 8, 8, 8]), 64, true);
        settle(&mut cache, &cs, &view(&[200.0]));
        let small = cache.entry(0).near_page.expect("a near page");
        assert_eq!(small.size, 256);
        // Nearer: one job for a 512 page; the 256 page is drawn meanwhile.
        cache.launch_jobs(&cs, &view(&[3000.0]), 8);
        let jobs: Vec<BakeJob> = cache.jobs().copied().collect();
        assert_eq!(jobs.len(), 1, "{jobs:?}");
        assert_eq!((jobs[0].kind, jobs[0].page.size), (PageKind::Near, 512));
        assert_eq!(cache.entry(0).drawn_page().map(|(p, _)| p), Some(small));
        assert_eq!(
            cache.pools().available(256),
            7,
            "the old page is still held"
        );
        drain(&mut cache);
        let big = cache.entry(0).drawn_page().map(|(p, _)| p).expect("drawn");
        assert_eq!(big.size, 512);
        assert_eq!(cache.pools().available(256), 8, "the old page went back");
        assert_eq!(cache.counters().resized, 1);
        // The same width again: nothing to do.
        cache.launch_jobs(&cs, &view(&[3000.0]), 8);
        assert_eq!(cache.jobs().count(), 0);
        // Far: the near page goes back and the cache page is drawn.
        cache.launch_jobs(&cs, &view(&[10.0]), 8);
        assert!(cache.entry(0).near_page.is_none());
        assert_eq!(cache.entry(0).drawn_page().map(|(p, _)| p.size), Some(64));
        assert_eq!(cache.pools().available(512), 8);
    }

    /// No page is taken from another cell for a resize: with the pool of 512 out, the cell keeps its 256
    /// page. And a cell that leaves the view while its new page bakes gives back both pages.
    #[test]
    fn a_resize_waits_for_a_free_page_and_a_cell_that_leaves_gives_both_back() {
        let cs = cells(3);
        let mut cache = TerrainCache::new(3, &settings([8, 8, 8, 2]), 64, true);
        // Cell 0 holds 512 pages: the pool of 512 has one left, and `CachePageAvailable` wants two.
        settle(&mut cache, &cs, &view(&[3000.0, 200.0, 10.0]));
        assert_eq!(cache.entry(1).near_page.map(|p| p.size), Some(256));
        cache.launch_jobs(&cs, &view(&[3000.0, 3000.0, 10.0]), 8);
        assert_eq!(cache.jobs().count(), 0, "no page free for the resize");
        assert_eq!(cache.entry(1).near_page.map(|p| p.size), Some(256));
        // Cell 0 out of view and its pages taken back: cell 1 gets its 512.
        cache.demote(0);
        cache.launch_jobs(&cs, &view(&[10.0, 3000.0, 10.0]), 8);
        assert_eq!(cache.jobs().count(), 1);
        // It leaves before the job is baked: the job is dropped, both pages go back.
        let view2 = vec![CellView {
            cell: 2,
            width_px: 10.0,
            dist: 0.0,
            ring: false,
        }];
        cache.launch_jobs(&cs, &view2, 8);
        assert!(cache.entry(1).near_page.is_none());
        assert_eq!(cache.pools().available(256), 8);
        assert_eq!(cache.pools().available(512), 2);
        assert_eq!(cache.entry(1).drawn_page().map(|(p, _)| p.size), Some(64));
    }

    /// The cells with nothing to draw take the budget first (the ring of a moving view always has some),
    /// yet the widest cell in view that wants a bigger page gets one each frame.
    #[test]
    fn a_page_grows_each_frame_even_when_the_holes_take_the_budget() {
        let cs = cells(6);
        let mut cache = TerrainCache::new(6, &settings([16, 8, 8, 8]), 64, true);
        for _ in 0..4 {
            cache.launch_jobs(&cs, &view(&[200.0, 200.0]), 8);
            drain(&mut cache);
        }
        assert_eq!(cache.entry(1).near_page.map(|p| p.size), Some(256));
        // Two cells in view grow; four ring cells without a page take the budget of one.
        let mut v = view(&[3000.0, 1000.0]);
        v.extend((2..6).map(|cell| CellView {
            cell,
            width_px: 0.0,
            dist: 10.0,
            ring: true,
        }));
        cache.launch_jobs(&cs, &v, 1);
        let near: Vec<usize> = cache
            .jobs()
            .filter(|j| j.kind == PageKind::Near)
            .map(|j| j.cell)
            .collect();
        assert_eq!(near, [0], "the widest grows first");
        assert_eq!(
            cache.jobs().filter(|j| j.kind == PageKind::Cache).count(),
            1
        );
        drain(&mut cache);
        cache.launch_jobs(&cs, &v, 1);
        assert!(
            cache
                .jobs()
                .any(|j| j.kind == PageKind::Near && j.cell == 1)
        );
    }
}
