//! The attribute and action grammars of the profile: parsing, typing and the error codes.
#![cfg(feature = "profile")]

use std::collections::BTreeSet;

use ui2_package::Layer;
use ui2_package::profile::action::{Action, AnchorKind, ControlKind, Event};
use ui2_package::profile::attrs::{
    AttrCtx, NodeAttrs, ProfileError, RawAttrs, check, codes, parse,
};
use ui2_package::profile::expr::{self, Expr, Scope};
use ui2_package::profile::{Profile, Tag, TypeSig};

mod common;
use common::{MapView, parse_type};

fn raw(tag: Tag, attrs: &[(&str, &str)]) -> RawAttrs {
    RawAttrs::new(tag, attrs)
}

fn ok(tag: Tag, attrs: &[(&str, &str)]) -> NodeAttrs {
    parse(&raw(tag, attrs), &AttrCtx::default()).unwrap_or_else(|e| panic!("{attrs:?}: {e:?}"))
}

fn codes_of(r: Result<NodeAttrs, Vec<ProfileError>>) -> Vec<&'static str> {
    match r {
        Ok(a) => panic!("expected errors, parsed {a:?}"),
        Err(es) => es.iter().map(|e| e.code).collect(),
    }
}

fn fails(tag: Tag, attrs: &[(&str, &str)], ctx: &AttrCtx) -> Vec<&'static str> {
    codes_of(parse(&raw(tag, attrs), ctx))
}

fn expect(tag: Tag, attrs: &[(&str, &str)], code: &str) {
    let got = fails(tag, attrs, &AttrCtx::default());
    assert!(got.contains(&code), "{attrs:?}: got {got:?}, want {code}");
}

// ---- parsing: what is accepted ------------------------------------------------------------

#[test]
fn bindings_of_text_and_conditions() {
    let a = ok(
        Tag::Span,
        &[
            ("data-bind-value", "{{ game.turn }}/{{ game.gold }}"),
            ("class", "x"),
        ],
    );
    assert_eq!(a.value.expect("value").segs.len(), 3);
    assert_eq!(a.plain, [("class".to_string(), "x".to_string())]);
    let a = ok(Tag::Div, &[("data-bind-if", "  {{ game.turn > 3 }} ")]);
    assert!(matches!(a.cond, Some(Expr::Binary(..))));
    let a = ok(Tag::Img, &[("data-bind-src", "{{ unit.portrait }}")]);
    assert!(a.src.is_some());
    let a = ok(Tag::Button, &[("data-bind-disabled", "{{ !game.alive }}")]);
    assert!(a.disabled.is_some());
}

#[test]
fn loops_take_their_companions() {
    let a = ok(
        Tag::Li,
        &[
            ("data-bind-for", "i, c:{{ game.cities }}"),
            ("data-bind-key", "{{ c.id }}"),
            ("data-bind-sort", "{{ state.prod.desc ? c.pop : c.name }}"),
            ("data-bind-sort-desc", "{{ state.prod.desc }}"),
            ("data-bind-filter", "{{ c.pop > 1 }}"),
            ("data-virtual", ""),
        ],
    );
    let f = a.for_.expect("for");
    assert_eq!((f.item.as_str(), f.index.as_deref()), ("c", Some("i")));
    assert!(f.key.is_some() && f.sort.is_some() && f.sort_desc.is_some() && f.filter.is_some());
    assert!(f.virtual_list);
    let a = ok(Tag::Li, &[("data-bind-for", "c:{{ game.cities }}")]);
    let f = a.for_.expect("for");
    assert_eq!(f.index, None);
    assert!(f.key.is_none());
}

#[test]
fn class_toggles_and_styles() {
    let a = ok(
        Tag::Div,
        &[(
            "data-bind-class-toggle",
            "is-on:{{ game.alive }}; late : {{ game.turn > 100 }};",
        )],
    );
    assert_eq!(a.class_toggle.len(), 2);
    assert_eq!(a.class_toggle[1].class, "late");
    let a = ok(
        Tag::Div,
        &[
            ("data-bind-style-width", "{{ game.turn }}%"),
            ("data-bind-style-color", "{{ game.accent }}"),
            ("data-bind-style-opacity", "0.5"),
        ],
    );
    assert_eq!(a.style.len(), 3);
}

#[test]
fn localization_and_includes() {
    let a = ok(
        Tag::Span,
        &[
            ("data-l10n", "top.turn"),
            ("data-l10n-args", "{{ game.turn }}, {{ game.gold }}"),
        ],
    );
    assert_eq!(a.l10n.as_deref(), Some("top.turn"));
    assert_eq!(a.l10n_args.len(), 2);
    let a = ok(
        Tag::Span,
        &[
            ("data-l10n", "k"),
            ("data-l10n-args", "{{ game.turn, 'a, b' }}"),
        ],
    );
    assert_eq!(a.l10n_args.len(), 2);
    let a = ok(
        Tag::Input,
        &[("type", "text"), ("data-l10n-placeholder", "chat.hint")],
    );
    assert_eq!(a.l10n_placeholder.as_deref(), Some("chat.hint"));
    let a = ok(Tag::Div, &[("data-include", "parts/top.html")]);
    assert_eq!(a.include.as_deref(), Some("parts/top.html"));
}

