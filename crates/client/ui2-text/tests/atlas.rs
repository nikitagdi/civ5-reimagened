//! The shelf packer: placement, margins, dirty rectangles, cache, overflow.

mod common;

use common::*;
use ui2_text::{
    AtlasConfig, AtlasError, AtlasSlot, DirtyRect, FaceId, GlyphAtlas, GlyphBitmap, GlyphKey,
    RunStyle,
};

fn small(page_size: u32, max_pages: u32) -> GlyphAtlas {
    GlyphAtlas::new(AtlasConfig {
        page_size,
        max_pages,
        padding: 1,
        shelf_quantum: 4,
    })
}

fn bitmap(w: u32, h: u32, fill: u8) -> GlyphBitmap {
    GlyphBitmap {
        w,
        h,
        left: -1,
        top: h as i32,
        coverage: vec![fill; (w * h) as usize],
    }
}

fn key(glyph: u32) -> GlyphKey {
    GlyphKey::new(FaceId(0), glyph, 14.0, 400.0, 0)
}

/// A cell with its margin, for overlap tests.
fn cell(s: &AtlasSlot) -> (u32, u32, u32, u32, u32) {
    (s.page, s.x - 1, s.y - 1, s.x + s.w + 1, s.y + s.h + 1)
}

fn overlap(a: (u32, u32, u32, u32, u32), b: (u32, u32, u32, u32, u32)) -> bool {
    a.0 == b.0 && a.1 < b.3 && b.1 < a.3 && a.2 < b.4 && b.2 < a.4
}

#[test]
fn packed_glyphs_do_not_overlap_and_keep_their_pixels() {
    let mut atlas = small(128, 3);
    let mut seed = 12345u32;
    let mut next = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        seed >> 16
    };
    let mut placed = Vec::new();
    for i in 0..150u32 {
        let (w, h) = (3 + next() % 14, 4 + next() % 20);
        let fill = 1 + (i % 250) as u8;
        match atlas.insert(key(i), &bitmap(w, h, fill)) {
            Ok(slot) => placed.push((slot, w, h, fill)),
            Err(e) => assert_eq!(e, AtlasError::Full),
        }
    }
    assert!(placed.len() > 100, "{} placed", placed.len());
    let size = atlas.config().page_size;
    for (i, (s, w, h, fill)) in placed.iter().enumerate() {
        assert_eq!((s.w, s.h), (*w, *h));
        assert!(
            s.x >= 1 && s.y >= 1 && s.x + s.w < size && s.y + s.h < size,
            "{s:?}"
        );
        for (t, ..) in &placed[i + 1..] {
            assert!(!overlap(cell(s), cell(t)), "{s:?} overlaps {t:?}");
        }
        // The pixels are there and the margin is zero.
        let page = atlas.page(s.page).unwrap();
        for y in s.y - 1..s.y + s.h + 1 {
            for x in s.x - 1..s.x + s.w + 1 {
                let inside = x >= s.x && x < s.x + s.w && y >= s.y && y < s.y + s.h;
                let v = page[(y * size + x) as usize];
                assert_eq!(
                    v,
                    if inside { *fill } else { 0 },
                    "pixel ({x},{y}) of {s:?}"
                );
            }
        }
    }
}

#[test]
fn shelves_are_multiples_of_four_and_reused() {
    let mut atlas = small(64, 1);
    // Cells of height 7 + 2 = 9 -> shelf of 12; two glyphs side by side on one shelf.
    let a = atlas.insert(key(1), &bitmap(5, 7, 9)).unwrap();
    let b = atlas.insert(key(2), &bitmap(5, 7, 9)).unwrap();
    assert_eq!(a.y, b.y);
    assert_eq!(b.x, a.x + 5 + 2);
    // A taller glyph starts a new shelf below the first (12 px high).
    let c = atlas.insert(key(3), &bitmap(5, 20, 9)).unwrap();
    assert_eq!(c.y, 12 + 1);
    // A short glyph goes back to the 12 px shelf rather than the 24 px one.
    let d = atlas.insert(key(4), &bitmap(5, 5, 9)).unwrap();
    assert_eq!(d.y, a.y);
}

