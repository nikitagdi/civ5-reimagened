//! Selectors and media queries of the profile: the static part, the conditions, the viewport
//! cells and the limits (4 conditions per node, 8 media conditions, 16 cells).
#![cfg(feature = "profile")]

use ui2_package::profile::media::{self, Axis, Cells, MediaCond, Op};
use ui2_package::profile::select::{
    self, Comb, Combinator, Pseudo, SelInfo, Simple, check, check_list, check_toggled, codes,
    node_conds, parse, parse_list, variants,
};
use ui2_package::profile::{Profile, Tag};
use ui2_package::{Bound, Cond, CondKind, MAX_CELLS, MAX_CONDS, StrId, Styles};

fn ok(s: &str) -> SelInfo {
    check(&parse(s).unwrap_or_else(|e| panic!("{s}: {e}"))).unwrap_or_else(|e| panic!("{s}: {e}"))
}

fn code(s: &str) -> &'static str {
    match parse(s).and_then(|a| check(&a)) {
        Ok(_) => panic!("{s} is accepted"),
        Err(e) => e.code,
    }
}

// ---------------------------------------------------------------------------------------------
// Selectors

#[test]
fn select_static_parts_of_the_mockup() {
    let s = ok(".top .grp");
    assert_eq!(s.static_part.len(), 2);
    assert_eq!(s.static_part[0].0, None);
    assert_eq!(s.static_part[1].0, Some(Comb::Descendant));
    assert_eq!(s.subject().classes, ["grp"]);
    assert!(s.conds.is_empty() && !s.placeholder);

    let s = ok(".yields .yield>svg.i");
    assert_eq!(s.static_part[2].0, Some(Comb::Child));
    assert_eq!(s.subject().tag, Some(Tag::Svg));
    assert_eq!(s.subject().classes, ["i"]);

    let s = ok("div#main.a.b");
    let c = s.subject();
    assert_eq!(
        (c.tag, c.ids.as_slice(), c.classes.as_slice()),
        (
            Some(Tag::Div),
            &["main".to_string()][..],
            &["a".to_string(), "b".to_string()][..]
        )
    );
}

#[test]
fn select_root_universal_and_case() {
    assert!(ok(":root").subject().root);
    assert_eq!(ok("*").subject(), &Default::default());
    assert_eq!(ok("*.x").subject().classes, ["x"]);
    assert_eq!(ok("DIV").subject().tag, Some(Tag::Div));
    assert_eq!(ok(".a:HOVER").conds[0].pseudo, Pseudo::Hover);
}

#[test]
fn select_spaces_around_child_combinators() {
    for s in [".a>.b", ".a > .b", ".a\n>\t.b", " .a  >  .b "] {
        let info = ok(s);
        assert_eq!(info.static_part.len(), 2, "{s}");
        assert_eq!(info.static_part[1].0, Some(Comb::Child), "{s}");
    }
    let info = ok(".a  .b   .c");
    assert_eq!(info.static_part.len(), 3);
}

#[test]
fn select_conditions_belong_to_their_compound() {
    let s = ok(".note:hover .x");
    assert_eq!(s.conds.len(), 1);
    assert_eq!((s.conds[0].part, s.conds[0].pseudo), (0, Pseudo::Hover));
    let s = ok(".a:focus-visible > button:disabled:active");
    let got: Vec<(usize, Pseudo)> = s.conds.iter().map(|c| (c.part, c.pseudo)).collect();
    assert_eq!(
        got,
        [
            (0, Pseudo::FocusVisible),
            (1, Pseudo::Disabled),
            (1, Pseudo::Active)
        ]
    );
    // The same pseudo-class twice in a compound is one condition.
    assert_eq!(ok(".a:hover:hover").conds.len(), 1);
    // ... but in two compounds it is two (two elements).
    assert_eq!(ok(".a:hover .b:hover").conds.len(), 2);
}

