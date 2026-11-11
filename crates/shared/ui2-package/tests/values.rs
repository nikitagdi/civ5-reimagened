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
    ] {
        assert!(
            kinds.contains(&format!("{kind:?}")),
            "no vector for {kind:?}"
        );
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