#[test]
fn dirty_rects_cover_every_new_glyph_and_merge_neighbours() {
    let mut atlas = small(64, 2);
    assert!(!atlas.has_dirty());
    let a = atlas.insert(key(1), &bitmap(5, 7, 9)).unwrap();
    let b = atlas.insert(key(2), &bitmap(6, 7, 9)).unwrap();
    let c = atlas.insert(key(3), &bitmap(5, 30, 9)).unwrap();
    let dirty = atlas.take_dirty();
    assert_eq!(dirty.len(), 2, "a and b touch on one shelf: {dirty:?}");
    for s in [a, b, c] {
        let (x0, y0, x1, y1) = (s.x - 1, s.y - 1, s.x + s.w + 1, s.y + s.h + 1);
        assert!(
            dirty.iter().any(|r| r.page == s.page
                && r.x <= x0
                && r.y <= y0
                && r.x + r.w >= x1
                && r.y + r.h >= y1),
            "{s:?} not covered by {dirty:?}"
        );
    }
    assert!(atlas.take_dirty().is_empty());
    // copy_rect returns the page pixels of the rectangle, tightly packed.
    let r = dirty[0];
    let px = atlas.copy_rect(&r);
    assert_eq!(px.len(), (r.w * r.h) as usize);
    let size = atlas.config().page_size;
    let first = atlas.page(r.page).unwrap()[(r.y * size + r.x) as usize];
    assert_eq!(px[0], first);
    assert!(px.contains(&9));
}

#[test]
fn zero_size_glyphs_take_no_room() {
    let mut atlas = small(32, 1);
    let s = atlas
        .insert(
            key(1),
            &GlyphBitmap {
                left: 2,
                top: 3,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!((s.w, s.h, s.left, s.top), (0, 0, 2, 3));
    assert!(!atlas.has_dirty());
    assert_eq!(atlas.stats().pages, 0);
}

#[test]
fn full_and_too_large_are_reported_and_clear_recovers() {
    let mut atlas = small(32, 1);
    assert_eq!(
        atlas.insert(key(1), &bitmap(31, 4, 1)).unwrap_err(),
        AtlasError::TooLarge
    );
    let mut n = 0;
    loop {
        match atlas.insert(key(100 + n), &bitmap(10, 10, 7)) {
            Ok(_) => n += 1,
            Err(e) => {
                assert_eq!(e, AtlasError::Full);
                break;
            }
        }
    }
    assert_eq!(
        n, 4,
        "two 12-high shelves of two 12 px cells fit a 32 px page"
    );
    atlas.take_dirty();
    atlas.clear();
    assert_eq!(atlas.stats().glyphs, 0);
    assert!(atlas.page(0).unwrap().iter().all(|&v| v == 0));
    // The used area is reported so the uploaded copy gets cleared too.
    assert_eq!(
        atlas.take_dirty(),
        vec![DirtyRect {
            page: 0,
            x: 0,
            y: 0,
            w: 32,
            h: 24
        }]
    );
    atlas.insert(key(100), &bitmap(10, 10, 7)).unwrap();
}

#[test]
fn get_or_raster_caches_by_key() {
    let mut e = engine();
    let mut atlas = GlyphAtlas::new(AtlasConfig {
        page_size: 128,
        ..Default::default()
    });
    let run = e
        .shape(&RunStyle::new(FIRA_400, 14.0), "Education")
        .unwrap();
    let keys: Vec<GlyphKey> = run
        .glyphs
        .iter()
        .map(|g| GlyphKey::new(FIRA_400, g.glyph_id, 14.0, 400.0, 0))
        .collect();
    let slots: Vec<AtlasSlot> = keys
        .iter()
        .map(|k| atlas.get_or_raster(&e, *k).unwrap())
        .collect();
    let unique = {
        let mut u = keys.clone();
        u.sort_by_key(|k| k.glyph);
        u.dedup();
        u.len()
    };
    let st = atlas.stats();
    assert_eq!(
        (st.glyphs, st.misses as usize, st.hits as usize),
        (unique, unique, keys.len() - unique)
    );
    assert!(slots.iter().all(|s| s.w > 0 && s.h > 0));
    // The same key gives the same slot; a different subpixel step is another glyph.
    assert_eq!(atlas.get_or_raster(&e, keys[0]).unwrap(), slots[0]);
    let other = atlas
        .get_or_raster(
            &e,
            GlyphKey::new(FIRA_400, run.glyphs[0].glyph_id, 14.0, 400.0, 2),
        )
        .unwrap();
    assert_ne!(other, slots[0]);
    // The rasterized pixels are in the page.
    let bmp = e.raster_glyph(keys[0]);
    let page = atlas.page(slots[0].page).unwrap();
    let size = atlas.config().page_size;
    for row in 0..bmp.h {
        let at = ((slots[0].y + row) * size + slots[0].x) as usize;
        assert_eq!(
            &page[at..at + bmp.w as usize],
            &bmp.coverage[(row * bmp.w) as usize..((row + 1) * bmp.w) as usize]
        );
    }
}