#[test]
fn events_and_actions() {
    let a = ok(
        Tag::Button,
        &[(
            "data-on-click",
            "intent unit.action(unit: u.key, action: 'fortify'); set state.prod.page = state.prod.page + 1; toggle state.prod.open; call ping(1, 'x'); stop",
        )],
    );
    assert_eq!(a.events.len(), 1);
    assert_eq!(a.events[0].ev, Event::Click);
    let acts = &a.events[0].actions;
    assert_eq!(acts.len(), 5);
    assert!(
        matches!(&acts[0], Action::Intent { name, args } if name == "unit.action" && args.len() == 2)
    );
    assert!(matches!(&acts[1], Action::Set { path, .. } if path == "state.prod.page"));
    assert!(matches!(&acts[2], Action::Toggle { path } if path == "state.prod.open"));
    assert!(matches!(&acts[3], Action::Call { name, args } if name == "ping" && args.len() == 2));
    assert_eq!(acts[4], Action::Stop);
    // A semicolon or a comma inside a string does not split.
    let a = ok(Tag::Button, &[("data-on-click", "call say('a;b', 'c,d')")]);
    assert!(matches!(&a.events[0].actions[0], Action::Call { args, .. } if args.len() == 2));
    let a = ok(
        Tag::Input,
        &[("data-on-submit", "intent chat.send(text: state.draft)")],
    );
    assert_eq!(a.events[0].ev, Event::Submit);
    let a = ok(
        Tag::Button,
        &[("data-on-click", "intent mods.my-mod.go(1, 2)")],
    );
    assert!(
        matches!(&a.events[0].actions[0], Action::Intent { name, .. } if name == "mods.my-mod.go")
    );
}

#[test]
fn the_event_shorthand_is_a_click_call() {
    let a = ok(
        Tag::Button,
        &[
            ("data-event", "open_tree"),
            ("data-event-arg", "{{ game.turn }}"),
        ],
    );
    assert_eq!(a.events.len(), 1);
    assert_eq!(a.events[0].ev, Event::Click);
    assert!(
        matches!(&a.events[0].actions[0], Action::Call { name, args }
        if name == "open_tree" && args.len() == 1 && args[0].name.is_none())
    );
}

#[test]
fn slots_layers_anchors_hotkeys() {
    let a = ok(
        Tag::Div,
        &[
            ("data-slot", "top.right"),
            ("data-part", "top-strip"),
            ("data-layer", "hud"),
        ],
    );
    assert_eq!(a.slot.as_deref(), Some("top.right"));
    assert_eq!(a.part.as_deref(), Some("top-strip"));
    assert_eq!(a.layer, Some(Layer::Hud));
    let a = parse(
        &raw(Tag::Div, &[("data-screen", "production")]),
        &AttrCtx {
            layer: Some(Layer::Window),
            ..AttrCtx::default()
        },
    )
    .expect("screen");
    assert_eq!(a.screen.as_deref(), Some("production"));
    let a = parse(
        &raw(
            Tag::Div,
            &[
                ("data-bind-for", "c:{{ game.cities }}"),
                ("data-anchor", "city:{{ c.id }}"),
            ],
        ),
        &AttrCtx {
            layer: Some(Layer::Map),
            ..AttrCtx::default()
        },
    )
    .expect("anchor");
    assert_eq!(a.anchor.as_ref().map(|(k, _)| *k), Some(AnchorKind::City));
    let a = ok(Tag::Button, &[("data-hotkey", "Ctrl+Shift+KeyB")]);
    let k = a.hotkey.expect("hotkey");
    assert!(k.ctrl && !k.alt && k.shift);
    assert_eq!((k.code.as_str(), k.mods()), ("KeyB", 5));
    for code in [
        "F5",
        "F24",
        "Digit3",
        "Numpad0",
        "Escape",
        "ArrowLeft",
        "Alt+Enter",
    ] {
        ok(Tag::Button, &[("data-hotkey", code)]);
    }
    let a = ok(Tag::Span, &[("data-hotkey-label", "")]);
    assert!(a.hotkey_label);
}

