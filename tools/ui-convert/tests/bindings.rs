//! Bindings and texts on the top strip of `skins/engraved` (stored scan, no browser): typing
//! against the model schema, the source position of errors, l10n keys and arguments, the glyph
//! set and the samples.

use std::path::{Path, PathBuf};

use serde_json::Value;
use ui2_convert::{Input, Run, check_skin};
use ui2_package::profile::TypeSig;
use ui2_package::profile::action::Action;

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn engraved() -> PathBuf {
    crate_dir()
        .join("..")
        .join("..")
        .join("skins")
        .join("engraved")
}

fn dump() -> String {
    std::fs::read_to_string(crate_dir().join("tests/fixtures/top-strip.ir.json")).expect("fixture")
}

fn top() -> Run {
    check_skin(&engraved(), Input::Dump(&dump()))
}

/// The dump with one attribute value of the scan replaced.
fn with_attr(name: &str, old: &str, new: &str) -> String {
    fn walk(v: &mut Value, name: &str, old: &str, new: &str) -> bool {
        match v {
            Value::Object(m) => {
                if let Some(Value::Array(attrs)) = m.get_mut("attrs") {
                    for a in attrs.iter_mut() {
                        if a[0] == name && a[1] == old {
                            a[1] = Value::String(new.to_string());
                            return true;
                        }
                    }
                }
                m.values_mut().any(|c| walk(c, name, old, new))
            }
            Value::Array(list) => list.iter_mut().any(|c| walk(c, name, old, new)),
            _ => false,
        }
    }
    let mut v: Value = serde_json::from_str(&dump()).unwrap();
    assert!(
        walk(&mut v["scan"], name, old, new),
        "{name}={old} not in the fixture"
    );
    v.to_string()
}

fn codes_of(run: &Run, prefix: &str) -> Vec<String> {
    run.errors
        .iter()
        .filter(|e| e.code.starts_with(prefix))
        .map(|e| format!("{} {} {}", e.code, e.prop, e.place))
        .collect()
}

#[test]
fn the_top_strip_types_against_the_schema() {
    let run = top();
    // The skin binds cleanly (background-size, which the research scale binds, is in the profile).
    assert_eq!(codes_of(&run, "E4"), Vec::<String>::new());
    assert_eq!(codes_of(&run, "E6"), Vec::<String>::new());
    let b = run.bindings.as_ref().expect("bindings");
    assert_eq!(b.paths.get("top.gold"), Some(&TypeSig::Int));
    assert!(b.paths.contains_key("research.current.name"));
    assert!(b.paths.contains_key("top.resources"));
    assert!(b.intents.contains_key("screen.open"));
    assert!(b.intents.contains_key("window.open"));
    // Intent arguments are named after the catalogue.
    let named = b.nodes.values().any(|n| {
        n.attrs.events.iter().any(|ev| {
            ev.actions.iter().any(|a| {
                matches!(a, Action::Intent { name, args } if name == "screen.open"
                    && args.iter().all(|x| x.name.as_deref() == Some("screen")))
            })
        })
    });
    assert!(named);
    // The resources loop takes its key from the schema.
    let lp = b
        .nodes
        .values()
        .find_map(|n| n.attrs.for_.as_ref())
        .expect("a loop");
    assert!(lp.key.is_some());
    let keys: Vec<&str> = b.keys.iter().map(|k| k.key.as_str()).collect();
    assert!(
        keys.contains(&"top.turns") && keys.contains(&"top.next") && keys.contains(&"skin.title")
    );
}

#[test]
fn a_path_outside_the_schema_is_e4_at_the_element() {
    let text = with_attr(
        "data-bind-value",
        "{{research.current.name}}",
        "{{research.current.nmae}}",
    );
    let run = check_skin(&engraved(), Input::Dump(&text));
    let e = run
        .errors
        .iter()
        .find(|e| e.code == "E4-004")
        .unwrap_or_else(|| panic!("{:?}", run.errors));
    assert_eq!(e.prop, "data-bind-value");
    assert!(e.element.contains("class=\"name\""), "{e}");
    assert!(e.place.line > 0 && e.place.col > 0, "{e}");
    assert!(e.why.contains("nmae") || e.why.contains("research"), "{e}");
    // The printed error names the file, the element, the attribute and why.
    let s = e.to_string();
    assert!(
        s.starts_with(&format!(
            "error[E4-004]: {}: <span class=\"name\"> data-bind-value",
            e.place
        )),
        "{s}"
    );
}

