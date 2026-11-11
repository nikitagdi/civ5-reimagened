//! Rules 1 to 3: features, no-break space, width snapping; plus the shaping cache.

mod common;

use common::*;
use ui2_text::{
    FaceId, NumericFigure, NumericSpacing, NumericVariant, RunStyle, TextEngine, TextError,
};

fn fira(size: f32) -> RunStyle {
    RunStyle::new(FIRA_400, size)
}

fn tabular() -> NumericVariant {
    NumericVariant::from_css("lining-nums tabular-nums")
}

#[test]
fn numeric_variant_parses_css() {
    let v = NumericVariant::from_css("lining-nums tabular-nums");
    assert_eq!(
        (v.figure, v.spacing),
        (NumericFigure::Lining, NumericSpacing::Tabular)
    );
    let v = NumericVariant::from_css("oldstyle-nums proportional-nums");
    assert_eq!(
        (v.figure, v.spacing),
        (NumericFigure::Oldstyle, NumericSpacing::Proportional)
    );
    assert_eq!(
        NumericVariant::from_css("normal"),
        NumericVariant::default()
    );
}

#[test]
fn chrome_width_of_a_digit_run() {
    // Chrome 148: "0123456789", Fira Sans 400, 12 px, tabular lining figures -> 67.203125.
    let mut e = engine();
    let w = e
        .measure(&fira(12.0).with_numeric(tabular()), "0123456789")
        .unwrap();
    assert!((w - 67.203125).abs() <= 0.05, "width {w}");
}

#[test]
fn tabular_figures_have_equal_advances_proportional_do_not() {
    let mut e = engine();
    let tnum = fira(20.0).with_numeric(NumericVariant::from_css("tabular-nums"));
    let pnum = fira(20.0).with_numeric(NumericVariant::from_css("proportional-nums"));
    let t1 = e.measure(&tnum, "1111").unwrap();
    let t8 = e.measure(&tnum, "8888").unwrap();
    assert_eq!(t1, t8);
    let p1 = e.measure(&pnum, "1111").unwrap();
    let p8 = e.measure(&pnum, "8888").unwrap();
    assert!(
        p1 < p8,
        "proportional 1 should be narrower than 8: {p1} {p8}"
    );
}

#[test]
fn ligatures_on_by_default_off_with_letter_spacing() {
    let mut e = engine();
    let plain = e
        .shape(&RunStyle::new(ALEGREYA, 24.0), "office fi")
        .unwrap();
    let spaced = e
        .shape(
            &RunStyle::new(ALEGREYA, 24.0).with_letter_spacing(0.5),
            "office fi",
        )
        .unwrap();
    assert!(
        plain.glyphs.len() < "office fi".chars().count(),
        "a ligature should form"
    );
    assert_eq!(
        spaced.glyphs.len(),
        "office fi".chars().count(),
        "letter-spacing disables liga"
    );
}

#[test]
fn nbsp_is_shaped_as_a_space_but_kept_in_the_text() {
    let mut e = engine();
    let a = e.shape(&fira(14.0), "a\u{a0}b").unwrap();
    let b = e.shape(&fira(14.0), "a b").unwrap();
    assert_eq!(a.width, b.width);
    assert_eq!(&*a.text, "a\u{a0}b");
    // The clusters index the original string: 'b' starts at byte 3 (the no-break space takes two).
    assert_eq!(
        a.glyphs.iter().map(|g| g.cluster).collect::<Vec<_>>(),
        vec![0, 1, 3]
    );
    let two = e.shape(&fira(14.0), "a\u{a0}\u{a0}b").unwrap();
    let sp = e.measure(&fira(14.0), " ").unwrap();
    assert!((two.width - (a.width + sp)).abs() <= 1.0 / 32.0);
}

#[test]
fn width_is_snapped_up_to_one_64th() {
    let mut e = engine();
    for text in ["Hello", "Turns production into gold 1,250", "AVATAR Wave"] {
        let w = e.measure(&fira(13.0), text).unwrap();
        assert_eq!(
            f64::from(w) * 64.0,
            (f64::from(w) * 64.0).round(),
            "{text}: {w}"
        );
    }
}