#[test]
fn tooltips_fit_islands_controls() {
    let a = ok(
        Tag::Div,
        &[
            ("data-tooltip", "tip-turn"),
            ("data-fit", "shrink"),
            ("data-fit-min", "12px"),
        ],
    );
    assert_eq!(a.tooltip.as_deref(), Some("tip-turn"));
    assert_eq!(a.fit, Some(Some(12.0)));
    let a = ok(
        Tag::Div,
        &[
            ("data-ui2-island", "clock"),
            ("data-ui2-classes", "on off, warn"),
        ],
    );
    let island = a.island.expect("island");
    assert_eq!((island.id.as_str(), island.classes.len()), ("clock", 3));
    let a = parse(
        &raw(Tag::Span, &[("data-ui2-key", "hours")]),
        &AttrCtx {
            in_island: true,
            ..AttrCtx::default()
        },
    )
    .expect("key");
    assert_eq!(a.island_key.as_deref(), Some("hours"));

    let a = ok(
        Tag::Div,
        &[
            ("data-control", "checkbox"),
            ("data-bind-model", "state.prod.open"),
        ],
    );
    assert_eq!(a.control.expect("control").kind, ControlKind::Checkbox);
    assert_eq!(a.model.as_deref(), Some("prod.open"));
    let a = ok(
        Tag::Div,
        &[
            ("data-control", "slider"),
            ("data-bind-model", "state.volume"),
            ("data-min", "0"),
            ("data-max", "100"),
            ("data-step", "5"),
        ],
    );
    assert_eq!(a.control.expect("slider").range, Some((0.0, 100.0, 5.0)));
    let a = ok(
        Tag::Div,
        &[
            ("data-control", "radio"),
            ("data-bind-model", "state.mode"),
            ("data-value", "a"),
        ],
    );
    assert_eq!(a.control.expect("radio").value.as_deref(), Some("a"));
    let a = ok(
        Tag::Div,
        &[
            ("data-control", "select"),
            ("data-bind-model", "state.mode"),
            ("data-options", "{{ state.tabs }}"),
        ],
    );
    assert!(a.control.expect("select").options.is_some());
}

#[test]
fn plain_attributes_pass_and_the_preview_flag_is_kept() {
    let a = ok(
        Tag::Input,
        &[
            ("id", "chat"),
            ("type", "text"),
            ("tabindex", "0"),
            ("aria-label", "x"),
            ("disabled", ""),
            ("data-ui2-dev", ""),
        ],
    );
    assert_eq!(a.plain.len(), 5);
    assert!(a.dev);
    ok(
        Tag::Img,
        &[
            ("src", "a.png"),
            ("width", "10"),
            ("height", "10"),
            ("data-asset", "render:minimap"),
        ],
    );
    ok(Tag::Button, &[("type", "button"), ("hidden", "")]);
}

// ---- parsing: the error codes ---------------------------------------------------------------

#[test]
fn attribute_names_are_checked() {
    expect(Tag::Div, &[("title", "x")], codes::TITLE);
    expect(Tag::Div, &[("onclick", "go()")], codes::ON_ATTR);
    expect(Tag::Div, &[("data-bind-vlaue", "x")], codes::UNKNOWN_DATA);
    expect(Tag::Div, &[("data-on-tap", "stop")], codes::UNKNOWN_DATA);
    expect(Tag::Img, &[("srcset", "a.png 2x")], codes::SRCSET);
    expect(Tag::Div, &[("alt", "x")], codes::OUTSIDE);
    expect(Tag::Div, &[("href", "x")], codes::NOT_ON_ELEMENT);
    expect(Tag::Div, &[("disabled", "")], codes::NOT_ON_ELEMENT);
    expect(Tag::Div, &[("src", "a.png")], codes::NOT_ON_ELEMENT);
    expect(
        Tag::Div,
        &[("data-bind-src", "{{ a }}")],
        codes::NOT_ON_ELEMENT,
    );
    expect(Tag::Img, &[("data-bind-value", "x")], codes::NOT_ON_ELEMENT);
    expect(Tag::Div, &[("class", "a"), ("class", "b")], codes::REPEATED);
    expect(Tag::Input, &[("type", "checkbox")], codes::VALUE);
    expect(
        Tag::Div,
        &[("data-on-submit", "stop")],
        codes::NOT_ON_ELEMENT,
    );
    for bad in [
        "/abs.html",
        "../up.html",
        "a/../b.html",
        "a\\b.html",
        "http://x/y.html",
        "",
    ] {
        expect(Tag::Div, &[("data-include", bad)], codes::INCLUDE);
    }
}

