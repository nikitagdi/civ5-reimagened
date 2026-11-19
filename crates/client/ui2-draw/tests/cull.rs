//! What a chunk builder may leave out (`Item::clipped_away`) and the text contrast of glyph runs.

use glam::{Affine2, Vec2};
use ui2_draw::glyphs::glyph_contrast;
use ui2_draw::{BoxItem, Clip, Color, Glyphs, Item, Rect, ShadowItem};

fn clip(x: f32, y: f32, w: f32, h: f32) -> Option<Clip> {
    Some(Clip {
        rect: Some(Rect::new(x, y, w, h)),
        radius: 0.0,
        polygon: None,
    })
}

fn boxed(rect: Rect, clip: Option<Clip>) -> Item {
    Item::Box(BoxItem {
        clip,
        ..BoxItem::solid(rect, Color::rgba(10, 20, 30, 255))
    })
}

#[test]
fn an_item_wholly_outside_its_clip_is_clipped_away() {
    let view = clip(0.0, 0.0, 100.0, 100.0);
    // Below, right of, above and left of the clip.
    for r in [
        Rect::new(10.0, 120.0, 20.0, 20.0),
        Rect::new(100.0, 10.0, 20.0, 20.0),
        Rect::new(10.0, -40.0, 20.0, 40.0),
        Rect::new(-30.0, 10.0, 30.0, 20.0),
    ] {
        assert!(boxed(r, view).clipped_away(), "{r:?}");
    }
    // Inside or across an edge: kept.
    for r in [
        Rect::new(10.0, 10.0, 20.0, 20.0),
        Rect::new(90.0, 90.0, 20.0, 20.0),
    ] {
        assert!(!boxed(r, view).clipped_away(), "{r:?}");
    }
    // Without a clip rectangle, or under a transform (its rectangle is not in chunk space), nothing is left out.
    assert!(!boxed(Rect::new(500.0, 500.0, 5.0, 5.0), None).clipped_away());
    let moved = Item::Box(BoxItem {
        transform: Some(Affine2::from_translation(Vec2::new(-500.0, -500.0))),
        clip: view,
        ..BoxItem::solid(Rect::new(500.0, 500.0, 5.0, 5.0), Color::rgba(0, 0, 0, 255))
    });
    assert!(!moved.clipped_away());
    // A clip that encloses nothing hides everything.
    assert!(boxed(Rect::new(0.0, 0.0, 5.0, 5.0), clip(0.0, 0.0, 0.0, 10.0)).clipped_away());
}

#[test]
fn shadows_and_glyph_runs_count_their_reach() {
    let view = clip(0.0, 0.0, 100.0, 100.0);
    // A box below the clip whose shadow, moved up and blurred, reaches into it: kept.
    let shadow = |offset_y: f32, blur: f32| {
        Item::Shadow(ShadowItem {
            rect: Rect::new(10.0, 110.0, 50.0, 20.0),
            offset: Vec2::new(0.0, offset_y),
            blur,
            color: Color::rgba(0, 0, 0, 128),
            clip: view,
            ..ShadowItem::default()
        })
    };
    assert!(!shadow(-12.0, 0.0).clipped_away());
    assert!(!shadow(0.0, 12.0).clipped_away());
    assert!(shadow(0.0, 2.0).clipped_away());
    // A run of glyphs: by its pen positions and its size (a glyph reaches above its baseline).
    let run = |baseline: f32| {
        Item::Glyphs(Glyphs {
            face: ui2_text::FaceId(0),
            size_px: 14.0,
            wght: 400.0,
            color: Color::rgba(255, 255, 255, 255),
            glyphs: vec![(5, 10.0, baseline).into(), (6, 18.0, baseline).into()],
            transform: None,
            clip: view,
        })
    };
    assert!(!run(50.0).clipped_away());
    assert!(
        !run(110.0).clipped_away(),
        "the ascenders reach into the clip"
    );
    assert!(run(140.0).clipped_away());
    assert!(run(-30.0).clipped_away());
}

#[test]
fn text_contrast_follows_the_luminance_and_darkens_small_sizes() {
    let white = Color::rgba(255, 255, 255, 255);
    let black = Color::rgba(0, 0, 0, 255);
    let e = |c: Color, s: f32| glyph_contrast(c, s);
    // Light text: the edges blend as in linear light (1 / 2.2); dark text: a little fuller than plain coverage.
    assert!((e(white, 20.0) - 1.0 / 2.2).abs() < 1e-3);
    assert!((e(black, 20.0) - 1.0 / 1.4).abs() < 1e-3);
    // Every exponent is below 1 (an edge never gets less alpha than its coverage) and positive.
    for c in [
        white,
        black,
        Color::rgba(200, 160, 90, 255),
        Color::rgba(40, 60, 90, 128),
    ] {
        for s in [8.0, 11.0, 12.0, 14.0, 16.0, 24.0, f32::NAN] {
            let v = e(c, s);
            assert!(v > 0.0 && v < 1.0, "{c:?} {s}: {v}");
        }
    }
    // Smaller sizes get more ink, down to 10 px, and none above 16 px.
    assert!(e(white, 11.0) < e(white, 14.0) && e(white, 14.0) < e(white, 16.0));
    assert_eq!(e(black, 16.0), e(black, 30.0));
    assert_eq!(e(black, 10.0), e(black, 6.0));
    // Brighter text: a smaller exponent.
    assert!(e(Color::rgba(200, 200, 200, 255), 14.0) < e(Color::rgba(60, 60, 60, 255), 14.0));
}