#[test]
fn select_every_dynamic_pseudo_class_maps_to_a_condition_kind() {
    let kinds = [
        CondKind::Hover,
        CondKind::Active,
        CondKind::Focus,
        CondKind::FocusVisible,
        CondKind::Disabled,
    ];
    for (p, k) in Pseudo::ALL.iter().zip(kinds) {
        assert_eq!(p.cond_kind(), k);
        assert_eq!(Pseudo::from_name(p.name()), Some(*p));
        assert_eq!(ok(&format!(".a:{}", p.name())).conds[0].pseudo, *p);
    }
    // The table of profile.toml lists the same pseudo-classes and combinators.
    let table = Profile::v1().selector();
    let names: Vec<&str> = Pseudo::ALL.iter().map(|p| p.name()).collect();
    assert_eq!(table.pseudo_class, names);
    assert_eq!(table.combinators, [" ", ">"]);
}

#[test]
fn select_dynamic_classes_are_found_by_the_toggle_list() {
    let s = ok(".is-chooser .city.sel:hover");
    let toggled = |k: &str| k == "is-chooser" || k == "sel";
    let found: Vec<(usize, &str)> = s.dynamic_classes(&toggled).collect();
    assert_eq!(found, [(0, "is-chooser"), (1, "sel")]);
    assert!(s.is_dynamic(&|_| false)); // :hover alone
    assert!(!ok(".a .b").is_dynamic(&|_| false));
    assert!(ok(".a .b").is_dynamic(&|k| k == "a"));
}

#[test]
fn select_placeholder_only_on_input() {
    let s = ok("input::placeholder");
    assert!(s.placeholder);
    assert_eq!(s.subject().tag, Some(Tag::Input));
    assert!(ok(".chat input.msg::placeholder").placeholder);
    assert_eq!(select::PLACEHOLDER_PROPS, ["color"]);
    assert_eq!(code(".a::placeholder"), codes::SELECTOR);
    assert_eq!(code("input::placeholder .b"), codes::SELECTOR);
    assert_eq!(code("input::placeholder:hover"), codes::SELECTOR);
}

#[test]
fn select_outside_the_profile_is_e3_002() {
    for bad in [
        ".a+.b",
        ".a ~ .b",
        "[data-x]",
        "a[href]",
        "li:nth-child(2)",
        ".a:first-child",
        ".a:last-of-type",
        ".a:only-child",
        ".a:empty",
        ".a:checked",
        ".a:focus-within",
        ".a:link",
        ".a:not(.b)",
        ".a:is(.b, .c)",
        ".a:where(.b)",
        ".a:has(> .b)",
        ".x::after",
        ".x::before",
        "::selection",
        "svg|use",
        "*|div",
        "&.a",
        "table",
        "a",
    ] {
        assert_eq!(code(bad), codes::SELECTOR, "{bad}");
    }
}

#[test]
fn select_errors_name_what_is_outside() {
    let e = check(&parse(".a + .b").expect("parses")).expect_err("sibling");
    assert!(e.message.contains('+'), "{e}");
    let e = check(&parse("li:nth-child(2n+1)").expect("parses")).expect_err("nth");
    assert!(e.message.contains(":nth-child()"), "{e}");
    let e = check(&parse(".x::after").expect("parses")).expect_err("after");
    assert!(e.message.contains("::after"), "{e}");
    let e = check(&parse("canvas").expect("parses")).expect_err("canvas");
    assert!(e.message.contains("canvas"), "{e}");
}

#[test]
fn select_syntax_errors_are_e3_010() {
    for bad in [
        "",
        "  ",
        "> .a",
        ".a >",
        ".a > > .b",
        ".",
        "#",
        ".a[x",
        ".a:not(.b",
        ".a $ .b",
        ".a:",
    ] {
        assert_eq!(
            parse(bad).err().map(|e| e.code),
            Some(codes::SYNTAX),
            "{bad:?}"
        );
    }
    // A type selector after a class in one compound is not CSS.
    assert_eq!(parse(".a.b div").map(|s| s.parts.len()), Ok(2));
    assert_eq!(parse(".adiv").map(|s| s.parts.len()), Ok(1));
}