#[test]
fn binding_values_are_checked() {
    expect(
        Tag::Div,
        &[("data-bind-if", "{{a}} {{b}}")],
        codes::ONE_EXPR,
    );
    expect(Tag::Div, &[("data-bind-if", "a")], codes::ONE_EXPR);
    expect(
        Tag::Div,
        &[("data-bind-if", "{{ 1 + }}")],
        expr::codes::SYNTAX,
    );
    expect(Tag::Div, &[("data-bind-if", "{{ a")], expr::codes::UNCLOSED);
    for bad in [
        "{{ game.cities }}",
        "c {{ game.cities }}",
        "a, b, c:{{ game.cities }}",
        "1c:{{ game.cities }}",
    ] {
        expect(Tag::Li, &[("data-bind-for", bad)], codes::FOR);
    }
    for attr in [
        "data-bind-key",
        "data-bind-sort",
        "data-bind-sort-desc",
        "data-bind-filter",
    ] {
        expect(Tag::Li, &[(attr, "{{ 1 }}")], codes::NEEDS);
    }
    expect(Tag::Li, &[("data-virtual", "")], codes::NEEDS);
    expect(Tag::Li, &[("data-virtual", "{{ 1 }}")], codes::VALUE);
    expect(
        Tag::Div,
        &[("data-bind-class-toggle", "x:y")],
        codes::ONE_EXPR,
    );
    for bad in ["", "x", "1x:{{ a }}", "x:{{ a }}; x:{{ b }}", ";"] {
        expect(
            Tag::Div,
            &[("data-bind-class-toggle", bad)],
            codes::CLASS_TOGGLE,
        );
    }
    expect(
        Tag::Div,
        &[("data-bind-style-min-width", "{{ 1 }}px")],
        codes::STYLE_PROP,
    );
    expect(
        Tag::Div,
        &[("data-bind-style-float", "left")],
        codes::STYLE_PROP,
    );
    // A value that does not need the runtime is checked by the value parser now.
    expect(Tag::Div, &[("data-bind-style-width", "5vw")], "E2-003");
    expect(Tag::Span, &[("data-l10n-args", "{{ a }}")], codes::NEEDS);
    expect(
        Tag::Span,
        &[("data-bind-if", "{{ a }}"), ("data-bind-for", "c:{{ b }}")],
        codes::IF_FOR,
    );
    expect(
        Tag::Span,
        &[("data-bind-value", "x"), ("data-l10n", "k")],
        codes::TEXT_SOURCE,
    );
    let with_children = AttrCtx {
        has_children: true,
        ..AttrCtx::default()
    };
    assert!(
        fails(Tag::Div, &[("data-bind-value", "x")], &with_children).contains(&codes::TEXT_SOURCE)
    );
    for bad in ["Upper", "a b", "", "1a", "a..b", "a."] {
        expect(Tag::Span, &[("data-l10n", bad)], codes::L10N_KEY);
    }
    expect(Tag::Img, &[("data-asset", "minimap")], codes::ASSET);
    expect(Tag::Img, &[("data-asset", "Render:x")], codes::ASSET);
}

#[test]
fn action_syntax_is_checked() {
    for bad in [
        "jump",
        "",
        ";",
        "intent",
        "intent unit.action",
        "intent unit.action(",
        "intent 1x(a)",
        "intent a(, b)",
        "intent a(x: 1, 2)",
        "intent a(x: 1, x: 2)",
        "set x = 1",
        "set state.x",
        "toggle x",
        "toggle state.",
        "stop now",
        "call a b(c)",
    ] {
        expect(Tag::Button, &[("data-on-click", bad)], codes::ACTION);
    }
    expect(
        Tag::Button,
        &[("data-on-click", "set state.x = 1 +")],
        expr::codes::SYNTAX,
    );
    expect(
        Tag::Button,
        &[("data-on-click", "set state.x == 1")],
        expr::codes::SYNTAX,
    );
    expect(
        Tag::Li,
        &[("data-bind-for", "c:{{ game.cities }} x")],
        codes::ONE_EXPR,
    );
    expect(
        Tag::Button,
        &[("data-on-click", "call f(nope(1))")],
        expr::codes::UNKNOWN_FUNC,
    );
}

#[test]
fn control_attributes_are_checked() {
    expect(
        Tag::Div,
        &[("data-control", "dial"), ("data-bind-model", "state.x")],
        codes::CONTROL,
    );
    expect(Tag::Div, &[("data-control", "checkbox")], codes::CONTROL);
    expect(
        Tag::Div,
        &[("data-control", "radio"), ("data-bind-model", "state.x")],
        codes::CONTROL,
    );
    expect(
        Tag::Div,
        &[
            ("data-control", "checkbox"),
            ("data-bind-model", "state.x"),
            ("data-min", "1"),
        ],
        codes::CONTROL,
    );
    expect(
        Tag::Div,
        &[
            ("data-control", "slider"),
            ("data-bind-model", "state.x"),
            ("data-min", "5"),
            ("data-max", "1"),
            ("data-step", "1"),
        ],
        codes::CONTROL,
    );
    expect(
        Tag::Div,
        &[
            ("data-control", "slider"),
            ("data-bind-model", "state.x"),
            ("data-min", "a"),
            ("data-max", "1"),
            ("data-step", "1"),
        ],
        codes::CONTROL,
    );
    expect(Tag::Div, &[("data-value", "a")], codes::CONTROL);
    expect(Tag::Div, &[("data-bind-model", "volume")], codes::MODEL);
    expect(Tag::Div, &[("data-bind-model", "state.")], codes::MODEL);
    expect(
        Tag::Div,
        &[("data-bind-model", "{{ state.x }}")],
        codes::MODEL,
    );
    expect(
        Tag::Div,
        &[("data-event", "x"), ("data-on-click", "stop")],
        codes::EVENT_CLICK,
    );
    expect(Tag::Div, &[("data-event-arg", "{{ 1 }}")], codes::NEEDS);
}

