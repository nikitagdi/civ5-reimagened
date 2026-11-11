//! Glyph placement, the rasterization budget and the atlas, with the vendored Fira Sans.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use ui2_draw::glyphs::{GlyphStore, Lookup, glyph_key};
use ui2_draw::{
    ChunkBuilder, ChunkId, Color, FaceId, GlyphPos, Glyphs, Item, LayerChange, LayerId, Scene,
};
use ui2_text::{AtlasConfig, GlyphKey, RunStyle, TextEngine, quantize_x};

const FIRA: FaceId = FaceId(0);
const WHITE: Color = Color::rgba(255, 255, 255, 255);

fn engine() -> TextEngine {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut e = TextEngine::new();
    e.load_face(FIRA, Arc::from(bytes)).expect("Fira Sans");
    e
}

fn run_item(text: &mut TextEngine, s: &str, k: f32, x: f32, y: f32, color: Color) -> Glyphs {
    let run = text.shape(&RunStyle::new(FIRA, 14.0), s).expect("shape");
    Glyphs::from_run(&run, k, x, y, color)
}

fn chunk_of(g: Glyphs) -> ui2_draw::ChunkItems {
    let mut b = ChunkBuilder::new();
    b.push(Item::Glyphs(g));
    b.build()
}

/// A clock that moves by `step` ms on every read.
fn ticking(step: f64) -> ui2_draw::Clock {
    let t = Rc::new(Cell::new(0.0));
    Box::new(move || {
        t.set(t.get() + step);
        t.get()
    })
}

#[test]
fn education_is_placed_where_shaping_says() {
    let mut text = engine();
    let g = run_item(&mut text, "Education", 1.0, 10.25, 30.0, WHITE);
    assert_eq!(g.glyphs.len(), 9);
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), chunk_of(g.clone()));

    // Before resolve: one zero-area placeholder per glyph, all of them queued.
    assert_eq!(s.layer_instances(LayerId::Hud).len(), 9);
    assert!(
        s.layer_instances(LayerId::Hud)
            .iter()
            .all(|i| i.rect == [0.0; 4])
    );
    assert_eq!(s.chunk_unresolved(ChunkId(1)), Some(9));
    s.take_layer_change(LayerId::Hud);
    let rev = s.layer_revision(LayerId::Hud);

    s.resolve(&mut text, 1000.0);
    assert_eq!(s.stats().raster_deferred, 0);
    assert_eq!(s.chunk_unresolved(ChunkId(1)), Some(0));
    // Same instance count: the layer is patched in place.
    assert_eq!(s.layer_revision(LayerId::Hud), rev + 1);
    assert_eq!(
        s.take_layer_change(LayerId::Hud),
        LayerChange::Ranges(std::iter::once(0..9).collect())
    );

    let atlas = s.glyphs();
    let page = atlas.page(0).expect("page 0");
    let size = atlas.config().page_size as usize;
    let mut total_ink = 0u64;
    for (pos, inst) in g.glyphs.iter().zip(s.layer_instances(LayerId::Hud)) {
        // The pen position of the shaped glyph, split into a whole pixel and a quarter.
        let (px, sub) = quantize_x(pos.x);
        let key = GlyphKey::new(FIRA, pos.glyph, 14.0, 400.0, sub);
        let slot = atlas.get(&key).expect("glyph in atlas");
        assert!(
            slot.w > 0 && slot.h > 0,
            "every letter of Education has ink"
        );
        let baseline = pos.y.round() as i32;
        assert_eq!(
            inst.rect,
            [
                (px + slot.left) as f32,
                (baseline - slot.top) as f32,
                slot.w as f32,
                slot.h as f32
            ]
        );
        assert_eq!(
            inst.uv,
            [
                slot.x as f32 / 2048.0,
                slot.y as f32 / 2048.0,
                (slot.x + slot.w) as f32 / 2048.0,
                (slot.y + slot.h) as f32 / 2048.0
            ]
        );
        assert_eq!(inst.atlas_layer, 0);
        assert_eq!(inst.colors[0], WHITE.0);
        // Coverage under the instance's texels is not empty.
        let ink: u64 = (0..slot.h as usize)
            .flat_map(|r| {
                let at = (slot.y as usize + r) * size + slot.x as usize;
                page[at..at + slot.w as usize].iter().map(|v| u64::from(*v))
            })
            .sum();
        assert!(ink > 0);
        total_ink += ink;
    }
    assert!(
        total_ink > 9 * 255,
        "text has real coverage, got {total_ink}"
    );

    // The whole word sits on the baseline pixel row 30: every glyph box bottom is at or below it by at most the
    // descender, and the pen origins follow the shaped advances (consecutive glyphs move right).
    let xs: Vec<f32> = s
        .layer_instances(LayerId::Hud)
        .iter()
        .map(|i| i.rect[0])
        .collect();
    assert!(xs.windows(2).all(|w| w[0] < w[1]), "{xs:?}");
    assert!(!s.take_glyph_dirty().is_empty());
    assert!(s.take_glyph_dirty().is_empty());
}

