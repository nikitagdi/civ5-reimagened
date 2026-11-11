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