#[test]
fn extension_attributes_are_checked() {
    expect(Tag::Div, &[("data-slot", "Top")], codes::SLOT);
    expect(Tag::Div, &[("data-part", "a..b")], codes::SLOT);
    expect(
        Tag::Div,
        &[
            ("data-slot", "top"),
            ("data-bind-for", "c:{{ game.cities }}"),
        ],
        codes::SLOT_NODE,
    );
    expect(Tag::Div, &[("data-layer", "overlay")], codes::LAYER);
    expect(Tag::Div, &[("data-screen", "Prod")], codes::LAYER);
    let hud = AttrCtx {
        layer: Some(Layer::Hud),
        ..AttrCtx::default()
    };
    assert!(fails(Tag::Div, &[("data-screen", "production")], &hud).contains(&codes::LAYER));
    expect(
        Tag::Div,
        &[("data-anchor", "city:{{ c.id }}")],
        codes::ANCHOR,
    );
    expect(
        Tag::Div,
        &[
            ("data-anchor", "tree:{{ c.id }}"),
            ("data-bind-for", "c:{{ x }}"),
        ],
        codes::ANCHOR,
    );
    expect(
        Tag::Div,
        &[
            ("data-anchor", "{{ c.id }}"),
            ("data-bind-for", "c:{{ x }}"),
        ],
        codes::ANCHOR,
    );
    assert!(
        fails(
            Tag::Div,
            &[
                ("data-anchor", "city:{{ c.id }}"),
                ("data-bind-for", "c:{{ x }}")
            ],
            &hud
        )
        .contains(&codes::ANCHOR)
    );
    for bad in [
        "ctrl+KeyB",
        "Shift+Ctrl+KeyB",
        "Ctrl+Ctrl+KeyB",
        "KeyBB",
        "Key1",
        "F0",
        "F25",
        "Meta+KeyB",
        "",
        "Ctrl+",
    ] {
        expect(Tag::Button, &[("data-hotkey", bad)], codes::HOTKEY);
    }
    expect(Tag::Span, &[("data-hotkey-label", "x")], codes::HOTKEY);
    let in_for = AttrCtx {
        in_for: true,
        ..AttrCtx::default()
    };
    assert!(fails(Tag::Button, &[("data-hotkey", "KeyB")], &in_for).contains(&codes::HOTKEY_FOR));
    expect(
        Tag::Button,
        &[("data-hotkey", "KeyB"), ("data-bind-for", "c:{{ x }}")],
        codes::HOTKEY_FOR,
    );
    expect(Tag::Div, &[("data-tooltip", "1tip")], codes::TOOLTIP);
    expect(Tag::Div, &[("data-fit", "grow")], codes::FIT);
    expect(Tag::Div, &[("data-fit-min", "12px")], codes::FIT);
    expect(
        Tag::Div,
        &[("data-fit", "shrink"), ("data-fit-min", "-3")],
        codes::FIT,
    );
    expect(Tag::Div, &[("data-ui2-island", "Clock")], codes::ISLAND);
    expect(Tag::Div, &[("data-ui2-classes", "a")], codes::ISLAND);
    expect(Tag::Div, &[("data-ui2-key", "k")], codes::ISLAND);
    expect(
        Tag::Div,
        &[("data-ui2-island", "clock"), ("data-bind-if", "{{ a }}")],
        codes::ISLAND_BIND,
    );
    let in_island = AttrCtx {
        in_island: true,
        ..AttrCtx::default()
    };
    assert!(
        fails(Tag::Span, &[("data-bind-value", "{{ a }}")], &in_island)
            .contains(&codes::ISLAND_BIND)
    );
}

#[test]
fn every_error_is_reported_not_only_the_first() {
    let got = fails(
        Tag::Div,
        &[("title", "x"), ("data-slot", "Bad"), ("onclick", "y")],
        &AttrCtx::default(),
    );
    assert_eq!(got, [codes::TITLE, codes::SLOT, codes::ON_ATTR]);
}

#[test]
fn every_attribute_code_has_a_text_and_a_case() {
    let profile = Profile::v1();
    for code in codes::ALL {
        let def = profile
            .error(code)
            .unwrap_or_else(|| panic!("no text for {code}"));
        assert!(!def.what.is_empty() && !def.hint.is_empty(), "{code}");
    }
    let unique: BTreeSet<_> = codes::ALL.iter().collect();
    assert_eq!(unique.len(), codes::ALL.len());
}

