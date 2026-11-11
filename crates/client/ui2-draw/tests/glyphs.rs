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

    let atlas = s.glyph_atlas();
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
    assert_eq!(s.glyph_atlas().stats().glyphs, 40);
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
    assert_eq!(
        s.glyph_atlas().stats().glyphs,
        4,
        "one raster per distinct key"
    );
    assert_eq!(s.stats().raster_glyphs, 4);

    s.take_glyph_dirty();
    s.set_scale(2.0);
    assert_eq!(s.glyph_atlas().stats().glyphs, 0);
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
fn a_full_atlas_drops_glyphs_without_retrying() {
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
    let dropped = keys
        .iter()
        .find(|k| store.atlas().get(k).is_none())
        .expect("a dropped glyph");
    assert_eq!(store.lookup(*dropped), Lookup::Unavailable);
    assert_eq!(store.pending_len(), 0);
    store.clear();
    assert_eq!(store.lookup(*dropped), Lookup::Pending);
    // Use the engine mutably once, as the scene's resolve does.
    let _ = text.shape(&RunStyle::new(FIRA, 14.0), "x");
}