#[test]
fn quantization_of_pen_positions() {
    let g = Glyphs {
        face: FIRA,
        size_px: 14.0,
        wght: 400.0,
        color: WHITE,
        glyphs: vec![],
        transform: None,
        clip: None,
    };
    let at = |x: f32, y: f32| glyph_key(&g, &GlyphPos { glyph: 7, x, y });
    let (k, px, base) = at(10.0, 20.4);
    assert_eq!((k.subpx, px, base), (0, 10, 20));
    assert_eq!(at(10.12, 20.5).0.subpx, 0);
    assert_eq!(at(10.12, 20.5).2, 21);
    assert_eq!((at(10.13, 0.0).0.subpx, at(10.13, 0.0).1), (1, 10));
    assert_eq!((at(10.4, 0.0).0.subpx, at(10.4, 0.0).1), (2, 10));
    assert_eq!((at(10.9, 0.0).0.subpx, at(10.9, 0.0).1), (0, 11));
    assert_eq!((at(-0.3, 0.0).0.subpx, at(-0.3, 0.0).1), (3, -1));
    // Colour is not part of the key; size is, unquantized.
    let mut g2 = g.clone();
    g2.color = Color::rgba(1, 2, 3, 4);
    assert_eq!(
        glyph_key(
            &g2,
            &GlyphPos {
                glyph: 7,
                x: 10.0,
                y: 0.0
            }
        )
        .0,
        k
    );
    g2.size_px = 14.5;
    assert_ne!(
        glyph_key(
            &g2,
            &GlyphPos {
                glyph: 7,
                x: 10.0,
                y: 0.0
            }
        )
        .0,
        k
    );
}

fn many_glyphs(n: u32) -> ui2_draw::ChunkItems {
    chunk_of(Glyphs {
        face: FIRA,
        size_px: 14.0,
        wght: 400.0,
        color: WHITE,
        glyphs: (0..n)
            .map(|i| GlyphPos {
                glyph: 3 + i,
                x: 0.0,
                y: 20.0,
            })
            .collect(),
        transform: None,
        clip: None,
    })
}

#[test]
fn rasterization_keeps_to_the_budget_and_finishes_over_frames() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_clock(ticking(0.5));
    s.set_chunk(LayerId::Hud, ChunkId(1), many_glyphs(40));
    assert_eq!(s.stats().raster_deferred, 40);

    // Every clock read moves 0.5 ms: with 1.5 ms the step handles the first glyph and then two more.
    s.resolve(&mut text, 1.5);
    assert_eq!(s.stats().raster_glyphs, 3);
    assert_eq!(s.stats().raster_deferred, 37);
    assert!(s.stats().raster_ms > 0.0);

    let mut frames = 1;
    while s.stats().raster_deferred > 0 {
        s.reset_frame_stats();
        s.resolve(&mut text, 1.5);
        assert!(s.stats().raster_glyphs <= 3);
        frames += 1;
        assert!(frames < 40, "no progress");
    }
    assert_eq!(frames, 14); // ceil(40 / 3)
    assert_eq!(s.chunk_unresolved(ChunkId(1)), Some(0));
    assert_eq!(s.glyphs().glyph_count(), 40);
}

#[test]
fn at_least_one_glyph_per_call_even_with_no_budget() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_clock(ticking(10.0));
    s.set_chunk(LayerId::Hud, ChunkId(1), many_glyphs(5));
    s.resolve(&mut text, 0.0);
    assert_eq!(s.stats().raster_glyphs, 1);
    assert_eq!(s.stats().raster_deferred, 4);
}

#[test]
fn without_a_clock_the_budget_is_a_glyph_count() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), many_glyphs(400));
    s.resolve(&mut text, 1.5);
    let done = s.stats().raster_glyphs;
    assert!((249..=251).contains(&done), "{done}");
    assert_eq!(s.stats().raster_deferred, 400 - done);
}

