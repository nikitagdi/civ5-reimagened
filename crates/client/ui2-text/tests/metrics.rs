//! Rule 4 and 5: line metrics and baseline placement, with values measured in Chrome 148.

mod common;

use common::*;
use std::sync::Arc;
use ui2_text::{FaceId, TextEngine, TextError};

#[test]
fn fira_metrics_round_each_value() {
    let e = engine();
    // Fira Sans: typo 935 / -265 / 0 at upem 1000. Chrome: 12 px -> content 14, baseline 11.
    let m = e.metrics(FIRA_400, 12.0, 400.0).unwrap();
    assert_eq!((m.ascent, m.descent, m.line_gap), (11.0, 3.0, 0.0));
    assert_eq!(m.content_height(), 14.0);
    assert_eq!(m.normal_line_height(), 14.0);
    assert_eq!(m.baseline_in(m.normal_line_height()), 11.0);
    // 14 px: 13.09 -> 13, 3.71 -> 4.
    let m = e.metrics(FIRA_500, 14.0, 500.0).unwrap();
    assert_eq!((m.ascent, m.descent), (13.0, 4.0));
}

#[test]
fn alegreya_uses_typo_not_win_metrics() {
    let e = engine();
    // typo 1016 / -345 (win would be 1123): at 48 px asc 48.77 -> 49, desc 16.56 -> 17.
    let m = e.metrics(ALEGREYA, 48.0, 500.0).unwrap();
    assert_eq!((m.ascent, m.descent), (49.0, 17.0));
}

#[test]
fn baseline_floors_the_top_half_leading() {
    let e = engine();
    let m = e.metrics(FIRA_400, 13.0, 400.0).unwrap(); // asc 12 (12.155), desc 3 (3.445)
    assert_eq!((m.ascent, m.descent), (12.0, 3.0));
    // line-height 20: leading 5, top half floor(2.5) = 2.
    assert_eq!(m.baseline_in(20.0), 14.0);
    // line-height 10 (< content): leading -5, floor(-2.5) = -3.
    assert_eq!(m.baseline_in(10.0), 9.0);
}

#[test]
fn unknown_face_and_bad_bytes_are_errors() {
    let mut e = TextEngine::new();
    assert!(matches!(
        e.metrics(FaceId(9), 12.0, 400.0),
        Err(TextError::UnknownFace(FaceId(9)))
    ));
    assert!(matches!(
        e.load_face(FaceId(1), Arc::from(vec![1u8, 2, 3])),
        Err(TextError::Font(_))
    ));
    assert!(!e.has_face(FaceId(1)));
}