#[test]
fn select_the_general_ast_keeps_what_was_written() {
    let a = parse("svg|use.a[x='],'] + b:nth-child(2n + 1)::part(x)").expect("parses");
    assert_eq!(a.parts.len(), 2);
    assert_eq!(a.parts[1].0, Some(Combinator::NextSibling));
    assert_eq!(
        a.parts[0].1,
        [
            Simple::Namespace("svg".into()),
            Simple::Type("use".into()),
            Simple::Class("a".into()),
            Simple::Attribute("x='],'".into()),
        ]
    );
    assert_eq!(
        a.parts[1].1[1],
        Simple::PseudoClass {
            name: "nth-child".into(),
            arg: Some("2n + 1".into())
        }
    );
    // CSS escapes are decoded in names.
    let a = parse(".w-1\\/2.\\31 x").expect("escapes");
    assert_eq!(
        a.parts[0].1,
        [Simple::Class("w-1/2".into()), Simple::Class("1x".into())]
    );
}

#[test]
fn select_lists_split_on_top_level_commas() {
    let list = parse_list(".a, .b > .c,\n.d:hover").expect("list");
    assert_eq!(list.len(), 3);
    let list = parse_list(".a:is(.b, .c), [x=\",\"]").expect("list");
    assert_eq!(list.len(), 2);
    let infos = check_list("html, body, .a:hover").expect("profile list");
    assert_eq!(infos.len(), 3);
    assert_eq!(
        check_list(".a, .b + .c").err().map(|e| e.code),
        Some(codes::SELECTOR)
    );
    assert_eq!(
        parse_list(".a,,.b").err().map(|e| e.code),
        Some(codes::SYNTAX)
    );
    assert_eq!(parse_list(".a,").err().map(|e| e.code), Some(codes::SYNTAX));
}

// ---------------------------------------------------------------------------------------------
// Conditions of a node

fn cond(up: u8, kind: CondKind) -> Cond {
    Cond { up, kind }
}

#[test]
fn select_four_conditions_per_node() {
    let four = [
        cond(0, CondKind::Hover),
        cond(0, CondKind::Active),
        cond(1, CondKind::Hover),
        cond(2, CondKind::Class(StrId(3))),
    ];
    let got = node_conds(four).expect("four");
    assert_eq!(got, four);
    assert_eq!(variants(got.len()), 16);
    assert_eq!(MAX_CONDS, 4);

    // Duplicates (the same condition from two rules) count once and keep the first bit.
    let got = node_conds([four[1], four[0], four[1], four[0]]).expect("dups");
    assert_eq!(got, [four[1], four[0]]);

    let mut five = four.to_vec();
    five.push(cond(3, CondKind::Focus));
    assert_eq!(node_conds(five).err().map(|e| e.code), Some(codes::CONDS));
    assert!(node_conds([]).expect("none").is_empty());
    assert_eq!(variants(0), 1);
}

#[test]
fn select_one_selector_with_five_pseudo_classes_is_too_many() {
    assert_eq!(
        code("button:hover:active:focus:focus-visible:disabled"),
        codes::CONDS
    );
    assert_eq!(ok("button:hover:active:focus:disabled").conds.len(), 4);
}

#[test]
fn select_toggled_classes_are_not_static() {
    assert!(check_toggled(&["a", "b"], &["c"]).is_ok());
    assert!(check_toggled(&[], &["c"]).is_ok());
    let e = check_toggled(&["a", "sel"], &["sel"]).expect_err("sel");
    assert_eq!(e.code, codes::TOGGLED_STATIC);
    assert!(e.message.contains("sel"));
}

#[test]
fn select_codes_have_texts() {
    for c in codes::ALL {
        let def = Profile::v1()
            .error(c)
            .unwrap_or_else(|| panic!("no text for {c}"));
        assert!(!def.what.is_empty() && !def.hint.is_empty());
    }
}

// ---------------------------------------------------------------------------------------------
// Media queries

fn m(axis: Axis, op: Op, px: f32) -> MediaCond {
    MediaCond { axis, op, px }
}

fn media_code(s: &str) -> &'static str {
    match media::parse(s) {
        Ok(v) => panic!("{s} is accepted: {v:?}"),
        Err(e) => e.code,
    }
}