// ---- checking against a schema -------------------------------------------------------------

fn view() -> MapView {
    MapView::default()
        .ty("game.turn", "int")
        .ty("game.alive", "bool")
        .ty("game.cities", "list<struct game.cities[]>")
        .ty("game.cities[].id", "key<city>")
        .ty("game.cities[].name", "str")
        .ty("game.cities[].pop", "int")
        .ty("game.units", "list<struct game.units[]>")
        .ty("game.units[].key", "key<unit>")
        .ty("game.units[].hp", "int")
        .ty("game.units[].portrait", "asset")
        .ty("game.tags", "list<str>")
        .ty("game.accent", "color")
        .ty("ui.panel", "str")
        .ty("ui.zoom", "float")
        .ty("ui.fixed", "int")
        .key("game.cities[]", "id")
        .key("game.units[]", "key")
        .writable("ui.panel")
        .writable("ui.zoom")
        .intent(
            "unit.action",
            &[("unit", "key<unit>"), ("action", "enum(fortify|sleep)")],
        )
        .intent(
            "city.focus",
            &[
                ("city", "key<city>"),
                ("focus", "enum(food|prod)"),
                ("note", "str?"),
            ],
        )
}

fn scope() -> Scope {
    Scope::new()
        .with_state("prod.sort", TypeSig::Str)
        .with_state("prod.desc", TypeSig::Bool)
        .with_state("prod.page", TypeSig::Int)
        .with_state("prod.open", TypeSig::Bool)
        .with_state("volume", TypeSig::Float)
        .with_state("tabs", parse_type("list<str>"))
        .with_state("mode", TypeSig::Str)
}

fn run(
    tag: Tag,
    attrs: &[(&str, &str)],
) -> Result<(NodeAttrs, ui2_package::profile::attrs::Checked), Vec<&'static str>> {
    run_in(tag, attrs, &scope(), &AttrCtx::default())
}

fn run_in(
    tag: Tag,
    attrs: &[(&str, &str)],
    scope: &Scope,
    ctx: &AttrCtx,
) -> Result<(NodeAttrs, ui2_package::profile::attrs::Checked), Vec<&'static str>> {
    let mut a = parse(&raw(tag, attrs), ctx).unwrap_or_else(|e| panic!("{attrs:?}: parse {e:?}"));
    match check(&mut a, scope, &view(), ctx) {
        Ok(c) => Ok((a, c)),
        Err(es) => Err(es.iter().map(|e| e.code).collect()),
    }
}

fn check_fails(tag: Tag, attrs: &[(&str, &str)], code: &str) {
    match run(tag, attrs) {
        Ok(_) => panic!("{attrs:?}: expected {code}"),
        Err(got) => assert!(got.contains(&code), "{attrs:?}: got {got:?}, want {code}"),
    }
}

#[test]
fn a_loop_derives_its_key_and_opens_a_scope() {
    let (a, c) = run(
        Tag::Li,
        &[
            ("data-bind-for", "i, c:{{ game.cities }}"),
            ("data-bind-value", "{{ i }} {{ c.name }}"),
        ],
    )
    .expect("check");
    let key = a.for_.as_ref().and_then(|f| f.key.clone());
    assert_eq!(key, Some(Expr::Path(vec!["c".into(), "id".into()])));
    assert_eq!(c.scope.vars["c"].origin.as_deref(), Some("game.cities[]"));
    assert_eq!(c.scope.vars["i"].ty, TypeSig::Int);
    let paths: Vec<&str> = c.paths.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, ["game.cities", "game.cities[].name"]);
    // The scope serves the children: a nested loop sees the outer item.
    let nested = Scope { ..c.scope.clone() };
    assert!(nested.vars.contains_key("c"));
}

#[test]
fn loops_need_keys_lists_and_free_names() {
    check_fails(
        Tag::Li,
        &[("data-bind-for", "t:{{ state.tabs }}")],
        codes::NO_KEY,
    );
    run(
        Tag::Li,
        &[
            ("data-bind-for", "t:{{ state.tabs }}"),
            ("data-bind-key", "{{ t }}"),
        ],
    )
    .expect("explicit key");
    check_fails(
        Tag::Li,
        &[("data-bind-for", "t:{{ game.turn }}")],
        expr::codes::NOT_LIST,
    );
    check_fails(
        Tag::Li,
        &[("data-bind-for", "game:{{ game.cities }}")],
        expr::codes::VAR_NAME,
    );
    check_fails(
        Tag::Li,
        &[("data-bind-for", "state:{{ game.cities }}")],
        expr::codes::VAR_NAME,
    );
    check_fails(
        Tag::Li,
        &[("data-bind-for", "c:{{ game.nope }}")],
        expr::codes::UNKNOWN_PATH,
    );
    check_fails(
        Tag::Li,
        &[
            ("data-bind-for", "c:{{ game.cities }}"),
            ("data-bind-sort", "{{ c.pop > 1 }}"),
        ],
        codes::TYPE,
    );
    run(
        Tag::Li,
        &[
            ("data-bind-for", "c:{{ game.cities }}"),
            ("data-bind-sort", "{{ state.prod.desc ? c.pop : c.name }}"),
            ("data-bind-filter", "{{ c.pop > 1 }}"),
        ],
    )
    .expect("sort and filter");
    // The sort direction is read once, outside the item.
    check_fails(
        Tag::Li,
        &[
            ("data-bind-for", "c:{{ game.cities }}"),
            ("data-bind-sort-desc", "{{ c.pop > 1 }}"),
        ],
        expr::codes::UNKNOWN_PATH,
    );
}