#[test]
fn recolouring_text_does_not_rasterize_again() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk_of(run_item(&mut text, "Hover", 1.0, 4.0, 20.0, WHITE)),
    );
    s.resolve(&mut text, 1000.0);
    s.take_layer_change(LayerId::Hud);
    s.take_glyph_dirty();
    s.reset_frame_stats();

    let gold = Color::rgba(240, 200, 80, 255);
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk_of(run_item(&mut text, "Hover", 1.0, 4.0, 20.0, gold)),
    );
    s.resolve(&mut text, 1000.0);
    assert_eq!(s.stats().raster_glyphs, 0);
    assert!(s.take_glyph_dirty().is_empty());
    assert_eq!(
        s.take_layer_change(LayerId::Hud),
        LayerChange::Ranges(std::iter::once(0..5).collect())
    );
    assert!(
        s.layer_instances(LayerId::Hud)
            .iter()
            .all(|i| i.colors[0] == gold.0)
    );
}

#[test]
fn a_space_keeps_its_instance_but_draws_nothing() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk_of(run_item(&mut text, "a b", 1.0, 0.0, 20.0, WHITE)),
    );
    s.resolve(&mut text, 1000.0);
    let inst = s.layer_instances(LayerId::Hud);
    assert_eq!(inst.len(), 3);
    assert_ne!(inst[0].rect, [0.0; 4]);
    assert_eq!(inst[1].rect, [0.0; 4], "space");
    assert_ne!(inst[2].rect, [0.0; 4]);
}

#[test]
fn glyphs_are_shared_between_chunks_and_scale_change_rerasterizes() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk_of(run_item(&mut text, "Gold", 1.0, 0.0, 20.0, WHITE)),
    );
    s.set_chunk(
        LayerId::Window,
        ChunkId(2),
        chunk_of(run_item(&mut text, "Gold", 1.0, 0.0, 40.0, WHITE)),
    );
    s.resolve(&mut text, 1000.0);
    assert_eq!(s.glyphs().glyph_count(), 4, "one raster per distinct key");
    assert_eq!(s.stats().raster_glyphs, 4);

    s.take_glyph_dirty();
    s.set_scale(2.0);
    assert_eq!(s.glyphs().glyph_count(), 0);
    // The cleared pages are reported dirty so that the GPU copy is zeroed.
    assert!(!s.take_glyph_dirty().is_empty());
    // The runtime sends the chunks again for the new factor.
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk_of(run_item(&mut text, "Gold", 2.0, 0.0, 40.0, WHITE)),
    );
    s.set_chunk(
        LayerId::Window,
        ChunkId(2),
        chunk_of(run_item(&mut text, "Gold", 2.0, 0.0, 80.0, WHITE)),
    );
    s.reset_frame_stats();
    s.resolve(&mut text, 1000.0);
    assert_eq!(s.stats().raster_glyphs, 4);
    for l in [LayerId::Hud, LayerId::Window] {
        assert!(s.layer_instances(l).iter().all(|i| i.rect[2] > 0.0));
    }
}

#[test]
fn a_stale_chunk_after_a_scale_change_is_encoded_again_by_resolve() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        chunk_of(run_item(&mut text, "Ab", 1.0, 0.0, 20.0, WHITE)),
    );
    s.resolve(&mut text, 1000.0);
    s.set_scale(1.5);
    s.reset_frame_stats();
    // The runtime did not resend the chunk: its glyphs are rasterized again anyway.
    s.resolve(&mut text, 1000.0);
    assert_eq!(s.stats().raster_glyphs, 2);
    assert!(
        s.layer_instances(LayerId::Hud)
            .iter()
            .all(|i| i.rect[2] > 0.0)
    );
}

