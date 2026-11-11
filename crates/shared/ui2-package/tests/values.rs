//! The profile's value parsers against `profile/value-vectors.json` (Chrome serializations).
#![cfg(feature = "profile")]

use std::collections::BTreeSet;

use serde::Deserialize;
use ui2_package::profile::values::{codes, parse_value};
use ui2_package::profile::{Profile, PropId, ValueKind};

#[derive(Deserialize)]
struct Vector {
    prop: String,
    input: String,
    ok: Option<String>,
    err: Option<String>,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("../profile/value-vectors.json")).expect("vectors parse")
}

#[test]
fn vectors_are_green() {
    let all = vectors();
    assert!(all.len() >= 150, "{} vectors", all.len());
    let mut failures = Vec::new();
    for v in &all {
        let Some(prop) = PropId::from_name(&v.prop) else {
            failures.push(format!("unknown property {}", v.prop));
            continue;
        };
        match (parse_value(prop, &v.input), &v.ok, &v.err) {
            (Ok(value), Some(want), None) => {
                if &value.canon() != want {
                    failures.push(format!(
                        "{} {:?}: got {}, want {want}",
                        v.prop,
                        v.input,
                        value.canon()
                    ));
                }
            }
            (Err(e), None, Some(code)) => {
                if e.code != code {
                    failures.push(format!("{} {:?}: got {e}, want {code}", v.prop, v.input));
                }
            }
            (got, ok, err) => failures.push(format!(
                "{} {:?}: got {got:?}, vector says ok={ok:?} err={err:?}",
                v.prop, v.input
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn vectors_cover_every_error_code_and_value_kind() {
    let all = vectors();
    let used: BTreeSet<&str> = all.iter().filter_map(|v| v.err.as_deref()).collect();
    for code in codes::ALL {
        assert!(used.contains(code), "no vector for {code}");
    }
    let profile = Profile::v1();
    let kinds: BTreeSet<String> = all
        .iter()
        .filter_map(|v| PropId::from_name(&v.prop))
        .map(|p| format!("{:?}", profile.def(p).kind))
        .collect();
    for kind in [
        ValueKind::Keyword,
        ValueKind::KeywordSet,
        ValueKind::Length,
        ValueKind::Color,
        ValueKind::Number,
        ValueKind::Integer,
        ValueKind::Radius,
        ValueKind::Shadows,
        ValueKind::Transform,
        ValueKind::Origin,
        ValueKind::Times,
        ValueKind::Easings,
        ValueKind::Properties,
        ValueKind::Families,
        ValueKind::LineHeight,
        ValueKind::Images,
        ValueKind::BgSizes,
        ValueKind::Lengths,
        ValueKind::Repeats,
        ValueKind::Keywords,
        ValueKind::Ratio,
        ValueKind::Tracks,
        ValueKind::TrackSizes,
        ValueKind::GridLine,
        ValueKind::Url,
        ValueKind::Slice,
        ValueKind::ImageWidths,
        ValueKind::ImageRepeat,
        ValueKind::Polygon,
    ] {
        assert!(
            kinds.contains(&format!("{kind:?}")),
            "no vector for {kind:?}"
        );
    }
}

/// Every longhand the profile reference lists (layout, decoration, text, icons), and the
/// properties it documents as ignored.
#[test]
fn the_profile_covers_the_reference_tables() {
    let layout = [
        "display",
        "position",
        "z-index",
        "box-sizing",
        "top",
        "right",
        "bottom",
        "left",
        "width",
        "height",
        "min-width",
        "min-height",
        "max-width",
        "max-height",
        "aspect-ratio",
        "margin-top",
        "margin-right",
        "margin-bottom",
        "margin-left",
        "padding-top",
        "padding-right",
        "padding-bottom",
        "padding-left",
        "overflow-x",
        "overflow-y",
        "scrollbar-width",
        "flex-direction",
        "flex-wrap",
        "flex-grow",
        "flex-shrink",
        "flex-basis",
        "order",
        "justify-content",
        "align-items",
        "align-self",
        "align-content",
        "justify-items",
        "justify-self",
        "row-gap",
        "column-gap",
        "grid-template-columns",
        "grid-template-rows",
        "grid-auto-columns",
        "grid-auto-rows",
        "grid-auto-flow",
        "grid-row-start",
        "grid-row-end",
        "grid-column-start",
        "grid-column-end",
    ];
    let decoration = [
        "color",
        "background-color",
        "opacity",
        "visibility",
        "background-image",
        "background-size",
        "background-position-x",
        "background-position-y",
        "background-repeat",
        "background-origin",
        "background-clip",
        "border-top-width",
        "border-top-style",
        "border-top-color",
        "border-top-left-radius",
        "border-image-source",
        "border-image-slice",
        "border-image-width",
        "border-image-repeat",
        "border-image-outset",
        "outline-style",
        "outline-width",
        "outline-color",
        "outline-offset",
        "box-shadow",
        "text-shadow",
        "clip-path",
        "transform",
        "transform-origin",
        "cursor",
        "pointer-events",
        "transition-property",
        "transition-duration",
        "transition-delay",
        "transition-timing-function",
    ];
    let text = [
        "font-family",
        "font-size",
        "font-weight",
        "font-style",
        "font-variant-numeric",
        "line-height",
        "letter-spacing",
        "word-spacing",
        "text-align",
        "text-indent",
        "white-space-collapse",
        "text-wrap-mode",
        "text-wrap-style",
        "text-overflow",
        "overflow-wrap",
        "word-break",
        "text-transform",
        "text-decoration-line",
        "vertical-align",
        "direction",
        "writing-mode",
    ];
    let icons = [
        "fill",
        "stroke",
        "stroke-width",
        "stroke-linecap",
        "stroke-linejoin",
    ];
    let profile = Profile::v1();
    for name in layout.iter().chain(&decoration).chain(&text).chain(&icons) {
        assert!(profile.prop(name).is_some(), "{name} has no row");
    }
    let ignored: Vec<&str> = profile.ignored_props().keys().map(String::as_str).collect();
    let mut want = vec![
        "-webkit-font-smoothing",
        "text-rendering",
        "user-select",
        "will-change",
        "-webkit-tap-highlight-color",
        "font-kerning",
        "font-optical-sizing",
        "caret-color",
    ];
    want.sort_unstable();
    assert_eq!(ignored, want);
    // Out of the table on purpose: Chrome would bake them, or the runtime has no use for them.
    for name in [
        "float",
        "grid-template-areas",
        "font-feature-settings",
        "animation-name",
    ] {
        assert!(profile.prop(name).is_none(), "{name}");
    }
}

#[test]
fn every_property_parses_its_initial_value() {
    for &id in PropId::ALL {
        let def = Profile::v1().def(id);
        assert!(
            parse_value(id, &def.initial).is_ok(),
            "{}: initial {:?}",
            id.name(),
            def.initial
        );
    }
}

#[test]
fn every_error_code_has_a_text() {
    let profile = Profile::v1();
    for code in codes::ALL {
        let def = profile
            .error(code)
            .unwrap_or_else(|| panic!("no text for {code}"));
        assert!(!def.what.is_empty() && !def.hint.is_empty());
    }
}

#[test]
fn nan_and_viewport_units_are_errors_everywhere() {
    let profile = Profile::v1();
    for &id in PropId::ALL {
        let def = profile.def(id);
        if matches!(def.kind, ValueKind::Length) {
            assert_eq!(
                parse_value(id, "10vw").err().map(|e| e.code),
                Some(codes::UNIT),
                "{}",
                id.name()
            );
            assert_eq!(
                parse_value(id, "NaN").err().map(|e| e.code),
                Some(codes::NOT_FINITE),
                "{}",
                id.name()
            );
        }
    }
}

#[test]
fn parsing_is_deterministic() {
    let a = parse_value(PropId::BoxShadow, "1px 2px 3px red, inset 0 0 0 1px #fff").expect("a");
    let b = parse_value(PropId::BoxShadow, "1px 2px 3px red, inset 0 0 0 1px #fff").expect("b");
    assert_eq!(a, b);
    assert_eq!(a.canon(), b.canon());
}

// ---------------------------------------------------------------------------------------------
// Parsed values into records

use ui2_package::StrId;
use ui2_package::profile::apply::compose_transitions;
use ui2_package::profile::values::Value;
use ui2_package::style::{PropSel, StyleRecord};

fn no_strings(_: &str) -> StrId {
    StrId(0)
}

#[test]
fn record_initials_match_the_profile_table() {
    for &id in PropId::ALL {
        let def = Profile::v1().def(id);
        let mut record = StyleRecord::initial();
        let value = parse_value(id, &def.initial).expect("initial parses");
        match record.set_value(id, &value, &mut no_strings) {
            Ok(()) => {}
            Err(ui2_package::style::StyleError::Transition(_)) => continue,
            Err(e) => panic!("{}: {e}", id.name()),
        }
        if id == PropId::FontFamily {
            continue; // family ids belong to the builder's string table
        }
        assert_eq!(record, StyleRecord::initial(), "{}", id.name());
    }
}

#[test]
fn every_keyword_of_the_table_has_a_record_variant() {
    for &id in PropId::ALL {
        let def = Profile::v1().def(id);
        if def.kind != ValueKind::Keyword {
            continue;
        }
        let mut seen = Vec::new();
        for keyword in &def.values {
            let mut record = StyleRecord::initial();
            record
                .set_value(id, &Value::Keyword(keyword.clone()), &mut no_strings)
                .unwrap_or_else(|e| panic!("{} {keyword}: {e}", id.name()));
            seen.push(format!("{record:?}"));
        }
        seen.sort();
        seen.dedup();
        assert_eq!(
            seen.len(),
            def.values.len(),
            "{}: two keywords share a variant",
            id.name()
        );
    }
}

#[test]
fn a_value_of_the_wrong_kind_does_not_fit() {
    let mut record = StyleRecord::initial();
    let color = parse_value(PropId::Color, "red").expect("color");
    assert!(
        record
            .set_value(PropId::Width, &color, &mut no_strings)
            .is_err()
    );
    let length = parse_value(PropId::Width, "4px").expect("length");
    assert!(
        record
            .set_value(PropId::Color, &length, &mut no_strings)
            .is_err()
    );
    let times = parse_value(PropId::TransitionDuration, "1s").expect("times");
    assert!(matches!(
        record.set_value(PropId::TransitionDuration, &times, &mut no_strings),
        Err(ui2_package::style::StyleError::Transition(_))
    ));
}

#[test]
fn background_and_outline_values_fill_the_record() {
    use ui2_package::style::{
        BgImage, BgRepeat, BgSize, ColorV, GradDir, Len, LenAuto, OutlineStyle, Repeat, Side,
    };
    let mut r = StyleRecord::initial();
    for (id, text) in [
        (
            PropId::BackgroundImage,
            "linear-gradient(to right, rgba(0, 0, 0, 0) calc(100% - 1px), rgb(27, 24, 21) calc(100% - 1px))",
        ),
        (PropId::BackgroundSize, "12.6667% 100%"),
        (PropId::BackgroundRepeat, "repeat-x"),
        (PropId::OutlineStyle, "solid"),
        (PropId::OutlineWidth, "1px"),
        (PropId::OutlineColor, "rgb(201, 163, 106)"),
        (PropId::OutlineOffset, "-2px"),
    ] {
        let value = parse_value(id, text).unwrap_or_else(|e| panic!("{}: {e}", id.name()));
        r.set_value(id, &value, &mut no_strings)
            .unwrap_or_else(|e| panic!("{}: {e}", id.name()));
    }
    let [BgImage::Gradient(g)] = r.background_image.as_slice() else {
        panic!("one layer");
    };
    assert!(!g.conic && !g.repeating);
    assert_eq!(g.dir, GradDir::To(Side::Right));
    assert_eq!(g.stops[0].color, ColorV::Lit([0, 0, 0, 0]));
    assert_eq!(
        g.stops[1].pos,
        Some(Len {
            px: -1.0,
            pct: 100.0
        })
    );
    assert_eq!(
        r.background_size,
        [BgSize::Size(
            LenAuto::Len(Len::pct(12.6667)),
            LenAuto::Len(Len::pct(100.0))
        )]
    );
    assert_eq!(
        r.background_repeat,
        [BgRepeat {
            x: Repeat::Repeat,
            y: Repeat::NoRepeat
        }]
    );
    assert_eq!(r.outline_style, OutlineStyle::Solid);
    assert_eq!(r.outline_width, Len::px(1.0));
    assert_eq!(r.outline_color, ColorV::Lit([201, 163, 106, 255]));
    assert_eq!(r.outline_offset, Len::px(-2.0));
    assert!(r.is_finite());

    let e = parse_value(PropId::OutlineStyle, "auto").expect_err("auto");
    assert!(e.message.contains(":focus-visible"), "{e}");
}

#[test]
fn picture_urls_become_string_ids() {
    use ui2_package::style::BgImage;
    let mut seen = Vec::new();
    let mut strs = |s: &str| {
        seen.push(s.to_string());
        StrId(seen.len() as u32 - 1)
    };
    let mut r = StyleRecord::initial();
    let v = parse_value(
        PropId::BackgroundImage,
        "url(\"img/a.svg\"), linear-gradient(rgb(255, 0, 0), rgb(0, 0, 255))",
    )
    .expect("layers");
    r.set_value(PropId::BackgroundImage, &v, &mut strs)
        .expect("fits");
    let v = parse_value(PropId::BorderImageSource, "url(\"img/frame.png\")").expect("frame");
    r.set_value(PropId::BorderImageSource, &v, &mut strs)
        .expect("fits");
    assert!(matches!(r.background_image[0], BgImage::Url(StrId(0))));
    assert!(matches!(r.background_image[1], BgImage::Gradient(_)));
    assert_eq!(r.border_image_source, Some(StrId(1)));
    assert_eq!(r.str_ids().collect::<Vec<_>>(), [StrId(0), StrId(1)]);
    let none = parse_value(PropId::BorderImageSource, "none").expect("none");
    r.set_value(PropId::BorderImageSource, &none, &mut strs)
        .expect("fits");
    assert_eq!(r.border_image_source, None);
    assert_eq!(seen, ["img/a.svg", "img/frame.png"]);
}

#[test]
fn grid_border_image_and_clip_values_fill_the_record() {
    use ui2_package::style::{
        AspectRatio, GridAutoFlow, GridLine, ImageRepeat, ImageRepeats, ImageWidth, Len, Point,
        Track, TrackBreadth, TrackSize,
    };
    let mut r = StyleRecord::initial();
    for (id, text) in [
        (
            PropId::GridTemplateColumns,
            "repeat(2, 40px 1fr) minmax(100px, 1fr)",
        ),
        (PropId::GridAutoRows, "minmax(0px, 1fr)"),
        (PropId::GridAutoFlow, "column dense"),
        (PropId::GridRowStart, "span 2"),
        (PropId::GridColumnEnd, "-1"),
        (PropId::AspectRatio, "16 / 9"),
        (PropId::BorderImageSlice, "10 20% 30 fill"),
        (PropId::BorderImageWidth, "10px 2"),
        (PropId::BorderImageRepeat, "stretch repeat"),
        (
            PropId::ClipPath,
            "polygon(0px 0px, calc(100% - 12px) 0px, 100% 12px, 100% 100%, 0px 100%)",
        ),
    ] {
        let value = parse_value(id, text).unwrap_or_else(|e| panic!("{}: {e}", id.name()));
        r.set_value(id, &value, &mut no_strings)
            .unwrap_or_else(|e| panic!("{}: {e}", id.name()));
    }
    let px = |v| TrackSize::Breadth(TrackBreadth::Len(Len::px(v)));
    let fr = |v| TrackSize::Breadth(TrackBreadth::Fr(v));
    assert_eq!(
        r.grid_template_columns,
        [
            Track::Repeat(2, vec![px(40.0), fr(1.0)]),
            Track::Size(TrackSize::MinMax(
                TrackBreadth::Len(Len::px(100.0)),
                TrackBreadth::Fr(1.0)
            ))
        ]
    );
    assert_eq!(
        r.grid_auto_rows,
        [TrackSize::MinMax(
            TrackBreadth::Len(Len::ZERO),
            TrackBreadth::Fr(1.0)
        )]
    );
    assert_eq!(r.grid_auto_flow, GridAutoFlow::ColumnDense);
    assert_eq!(r.grid_row_start, GridLine::Span(2));
    assert_eq!(r.grid_column_end, GridLine::Line(-1));
    assert_eq!(r.aspect_ratio, AspectRatio::Ratio(16.0, 9.0));
    assert_eq!(
        r.border_image_slice.edges,
        [Len::px(10.0), Len::pct(20.0), Len::px(30.0), Len::pct(20.0)]
    );
    assert!(r.border_image_slice.fill);
    assert_eq!(
        r.border_image_width,
        [
            ImageWidth::Px(10.0),
            ImageWidth::Factor(2.0),
            ImageWidth::Px(10.0),
            ImageWidth::Factor(2.0)
        ]
    );
    assert_eq!(
        r.border_image_repeat,
        ImageRepeats {
            x: ImageRepeat::Stretch,
            y: ImageRepeat::Repeat
        }
    );
    assert_eq!(r.clip_path.len(), 5);
    assert_eq!(
        r.clip_path[1],
        Point {
            x: Len {
                px: -12.0,
                pct: 100.0
            },
            y: Len::ZERO
        }
    );
    assert!(r.is_finite());
    // A repeat() cannot reach a grid-auto-* field.
    let t = parse_value(PropId::GridTemplateRows, "repeat(2, 1fr)").expect("template");
    assert!(
        r.set_value(PropId::GridAutoRows, &t, &mut no_strings)
            .is_err()
    );
}

#[test]
fn clip_polygons_are_convex_in_any_box() {
    use ui2_package::style::{Len, Point, convex_at};
    let p = |x: Len, y: Len| Point { x, y };
    let cut = [
        p(Len::ZERO, Len::ZERO),
        p(
            Len {
                px: -12.0,
                pct: 100.0,
            },
            Len::ZERO,
        ),
        p(Len::pct(100.0), Len::px(12.0)),
        p(Len::pct(100.0), Len::pct(100.0)),
        p(Len::ZERO, Len::pct(100.0)),
    ];
    assert!(convex_at(&cut, [200.0, 40.0]));
    // In a box smaller than the cut the polygon folds over itself.
    assert!(!convex_at(&cut, [8.0, 8.0]));
    // Either direction around the shape is fine; a repeated point is not.
    let mut rev = cut;
    rev.reverse();
    assert!(convex_at(&rev, [200.0, 40.0]));
    let dup = [cut[0], cut[0], cut[1], cut[2]];
    assert!(!convex_at(&dup, [200.0, 40.0]));
    // Three points on a line are a straight corner, not a fold.
    let straight = [
        p(Len::ZERO, Len::ZERO),
        p(Len::pct(50.0), Len::ZERO),
        p(Len::pct(100.0), Len::ZERO),
        p(Len::pct(100.0), Len::pct(100.0)),
    ];
    assert!(convex_at(&straight, [100.0, 100.0]));
    assert!(!convex_at(&straight[..2], [100.0, 100.0]));
}

#[test]
fn transitions_are_composed_like_css() {
    let profile = Profile::v1();
    let props = [
        PropSel::Prop(PropId::Color),
        PropSel::Prop(PropId::Opacity),
        PropSel::Prop(PropId::BackgroundColor),
    ];
    let t = compose_transitions(
        profile,
        &props,
        &[150.0, 300.0],
        &[],
        &[[0.0, 0.0, 1.0, 1.0]],
    );
    assert_eq!(t.len(), 3);
    assert_eq!(
        (t[0].dur_ms, t[1].dur_ms, t[2].dur_ms),
        (150.0, 300.0, 150.0)
    );
    assert!(
        t.iter()
            .all(|x| x.delay_ms == 0.0 && x.ease == [0.0, 0.0, 1.0, 1.0])
    );
    // `all` is one list item that covers every animatable property.
    let t = compose_transitions(profile, &[PropSel::All], &[200.0], &[50.0], &[]);
    assert!(t.len() > 10);
    assert!(t.iter().all(|x| x.dur_ms == 200.0 && x.delay_ms == 50.0));
    assert!(t.iter().all(|x| profile.def(x.prop).anim));
    assert!(compose_transitions(profile, &[], &[200.0], &[], &[]).is_empty());
}
