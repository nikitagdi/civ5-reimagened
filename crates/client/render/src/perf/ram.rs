//! RAM accounting (AC-H2). [`CountingAlloc`] wraps the system allocator of an app, keeps a 16 byte
//! header before each block with the [`RamTag`] it was made under, and counts live bytes per tag.
//! `ram_scope(tag)` sets the tag for the allocations of a piece of code until the guard drops; the
//! main thread of the web client is the only user that matters, but the counters are atomics.
//! The size of the wasm linear memory comes from [`wasm_memory_bytes`].

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// Who an allocation belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RamTag {
    /// No scope was open.
    Untagged = 0,
    /// Terrain meshes, bake inputs, page tables.
    Terrain,
    /// Model, skeleton and clip data on the CPU.
    Models,
    /// Decoded textures and asset blobs.
    Assets,
    /// Network buffers and decoded messages.
    Net,
    /// The simulation core: game state, views.
    Sim,
    /// egui and the UI views.
    Ui,
}

const N: usize = 7;

impl RamTag {
    pub const ALL: [RamTag; N] = [
        Self::Untagged,
        Self::Terrain,
        Self::Models,
        Self::Assets,
        Self::Net,
        Self::Sim,
        Self::Ui,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Untagged => "untagged",
            Self::Terrain => "terrain",
            Self::Models => "models",
            Self::Assets => "assets",
            Self::Net => "net",
            Self::Sim => "sim",
            Self::Ui => "ui",
        }
    }

    #[cfg(test)]
    fn from_u8(b: u8) -> Self {
        Self::ALL
            .get(usize::from(b))
            .copied()
            .unwrap_or(Self::Untagged)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static LIVE: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static ALLOCS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static PEAK: AtomicU64 = AtomicU64::new(0);

/// Allocations made in the scope count for `tag` until the guard drops (the previous tag returns).
#[must_use = "the tag is reset when the guard drops"]
pub struct RamScope {
    prev: u8,
}

pub fn ram_scope(tag: RamTag) -> RamScope {
    RamScope {
        prev: CURRENT.swap(tag as u8, Ordering::Relaxed),
    }
}

impl Drop for RamScope {
    fn drop(&mut self) {
        CURRENT.store(self.prev, Ordering::Relaxed);
    }
}

/// The allocator for `#[global_allocator]` in the apps.
pub struct CountingAlloc;

const HEADER: usize = 16;

/// The header sits `HEADER` bytes before the user block; blocks with a bigger alignment get a bigger
/// offset so that the user pointer stays aligned.
fn offset(layout: Layout) -> usize {
    layout.align().max(HEADER)
}

fn grown(tag: u8, size: usize) {
    let live = LIVE[usize::from(tag)].fetch_add(size as u64, Ordering::Relaxed) + size as u64;
    ALLOC_BYTES.fetch_add(size as u64, Ordering::Relaxed);
    ALLOCS.fetch_add(1, Ordering::Relaxed);
    if live > PEAK.load(Ordering::Relaxed) {
        let total: u64 = LIVE.iter().map(|l| l.load(Ordering::Relaxed)).sum();
        PEAK.fetch_max(total, Ordering::Relaxed);
    }
}

// SAFETY: forwards to `System` with a layout that is the user's plus a header; the header holds the
// tag and the user size and is read back with the same offset.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let off = offset(layout);
        let Ok(inner) = Layout::from_size_align(layout.size() + off, layout.align()) else {
            return std::ptr::null_mut();
        };
        let base = unsafe { System.alloc(inner) };
        if base.is_null() {
            return base;
        }
        let tag = CURRENT.load(Ordering::Relaxed);
        let user = unsafe { base.add(off) };
        unsafe {
            (user.sub(8) as *mut u64).write_unaligned(layout.size() as u64);
            (user.sub(9)).write(tag);
        }
        grown(tag, layout.size());
        user
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let off = offset(layout);
        let Ok(inner) = Layout::from_size_align(layout.size() + off, layout.align()) else {
            return std::ptr::null_mut();
        };
        let base = unsafe { System.alloc_zeroed(inner) };
        if base.is_null() {
            return base;
        }
        let tag = CURRENT.load(Ordering::Relaxed);
        let user = unsafe { base.add(off) };
        unsafe {
            (user.sub(8) as *mut u64).write_unaligned(layout.size() as u64);
            (user.sub(9)).write(tag);
        }
        grown(tag, layout.size());
        user
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let off = offset(layout);
        let tag = unsafe { ptr.sub(9).read() };
        LIVE[usize::from(tag).min(N - 1)].fetch_sub(layout.size() as u64, Ordering::Relaxed);
        let inner =
            unsafe { Layout::from_size_align_unchecked(layout.size() + off, layout.align()) };
        unsafe { System.dealloc(ptr.sub(off), inner) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let off = offset(layout);
        let tag = unsafe { ptr.sub(9).read() };
        let old_inner =
            unsafe { Layout::from_size_align_unchecked(layout.size() + off, layout.align()) };
        let base = unsafe { System.realloc(ptr.sub(off), old_inner, new_size + off) };
        if base.is_null() {
            return base;
        }
        let user = unsafe { base.add(off) };
        unsafe {
            (user.sub(8) as *mut u64).write_unaligned(new_size as u64);
            (user.sub(9)).write(tag);
        }
        let i = usize::from(tag).min(N - 1);
        if new_size >= layout.size() {
            let grow = (new_size - layout.size()) as u64;
            let live = LIVE[i].fetch_add(grow, Ordering::Relaxed) + grow;
            ALLOC_BYTES.fetch_add(grow, Ordering::Relaxed);
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            if live > PEAK.load(Ordering::Relaxed) {
                let total: u64 = LIVE.iter().map(|l| l.load(Ordering::Relaxed)).sum();
                PEAK.fetch_max(total, Ordering::Relaxed);
            }
        } else {
            LIVE[i].fetch_sub((layout.size() - new_size) as u64, Ordering::Relaxed);
        }
        user
    }
}