#[test]
fn a_full_atlas_without_an_old_page_defers_glyphs_and_retries_them_later() {
    let mut text = engine();
    let mut store = GlyphStore::new(AtlasConfig {
        page_size: 64,
        max_pages: 1,
        padding: 1,
        shelf_quantum: 4,
    });
    let keys: Vec<GlyphKey> = (3..40)
        .map(|g| GlyphKey::new(FIRA, g, 30.0, 400.0, 0))
        .collect();
    for k in &keys {
        assert_eq!(store.lookup(*k), Lookup::Pending);
    }
    assert_eq!(store.lookup(keys[0]), Lookup::Pending, "queued once");
    assert_eq!(store.pending_len(), keys.len());
    let report = store.rasterize(&text, |_| true);
    assert!(report.rasterized > 0);
    assert!(report.dropped > 0);
    assert_eq!(report.rasterized + report.dropped, keys.len() as u32);
    assert_eq!(report.deferred, 0);
    assert_eq!(store.full_count(), u64::from(report.dropped));
    assert!(store.take_full_error(), "reported once");
    assert!(!store.take_full_error());
    // The only page was used in this frame: nothing could be cleared.
    assert_eq!(store.evictions(), 0);
    let dropped = keys
        .iter()
        .find(|k| store.get(k).is_none())
        .expect("a dropped glyph");
    // It waits for room without being queued again on every lookup.
    assert_eq!(store.lookup(*dropped), Lookup::Pending);
    assert_eq!(store.pending_len(), 0);
    // Two frames later it is queued again, and the old page makes room for it.
    store.begin_frame();
    assert_eq!(store.pending_len(), 0);
    store.begin_frame();
    assert_eq!(store.pending_len(), report.dropped as usize);
    let again = store.rasterize(&text, |_| true);
    assert!(again.evicted >= 1, "{again:?}");
    assert_eq!(again.live_page, None, "no chunk drew from the page");
    assert!(again.rasterized > 0, "{again:?}");
    assert!(
        store.get(&keys[0]).is_none(),
        "the first glyphs went with the page"
    );
    store.clear();
    assert_eq!(store.lookup(*dropped), Lookup::Pending);
    // Use the engine mutably once, as the scene's resolve does.
    let _ = text.shape(&RunStyle::new(FIRA, 14.0), "x");
}

fn sized_run(text: &mut TextEngine, s: &str, size: f32, x: f32, y: f32) -> Glyphs {
    let run = text.shape(&RunStyle::new(FIRA, size), s).expect("shape");
    Glyphs::from_run(&run, 1.0, x, y, WHITE)
}

/// The GPU's copy of the glyph pages, kept up to date only with the rectangles the scene reports dirty (what the
/// pass uploads).
#[derive(Default)]
struct Mirror {
    pages: Vec<Vec<u8>>,
}

impl Mirror {
    fn sync(&mut self, s: &mut Scene) {
        let side = s.glyphs().page_size() as usize;
        while self.pages.len() < s.glyphs().page_count() {
            self.pages.push(vec![0; side * side]);
        }
        for r in s.take_glyph_dirty() {
            let px = s.glyphs().copy_rect(&r);
            let page = &mut self.pages[r.page as usize];
            for row in 0..r.h as usize {
                let at = (r.y as usize + row) * side + r.x as usize;
                page[at..at + r.w as usize]
                    .copy_from_slice(&px[row * r.w as usize..(row + 1) * r.w as usize]);
            }
        }
    }
}

/// Every glyph of `run` drawn by `id` is resolved, points at its slot, and the uploaded copy of the page holds
/// its coverage (checked against a fresh raster when `deep`).
fn assert_visible(
    s: &Scene,
    text: &TextEngine,
    mirror: &Mirror,
    id: ChunkId,
    run: &Glyphs,
    deep: bool,
) {
    assert_eq!(
        s.chunk_unresolved(id),
        Some(0),
        "chunk {id:?} waits for glyphs"
    );
    let inst = s.chunk_instances(id).expect("chunk");
    assert_eq!(inst.len(), run.glyphs.len());
    let side = s.glyphs().page_size() as usize;
    for (pos, i) in run.glyphs.iter().zip(inst) {
        let (key, _, _) = glyph_key(run, pos);
        let slot = s.glyphs().get(&key).expect("a drawn glyph is in the store");
        if slot.w == 0 {
            assert_eq!(i.rect, [0.0; 4]);
            continue;
        }
        assert_eq!(u32::from(i.atlas_layer), slot.page, "chunk {id:?}");
        let page = side as f32;
        assert_eq!(
            i.uv,
            [
                slot.x as f32 / page,
                slot.y as f32 / page,
                (slot.x + slot.w) as f32 / page,
                (slot.y + slot.h) as f32 / page
            ]
        );
        if deep {
            let want = text.raster_glyph(key);
            let copy = &mirror.pages[slot.page as usize];
            for row in 0..slot.h as usize {
                let at = (slot.y as usize + row) * side + slot.x as usize;
                assert_eq!(
                    &copy[at..at + slot.w as usize],
                    &want.coverage[row * want.w as usize..(row + 1) * want.w as usize],
                    "chunk {id:?}: coverage of glyph {} on the GPU copy",
                    pos.glyph
                );
            }
        }
    }
}