#[test]
fn loops_need_lists_and_intents_their_types() {
    let run = check_skin(
        &engraved(),
        Input::Dump(&with_attr(
            "data-bind-for",
            "r:{{top.resources}}",
            "r:{{top.gold}}",
        )),
    );
    let e4 = codes_of(&run, "E4");
    assert!(
        e4.iter().any(|c| c.starts_with("E4-009 data-bind-for")),
        "{e4:?}"
    );
    // The loop body is not checked against a variable that could not be typed: every error is on
    // the loop element itself.
    let at = e4
        .iter()
        .find(|c| c.starts_with("E4-009"))
        .and_then(|c| c.rsplit(' ').next())
        .unwrap()
        .to_string();
    assert!(
        e4.iter()
            .filter(|c| !c.starts_with("E4-025"))
            .all(|c| c.ends_with(&at)),
        "{e4:?}"
    );

    let run = check_skin(
        &engraved(),
        Input::Dump(&with_attr(
            "data-on-click",
            "intent screen.open(screen: 'tech_tree')",
            "intent screen.open(screen: 'nowhere')",
        )),
    );
    assert!(
        codes_of(&run, "E4").iter().any(|c| c.starts_with("E4-007")),
        "{:?}",
        run.errors
    );
}

#[test]
fn text_keys_exist_and_get_their_arguments() {
    let run = check_skin(
        &engraved(),
        Input::Dump(&with_attr("data-l10n", "top.next", "top.later")),
    );
    let e6: Vec<String> = codes_of(&run, "E6");
    assert_eq!(e6.len(), 1, "{e6:?}");
    assert!(e6[0].starts_with("E6-002 top.later "), "{e6:?}");

    // A loc() call that passes fewer arguments than the template uses.
    let run = check_skin(
        &engraved(),
        Input::Dump(&with_attr(
            "data-bind-value",
            "{{top.gold}}",
            "{{loc('top.next')}}",
        )),
    );
    let e6: Vec<String> = codes_of(&run, "E6");
    assert_eq!(e6.len(), 1, "{e6:?}");
    assert!(e6[0].starts_with("E6-004 top.next "), "{e6:?}");
}
#[test]
fn the_glyph_set_covers_tables_literals_and_samples() {
    let run = top();
    let t = run.texts.as_ref().expect("texts");
    assert_eq!(t.langs.len(), 1);
    assert_eq!(t.langs[0].tag, "en_US");
    let g = &t.glyphs;
    for c in "Engraved turns next AD BC0123456789/+\u{a0}".chars() {
        assert!(g.contains(c), "{c:?}");
    }
    // The named ranges of [fonts] subset: latin, latin-ext, cyrillic.
    assert!(g.contains('\u{416}') && g.contains('\u{142}'));
    assert!(!g.contains('\u{3b1}'));
    let cps = g.codepoints();
    assert!(
        cps.windows(2).all(|w| w[0].1 + 1 < w[1].0),
        "merged and ascending"
    );
}

fn temp_skin(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ui2-convert-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sample")).unwrap();
    dir
}

#[test]
fn samples_fit_the_schema_and_the_skin_state() {
    let skin_text = std::fs::read_to_string(engraved().join("skin.toml")).unwrap();
    let skin = ui2_convert::skin::Skin::parse(&skin_text.replace("\r\n", "\n")).unwrap();
    let schema = ui2_model::Schema::v1();

    let (texts, errs) = ui2_convert::bindings::check_samples(&engraved(), &skin, schema);
    assert!(errs.is_empty(), "{errs:?}");
    assert!(!texts.is_empty());

    // The temporary skin has only a hud sample.
    let mut skin = skin;
    skin.verify.states = vec!["hud".to_string()];
    let dir = temp_skin("samples");
    std::fs::write(
        dir.join("sample/hud.json"),
        r#"{"model": {"top": {"gold": "lots"}}, "state": {"zoom": 2}}"#,
    )
    .unwrap();
    let (_, errs) = ui2_convert::bindings::check_samples(&dir, &skin, schema);
    let codes: Vec<&str> = errs.iter().map(|e| e.code.as_str()).collect();
    assert!(codes.contains(&"E4-040"), "{errs:?}");
    assert!(errs.iter().all(|e| e.place.file == "sample/hud.json"));

    std::fs::write(
        dir.join("sample/hud.json"),
        r#"{"model": {}, "state": {"zoom": 2}}"#,
    )
    .unwrap();
    let (_, errs) = ui2_convert::bindings::check_samples(&dir, &skin, schema);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].code, "E4-041");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn skin_state_types_reach_the_expressions() {
    let text = "[skin]\nid = \"t\"\nversion = \"0.1.0\"\ntitle = \"skin.title\"\nmodel = \"1.0\"\napi = \"1.0\"\n\n[state]\nprod = { sort = \"name\" }\nopen = false\n";
    let skin = ui2_convert::skin::Skin::parse(text).unwrap();
    let scope = ui2_convert::bindings::state_scope(&skin);
    let schema = ui2_model::Schema::v1();
    let e = ui2_package::profile::expr::parse("state.open ? state.prod.sort : 'x'").unwrap();
    let ty = ui2_package::profile::expr::typecheck(&e, &scope, schema).expect("types");
    assert!(ty.alts.contains(&TypeSig::Str));
    let e = ui2_package::profile::expr::parse("state.missing").unwrap();
    assert!(ui2_package::profile::expr::typecheck(&e, &scope, schema).is_err());
}