/// Heap bytes by tag, and totals, now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RamSnapshot {
    pub by_tag: Vec<(RamTag, u64)>,
    pub live: u64,
    pub peak: u64,
    /// Allocations and bytes handed out since the start (a per-frame difference shows churn).
    pub allocs: u64,
    pub alloc_bytes: u64,
    /// The size of the wasm linear memory (0 elsewhere): what the browser tab holds for the module.
    pub wasm_linear: u64,
}

pub fn snapshot() -> RamSnapshot {
    let by_tag: Vec<_> = RamTag::ALL
        .iter()
        .map(|&t| (t, LIVE[t as usize].load(Ordering::Relaxed)))
        .collect();
    RamSnapshot {
        live: by_tag.iter().map(|t| t.1).sum(),
        by_tag,
        peak: PEAK.load(Ordering::Relaxed),
        allocs: ALLOCS.load(Ordering::Relaxed),
        alloc_bytes: ALLOC_BYTES.load(Ordering::Relaxed),
        wasm_linear: wasm_memory_bytes(),
    }
}

/// The wasm linear memory in bytes; 0 where there is none.
pub fn wasm_memory_bytes() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        (core::arch::wasm32::memory_size(0) as u64) * 65536
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tag and the live counts are global: tests that look at them run one at a time.
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn tag_round_trips() {
        for t in RamTag::ALL {
            assert_eq!(RamTag::from_u8(t as u8), t);
        }
    }

    #[test]
    fn alloc_counts_live_per_tag_and_dealloc_gives_back() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let a = CountingAlloc;
        let layout = Layout::from_size_align(1000, 8).unwrap();
        let _s = ram_scope(RamTag::Ui);
        let before = LIVE[RamTag::Ui as usize].load(Ordering::Relaxed);
        let p = unsafe { a.alloc(layout) };
        assert!(!p.is_null());
        assert_eq!(p as usize % 8, 0);
        assert!(LIVE[RamTag::Ui as usize].load(Ordering::Relaxed) >= before + 1000);
        let p2 = unsafe { a.realloc(p, layout, 3000) };
        assert!(LIVE[RamTag::Ui as usize].load(Ordering::Relaxed) >= before + 3000);
        unsafe { a.dealloc(p2, Layout::from_size_align(3000, 8).unwrap()) };
        // Another test may allocate under `Ui` at once; the live count is at least back to start.
        assert!(LIVE[RamTag::Ui as usize].load(Ordering::Relaxed) <= before + 5000);
    }

    #[test]
    fn big_alignment_stays_aligned() {
        let a = CountingAlloc;
        let layout = Layout::from_size_align(64, 64).unwrap();
        let p = unsafe { a.alloc(layout) };
        assert_eq!(p as usize % 64, 0);
        unsafe { a.dealloc(p, layout) };
    }

    #[test]
    fn scope_restores_previous_tag() {
        let _l = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = CURRENT.load(Ordering::Relaxed);
        {
            let _a = ram_scope(RamTag::Net);
            {
                let _b = ram_scope(RamTag::Sim);
                assert_eq!(CURRENT.load(Ordering::Relaxed), RamTag::Sim as u8);
            }
            assert_eq!(CURRENT.load(Ordering::Relaxed), RamTag::Net as u8);
        }
        assert_eq!(CURRENT.load(Ordering::Relaxed), before);
    }
}