/// Eviction: 5000 different strings (each of its own size, so every one brings new glyph keys) pass through 40
/// chunks, two per frame, while two labels stay on screen the whole time. With pages of 256 px the store fills
/// its four pages again and again; after every frame every chunk shows all its glyphs, and the GPU copy (kept
/// with the dirty rectangles alone) holds their coverage.
#[test]
fn five_thousand_unique_strings_never_lose_visible_glyphs() {
    let mut text = engine();
    let mut s = Scene::with_atlas(
        1.0,
        AtlasConfig {
            page_size: 256,
            max_pages: 4,
            padding: 1,
            shelf_quantum: 4,
        },
    );
    let mut mirror = Mirror::default();
    let mut shown: std::collections::BTreeMap<ChunkId, Glyphs> = Default::default();
    for (id, label, y) in [(1, "Science +42", 20.0), (2, "Gold 125 (+7)", 40.0)] {
        let g = sized_run(&mut text, label, 14.0, 4.0, y);
        s.set_chunk(LayerId::Hud, ChunkId(id), chunk_of(g.clone()));
        shown.insert(ChunkId(id), g);
    }
    const SLOTS: u32 = 40;
    const PER_FRAME: u32 = 2;
    let mut frame = 0u32;
    for i in 0..5000u32 {
        let size = 10.0 + i as f32 * 0.0012;
        let g = sized_run(
            &mut text,
            &format!("Tile {i} yields {}", i * 7 % 13),
            size,
            0.0,
            30.0,
        );
        let id = ChunkId(100 + i % SLOTS);
        s.set_chunk(LayerId::Window, id, chunk_of(g.clone()));
        shown.insert(id, g);
        if i % PER_FRAME == PER_FRAME - 1 {
            s.resolve(&mut text, 1000.0);
            mirror.sync(&mut s);
            frame += 1;
            let deep = frame.is_multiple_of(50);
            for (id, g) in &shown {
                assert_visible(&s, &text, &mirror, *id, g, deep);
            }
            s.reset_frame_stats();
        }
    }
    let st = s.stats();
    assert_eq!(st.atlas_pages, 4);
    assert_eq!(st.atlas_full, 0, "a page was always free to clear");
    assert!(s.glyphs().evictions() >= 20, "{}", s.glyphs().evictions());
    assert!(
        s.glyphs().moved() > 0,
        "pages still in use were cleared and their glyphs moved"
    );
    for (id, g) in &shown {
        assert_visible(&s, &text, &mirror, *id, g, true);
    }
}

/// A frozen scene (reference images) shows all its text in one frame whatever the budget, and says the time it
/// is frozen at; thawed, the budget counts again. The solid background is kept opaque.
#[test]
fn a_frozen_scene_rasterizes_everything_at_once() {
    let mut text = engine();
    let mut s = Scene::new(1.0);
    s.set_clock(ticking(10.0));
    assert_eq!(s.frozen(), None);
    s.freeze(Some(1234.5));
    assert_eq!(s.frozen(), Some(1234.5));
    s.set_chunk(LayerId::Hud, ChunkId(1), many_glyphs(60));
    s.resolve(&mut text, 0.0);
    assert_eq!(s.stats().raster_glyphs, 60);
    assert_eq!(s.stats().raster_deferred, 0);
    assert_eq!(s.chunk_unresolved(ChunkId(1)), Some(0));

    s.freeze(None);
    s.freeze(Some(f64::NAN));
    assert_eq!(s.frozen(), None, "a time that is not finite thaws");
    s.reset_frame_stats();
    s.set_chunk(LayerId::Hud, ChunkId(2), many_glyphs(70));
    s.resolve(&mut text, 0.0);
    assert_eq!(
        s.stats().raster_glyphs,
        1,
        "thawed: one glyph for a zero budget"
    );

    assert_eq!(s.background(), None);
    s.set_background(Some(Color::rgba(32, 32, 32, 7)));
    assert_eq!(s.background(), Some(Color::rgba(32, 32, 32, 255)));
    s.set_background(None);
    assert_eq!(s.background(), None);
}