#[test]
fn intents_are_checked_and_named() {
    let (a, c) = run(
        Tag::Button,
        &[
            ("data-bind-for", "u:{{ game.units }}"),
            ("data-on-click", "intent unit.action(u.key, 'sleep')"),
        ],
    )
    .expect("positional");
    let Action::Intent { args, .. } = &a.events[0].actions[0] else {
        panic!("intent");
    };
    let names: Vec<_> = args
        .iter()
        .map(|a| a.name.clone().unwrap_or_default())
        .collect();
    assert_eq!(names, ["unit", "action"]);
    assert_eq!(c.intents.len(), 1);
    assert_eq!(c.intents[0].0, "unit.action");
    assert_eq!(c.intents[0].1[0], TypeSig::Key("unit".into()));

    // Named arguments in any order are put in declaration order; an optional one may be left out.
    let (a, _) = run(
        Tag::Button,
        &[
            ("data-bind-for", "c:{{ game.cities }}"),
            (
                "data-on-click",
                "intent city.focus(focus: 'prod', city: c.id)",
            ),
        ],
    )
    .expect("named");
    let Action::Intent { args, .. } = &a.events[0].actions[0] else {
        panic!("intent");
    };
    assert_eq!(args[0].name.as_deref(), Some("city"));
    assert_eq!(args[1].name.as_deref(), Some("focus"));
}

#[test]
fn intent_errors() {
    let in_loop = |action: &str| {
        run(
            Tag::Button,
            &[
                ("data-bind-for", "u:{{ game.units }}"),
                ("data-on-click", action),
            ],
        )
    };
    let code = |r: Result<_, Vec<&'static str>>| match r {
        Ok(_) => "ok",
        Err(es) => es[0],
    };
    assert_eq!(
        code(in_loop("intent unit.dance(u.key)")),
        codes::UNKNOWN_INTENT
    );
    assert_eq!(
        code(in_loop("intent unit.action(u.key)")),
        codes::INTENT_ARGS
    );
    assert_eq!(
        code(in_loop("intent unit.action(u.key, 'sleep', 1)")),
        codes::INTENT_ARGS
    );
    assert_eq!(
        code(in_loop("intent unit.action(unit: u.key, act: 'sleep')")),
        codes::INTENT_ARGS
    );
    assert_eq!(
        code(in_loop("intent unit.action(u.key, 'dance')")),
        expr::codes::ENUM_VALUE
    );
    assert_eq!(code(in_loop("intent unit.action(5, 'sleep')")), codes::TYPE);
    assert_eq!(
        code(in_loop("intent unit.action(u.hp, 'sleep')")),
        codes::TYPE
    );
    assert_eq!(
        code(in_loop("intent unit.action(null, 'sleep')")),
        codes::TYPE
    );
    assert_eq!(code(in_loop("intent mods.demo.go(1)")), codes::MOD_INTENT);
}

#[test]
fn mod_fragments_send_only_their_own_intents() {
    let frag = AttrCtx {
        mod_id: Some("my-mod".into()),
        ..AttrCtx::default()
    };
    run_in(
        Tag::Button,
        &[("data-on-click", "intent mods.my-mod.go(1, 'x')")],
        &scope(),
        &frag,
    )
    .expect("own");
    let other = run_in(
        Tag::Button,
        &[("data-on-click", "intent mods.other.go()")],
        &scope(),
        &frag,
    );
    assert_eq!(other.err().map(|e| e[0]), Some(codes::MOD_INTENT));
    let bare = run_in(
        Tag::Button,
        &[("data-on-click", "intent mods.my-mod()")],
        &scope(),
        &frag,
    );
    assert_eq!(bare.err().map(|e| e[0]), Some(codes::MOD_INTENT));
    run_in(
        Tag::Button,
        &[(
            "data-on-click",
            "intent city.focus(city: null, focus: 'food')",
        )],
        &scope(),
        &frag,
    )
    .expect_err("null is not a city");
}

