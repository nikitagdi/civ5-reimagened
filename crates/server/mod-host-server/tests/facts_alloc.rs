//! T22: a call of a mod reads the facts without allocating (D7, "no allocations on the hot path"). A counting global
//! allocator counts the allocations of this thread around the reads.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use mod_api::{MapShape, Owner, PlotClass, Seat, WorldFacts};
use mod_host_server::WorldTable;
use protocol::{MapBasis, OwnersUpdate, PlotBasisView};

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: forwards to the system allocator; the counter is a plain thread-local cell without a destructor.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[test]
fn reading_the_facts_allocates_nothing() {
    let mut t = WorldTable::new();
    t.restart(
        MapShape {
            width: 128,
            height: 80,
            wrap_x: true,
            wrap_y: false,
        },
        3,
    );
    t.set_human_mask(0b11);
    let n = 128 * 80;
    t.set_basis(&MapBasis {
        width: 128,
        height: 80,
        plots: vec![PlotBasisView::default(); n],
        ..MapBasis::default()
    });
    let mut owners = vec![-1i8; n];
    owners[5] = 1;
    owners[6] = 9;
    t.apply_owners(&OwnersUpdate::Full {
        rev: 1,
        turn: 3,
        owners,
    });
    t.apply_owners(&OwnersUpdate::Delta {
        base_rev: 1,
        rev: 2,
        changes: vec![(7, 0)],
    });

    let before = ALLOCS.with(Cell::get);
    let mut seen = 0u32;
    let f: &dyn WorldFacts = &t;
    for y in -2..82i16 {
        for x in -3..131i16 {
            if let Some(p) = f.plot_at(x, y) {
                if f.owner(p) != Owner::None {
                    seen += 1;
                }
                if f.class(p) == PlotClass::Land {
                    seen += 1;
                }
                let _ = f.owned_by(p, Seat(1));
                let _ = f.plot_xy(p);
            }
        }
    }
    let _ = (f.map(), f.turn(), f.seat_player(Seat(0)));
    let after = ALLOCS.with(Cell::get);
    assert!(seen > 10_000);
    assert_eq!(f.owner(5), Owner::Seat(Seat(1)));
    assert_eq!(f.owner(6), Owner::Npc(9));
    assert_eq!(f.owner(7), Owner::Seat(Seat(0)));
    assert_eq!(after, before, "the reads allocated");
}