#[test]
fn letter_spacing_follows_every_character_including_the_last() {
    let mut e = engine();
    let base = e.shape(&fira(16.0), "ab").unwrap();
    let spaced = e.shape(&fira(16.0).with_letter_spacing(2.0), "ab").unwrap();
    // No kerning pair or ligature in "ab" for Fira, so only the spacing differs: 2 characters, 2 px each.
    assert!(
        (spaced.width - base.width - 4.0).abs() <= 1.0 / 32.0,
        "{} vs {}",
        spaced.width,
        base.width
    );
    // The second glyph starts after one spacing; the run ends after two.
    let g = &spaced.glyphs;
    assert!((g[1].x - g[0].advance).abs() < 1e-4);
    assert!((g[1].x - (base.glyphs[1].x + 2.0)).abs() < 1e-3);
}

#[test]
fn glyph_positions_accumulate_advances() {
    let mut e = engine();
    let r = e.shape(&fira(20.0), "Hello World").unwrap();
    let mut pen = 0.0f32;
    for g in r.glyphs.iter() {
        assert!((g.x - pen).abs() < 1e-3);
        pen += g.advance;
    }
    assert!(r.width >= pen - 1e-3 && r.width - pen < 1.0 / 64.0 + 1e-3);
}

#[test]
fn alegreya_weight_axis_changes_widths_fira_ignores_weight() {
    let mut e = engine();
    let a400 = e
        .measure(&RunStyle::new(ALEGREYA, 24.0), "Moscow Great Library")
        .unwrap();
    let a500 = e
        .measure(
            &RunStyle::new(ALEGREYA, 24.0).with_wght(500.0),
            "Moscow Great Library",
        )
        .unwrap();
    assert!(a500 > a400, "{a500} {a400}");
    let f400 = e.measure(&fira(24.0), "Moscow").unwrap();
    let f_wght = e.measure(&fira(24.0).with_wght(700.0), "Moscow").unwrap();
    assert_eq!(f400, f_wght);
}

#[test]
fn unknown_face_is_an_error() {
    let mut e = TextEngine::new();
    let r = e.shape(&RunStyle::new(FaceId(3), 12.0), "x");
    assert!(matches!(r, Err(TextError::UnknownFace(FaceId(3)))));
}

#[test]
fn cache_hits_and_distinguishes_styles() {
    let mut e = engine();
    let s = fira(14.0);
    let a = e.shape(&s, "Education").unwrap();
    assert_eq!(
        (e.shape_cache_stats().hits, e.shape_cache_stats().misses),
        (0, 1)
    );
    let b = e.shape(&s, "Education").unwrap();
    assert_eq!(a, b);
    assert_eq!(
        (e.shape_cache_stats().hits, e.shape_cache_stats().misses),
        (1, 1)
    );
    e.shape(&fira(15.0), "Education").unwrap();
    e.shape(&s.with_letter_spacing(1.0), "Education").unwrap();
    e.shape(&s.with_numeric(tabular()), "Education").unwrap();
    e.shape(&RunStyle::new(FIRA_500, 14.0), "Education")
        .unwrap();
    e.shape(&s, "Educations").unwrap();
    let st = e.shape_cache_stats();
    assert_eq!((st.hits, st.misses, st.entries), (1, 6, 6));
}

#[test]
fn cache_is_bounded() {
    let mut e = TextEngine::with_shape_cache_capacity(16);
    e.load_face(FIRA_400, read_font("fira-sans/FiraSans-Regular.ttf"))
        .unwrap();
    for i in 0..200 {
        e.shape(&fira(12.0), &format!("run number {i}")).unwrap();
    }
    assert!(e.shape_cache_stats().entries <= 16);
    // A recent entry survives eviction, an old one is shaped again.
    let before = e.shape_cache_stats();
    e.shape(&fira(12.0), "run number 199").unwrap();
    assert_eq!(e.shape_cache_stats().hits, before.hits + 1);
}