#[test]
fn set_and_toggle_write_only_where_markup_may() {
    let (_, c) = run(
        Tag::Button,
        &[("data-on-click", "set state.prod.page = state.prod.page + 1; toggle state.prod.open; set ui.panel = 'tech'; set ui.zoom = 2; set state.prod.sort = 'name'; call refresh(game.turn)")],
    )
    .expect("writes");
    assert_eq!(c.handlers, ["refresh"]);
    assert_eq!(c.paths[0].0, "game.turn");
    check_fails(
        Tag::Button,
        &[("data-on-click", "set state.nope = 1")],
        expr::codes::UNKNOWN_PATH,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "toggle state.nope")],
        expr::codes::UNKNOWN_PATH,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "set ui.fixed = 1")],
        codes::NOT_WRITABLE,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "set ui.nope = 1")],
        expr::codes::UNKNOWN_PATH,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "set state.prod.page = 'x'")],
        codes::TYPE,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "set state.prod.page = null")],
        codes::TYPE,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "toggle state.prod.page")],
        codes::TYPE,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "toggle ui.panel")],
        codes::TYPE,
    );
    check_fails(
        Tag::Button,
        &[("data-on-click", "call go(game.nope)")],
        expr::codes::UNKNOWN_PATH,
    );
}

#[test]
fn models_sources_and_anchors_are_typed() {
    run(
        Tag::Div,
        &[
            ("data-control", "checkbox"),
            ("data-bind-model", "state.prod.open"),
        ],
    )
    .expect("bool");
    check_fails(
        Tag::Div,
        &[
            ("data-control", "checkbox"),
            ("data-bind-model", "state.prod.sort"),
        ],
        codes::TYPE,
    );
    check_fails(
        Tag::Div,
        &[
            ("data-control", "checkbox"),
            ("data-bind-model", "state.nope"),
        ],
        expr::codes::UNKNOWN_PATH,
    );
    run(
        Tag::Div,
        &[
            ("data-control", "slider"),
            ("data-bind-model", "state.volume"),
            ("data-min", "0"),
            ("data-max", "1"),
            ("data-step", "0.1"),
        ],
    )
    .expect("slider");
    check_fails(
        Tag::Div,
        &[
            ("data-control", "slider"),
            ("data-bind-model", "state.prod.sort"),
            ("data-min", "0"),
            ("data-max", "1"),
            ("data-step", "1"),
        ],
        codes::TYPE,
    );
    check_fails(
        Tag::Div,
        &[
            ("data-control", "select"),
            ("data-bind-model", "state.mode"),
            ("data-options", "{{ game.turn }}"),
        ],
        expr::codes::NOT_LIST,
    );
    run(
        Tag::Input,
        &[("type", "text"), ("data-bind-model", "state.mode")],
    )
    .expect("text input");
    check_fails(
        Tag::Input,
        &[("type", "text"), ("data-bind-model", "state.prod.open")],
        codes::TYPE,
    );

    run(
        Tag::Img,
        &[
            ("data-bind-for", "u:{{ game.units }}"),
            ("data-bind-src", "{{ u.portrait }}"),
        ],
    )
    .expect("asset");
    check_fails(
        Tag::Img,
        &[("data-bind-src", "{{ state.mode }}")],
        codes::TYPE,
    );
    check_fails(
        Tag::Div,
        &[("data-bind-style-color", "{{ game.turn }}")],
        codes::TYPE,
    );
    run(Tag::Div, &[("data-bind-style-color", "{{ game.accent }}")]).expect("color");
    check_fails(
        Tag::Span,
        &[("data-bind-value", "{{ game.alive }}")],
        expr::codes::TEXT_TYPE,
    );
    check_fails(
        Tag::Span,
        &[("data-l10n", "k"), ("data-l10n-args", "{{ game.cities }}")],
        codes::TYPE,
    );

    let map = AttrCtx {
        layer: Some(Layer::Map),
        ..AttrCtx::default()
    };
    let anchored = |kind: &str, list: &str, id: &str| {
        run_in(
            Tag::Div,
            &[
                ("data-bind-for", list),
                ("data-anchor", &format!("{kind}:{{{{ {id} }}}}")),
            ],
            &scope(),
            &map,
        )
    };
    anchored("city", "c:{{ game.cities }}", "c.id").expect("city");
    anchored("unit", "u:{{ game.units }}", "u.key").expect("unit");
    anchored("plot", "u:{{ game.units }}", "u.key").expect("plot takes any key");
    assert_eq!(
        anchored("unit", "c:{{ game.cities }}", "c.id")
            .err()
            .map(|e| e[0]),
        Some(codes::TYPE)
    );
    assert_eq!(
        anchored("city", "c:{{ game.cities }}", "c.name")
            .err()
            .map(|e| e[0]),
        Some(codes::TYPE)
    );
}