#[test]
fn media_min_max_and_types() {
    use Axis::{Height as H, Width as W};
    assert_eq!(
        media::parse("(min-width:1600px)").unwrap(),
        [m(W, Op::Ge, 1600.0)]
    );
    assert_eq!(
        media::parse("screen and (max-height: 800px)").unwrap(),
        [m(H, Op::Le, 800.0)]
    );
    assert_eq!(
        media::parse("only screen and (min-width:1280px) and (max-width:1599.98px)").unwrap(),
        [m(W, Op::Ge, 1280.0), m(W, Op::Le, 1599.98)]
    );
    assert_eq!(
        media::parse("ALL AND (MIN-HEIGHT:900PX)").unwrap(),
        [m(H, Op::Ge, 900.0)]
    );
    assert_eq!(media::parse("all").unwrap(), []);
}

#[test]
fn media_ranges() {
    use Axis::{Height as H, Width as W};
    assert_eq!(
        media::parse("(width >= 1280px) and (height < 900px)").unwrap(),
        [m(W, Op::Ge, 1280.0), m(H, Op::Lt, 900.0)]
    );
    assert_eq!(
        media::parse("(1280px<=width<1600px)").unwrap(),
        [m(W, Op::Ge, 1280.0), m(W, Op::Lt, 1600.0)]
    );
    assert_eq!(
        media::parse("(1600px < width)").unwrap(),
        [m(W, Op::Gt, 1600.0)]
    );
    assert_eq!(
        media::parse("(900px >= height > 600px)").unwrap(),
        [m(H, Op::Le, 900.0), m(H, Op::Gt, 600.0)]
    );
}

#[test]
fn media_outside_the_profile_is_e3_004() {
    for bad in [
        "",
        "print",
        "not screen",
        "only",
        "(prefers-color-scheme:dark)",
        "(prefers-reduced-motion: reduce)",
        "(orientation:landscape)",
        "(min-resolution:2dppx)",
        "(min-width:40em)",
        "(min-width: 50vw)",
        "(min-width: 0px)",
        "(width:100px)",
        "(min-width:100px),(max-width:50px)",
        "(min-width:100px) or (max-width:50px)",
        "(not (min-width:100px))",
        "((min-width:100px))",
        "(min-width:100px",
        "(min-width:100px) and",
        "(min-width:100px) (max-width:200px)",
        "(width = 100px)",
        "(width < height)",
        "(100px < width > 50px)",
        "screen and print",
        "(min-width:100px) and screen",
    ] {
        assert_eq!(media_code(bad), codes::MEDIA, "{bad:?}");
    }
}

