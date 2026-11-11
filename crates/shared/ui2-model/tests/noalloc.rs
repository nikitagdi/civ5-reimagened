//! A frame without writes costs nothing: `take_frame` returns `None` without allocating.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use ui2_model::generated as g;
use ui2_model::{ModelStore, Owner, Schema};

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to the system allocator and only bumps a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn take_frame_without_puts_does_not_allocate() {
    let mut st = ModelStore::new(Schema::v1());
    st.writer(Owner::Source).put(g::Top::default()).unwrap();
    assert!(st.take_frame().is_some());

    let before = ALLOCS.with(Cell::get);
    for _ in 0..10_000 {
        assert!(st.take_frame().is_none());
    }
    let after = ALLOCS.with(Cell::get);
    assert_eq!(after - before, 0, "take_frame allocated on an idle frame");

    // The counter works: a put that changes something does allocate.
    let t = g::Top {
        gold: 1,
        ..Default::default()
    };
    let before = ALLOCS.with(Cell::get);
    st.writer(Owner::Source).put(t).unwrap();
    assert!(st.take_frame().is_some());
    assert!(ALLOCS.with(Cell::get) > before);
}