#[test]
fn media_conditions_hold_like_css() {
    let vp = |w: f32| [w, 1000.0];
    let ge = m(Axis::Width, Op::Ge, 1600.0);
    assert!(ge.holds(vp(1600.0)) && !ge.holds(vp(1599.5)));
    let le = m(Axis::Width, Op::Le, 1600.0);
    assert!(le.holds(vp(1600.0)) && !le.holds(vp(1600.5)));
    let lt = m(Axis::Width, Op::Lt, 1600.0);
    assert!(!lt.holds(vp(1600.0)) && lt.holds(vp(1599.0)));
    let gt = m(Axis::Width, Op::Gt, 1600.0);
    assert!(!gt.holds(vp(1600.0)) && gt.holds(vp(1600.5)));
    // The stored bound puts the breakpoint value itself on the side where the condition says.
    for c in [ge, le, lt, gt] {
        let styles = Styles {
            media_w: vec![c.bound()],
            ..Styles::default()
        };
        let upper = styles.cell([1600.0, 1000.0]) == 1;
        assert_eq!(upper, matches!(c.op, Op::Ge | Op::Lt), "{c:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Viewport cells

fn conds(prelude: &[&str]) -> Vec<MediaCond> {
    prelude
        .iter()
        .flat_map(|p| media::parse(p).unwrap_or_else(|e| panic!("{p}: {e}")))
        .collect()
}

#[test]
fn cells_without_breakpoints_are_one_full_hd_cell() {
    let c = Cells::new(&[]).expect("cells");
    assert_eq!(c.count(), 1);
    assert_eq!(c.viewports(), [[1920, 1080]]);
    assert_eq!(c.cell([800.0, 600.0]), 0);
    assert!(c.applies(&[], 0));
}

#[test]
fn cells_follow_the_breakpoints_and_agree_with_the_package() {
    let all = conds(&[
        "(min-width:1280px)",
        "(min-width:1600px)",
        "(max-height:800px)",
        "(min-width:1280px) and (max-height:800px)", // repeats: counted once
    ]);
    let c = Cells::new(&all).expect("cells");
    assert_eq!(c.count(), 6);
    assert_eq!(
        c.media_w,
        [
            Bound {
                px: 1280.0,
                upper_inclusive: false
            },
            Bound {
                px: 1600.0,
                upper_inclusive: false
            }
        ]
    );
    let styles = Styles {
        media_w: c.media_w.clone(),
        media_h: c.media_h.clone(),
        ..Styles::default()
    };
    let vps = c.viewports();
    assert_eq!(
        vps,
        [
            [1279, 800],
            [1279, 801],
            [1280, 800],
            [1280, 801],
            [1600, 800],
            [1600, 801]
        ]
    );
    for (i, v) in vps.iter().enumerate() {
        let vf = [v[0] as f32, v[1] as f32];
        assert_eq!(usize::from(styles.cell(vf)), i);
        assert_eq!(c.cell(vf), i);
    }
}

#[test]
fn cells_tell_where_a_rule_applies() {
    let wide = conds(&["(min-width:1600px)"]);
    let short = conds(&["(max-height:800px)"]);
    let mid = conds(&["(1280px <= width < 1600px)"]);
    let mut all = wide.clone();
    all.extend(&short);
    all.extend(&mid);
    let c = Cells::new(&all).expect("cells");
    assert_eq!(c.count(), 6);
    let applies =
        |q: &[MediaCond]| -> Vec<usize> { (0..c.count()).filter(|&i| c.applies(q, i)).collect() };
    assert_eq!(applies(&wide), [4, 5]);
    assert_eq!(applies(&short), [0, 2, 4]);
    assert_eq!(applies(&mid), [2, 3]);
}

#[test]
fn cells_limits_eight_conditions_sixteen_cells() {
    assert_eq!(media::MAX_MEDIA_CONDS, 8);
    assert_eq!(MAX_CELLS, 16);
    // 3 x 3 breakpoints: 6 conditions, 16 cells: the most a skin may have.
    let w = [
        "(min-width:1000px)",
        "(min-width:1200px)",
        "(min-width:1400px)",
    ];
    let h = [
        "(min-height:600px)",
        "(min-height:700px)",
        "(min-height:800px)",
    ];
    let mut q: Vec<&str> = w.to_vec();
    q.extend(h);
    let c = Cells::new(&conds(&q)).expect("16 cells");
    assert_eq!(c.count(), 16);
    // One more breakpoint on either axis: 20 cells.
    q.push("(min-width:1600px)");
    let e = Cells::new(&conds(&q)).expect_err("20 cells");
    assert_eq!(e.code, codes::CELLS);
    // Fifteen width breakpoints would be 16 cells but 15 conditions.
    let many: Vec<String> = (1..=9)
        .map(|i| format!("(min-width:{}px)", i * 100))
        .collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    let e = Cells::new(&conds(&refs)).expect_err("nine conditions");
    assert_eq!(e.code, codes::CELLS);
    assert!(e.message.contains("9 media conditions"), "{e}");
    let eight: Vec<&str> = refs[..8].to_vec();
    assert_eq!(Cells::new(&conds(&eight)).expect("eight").count(), 9);
}

#[test]
fn cells_refuse_one_breakpoint_in_two_forms() {
    let e = Cells::new(&conds(&["(min-width:1600px)", "(max-width:1600px)"])).expect_err("same px");
    assert_eq!(e.code, codes::MEDIA);
    // `max-width: 1599.98px` next to `min-width: 1600px` is the usual way and is fine.
    let c = Cells::new(&conds(&["(min-width:1600px)", "(max-width:1599.98px)"])).expect("ok");
    assert_eq!(c.count(), 3);
    // `<` and `>=` at one value are one breakpoint.
    let c = Cells::new(&conds(&["(width < 1600px)", "(width >= 1600px)"])).expect("ok");
    assert_eq!(c.count(), 2);
}
