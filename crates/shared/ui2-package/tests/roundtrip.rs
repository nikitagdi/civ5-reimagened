//! Write and read back. Each section family adds its cases here.
#![cfg(feature = "write")]

use std::sync::Arc;

use ui2_package::style::{ColorV, Len, LenAuto, PropSel, Transition};
use ui2_package::{
    BlobRef, BlobSection, Bound, Cond, CondKind, Face, FaceFamily, FontRoles, ImageSrc, Images,
    Img, Kind, Layer, Manifest, Node, NodeKind, Package, PackageBuilder, PackageError, Page,
    PageFormat, PropId, Sprite, StrId, StyleId, StyleRecord, StyleSet, StyleSetId, Tag, Template,
    TemplateId, TextSrc, Theme, Tokens, Tool, node_flags, style,
};

fn manifest() -> Manifest {
    Manifest {
        kind: Kind::Skin,
        id: "test-skin".into(),
        version: "1.0.0".into(),
        title_key: "skin.title".into(),
        profile: (1, 0),
        model: (1, 0),
        api: (1, 0),
        default: true,
        tool: Tool {
            converter: "test 1".into(),
            chrome: "157".into(),
            chrome_flags: "00".into(),
            fonttools: "4".into(),
        },
        themes: vec!["dark".into()],
        languages: vec!["en_US".into()],
        font_roles: FontRoles {
            sans: FaceFamily("Sans".into()),
            display: FaceFamily("Display".into()),
        },
        schema_hash: 7,
    }
}

fn read(bytes: &[u8]) -> Result<Package, PackageError> {
    Package::read(Arc::from(bytes))
}

#[test]
fn empty_package_round_trips() {
    let bytes = PackageBuilder::new().finish(manifest()).expect("finish");
    let pkg = read(&bytes).expect("read");
    assert_eq!(pkg.manifest(), &manifest());
    // The builder names the manifest's theme in the string table.
    assert_eq!(pkg.str_count(), 1);
    assert_eq!(pkg.format_minor(), 0);
    assert_eq!(bytes.len() % 8, 0);
}

#[test]
fn hash_is_stable_and_names_the_file() {
    let a = PackageBuilder::new().finish(manifest()).expect("a");
    let b = PackageBuilder::new().finish(manifest()).expect("b");
    assert_eq!(a, b);
    let hash = read(&a).expect("read").hash();
    assert_eq!(&hash.0[..], &a[a.len() - 32..]);
    assert_eq!(hash.short().len(), 16);
    assert!(hash.short().bytes().all(|c| c.is_ascii_hexdigit()));
    let mut other = manifest();
    other.version = "1.0.1".into();
    let c = PackageBuilder::new().finish(other).expect("c");
    assert_ne!(read(&c).expect("read c").hash(), hash);
}

#[test]
fn strings_are_sorted_whatever_the_insertion_order() {
    let words = ["zeta", "alpha", "mid", "alpha", "Beta"];
    let mut forward = PackageBuilder::new();
    for w in words {
        forward.str(w);
    }
    let mut backward = PackageBuilder::new();
    for w in words.iter().rev() {
        backward.str(w);
    }
    let a = forward.finish(manifest()).expect("a");
    let b = backward.finish(manifest()).expect("b");
    assert_eq!(a, b);
    let pkg = read(&a).expect("read");
    assert_eq!(pkg.str_count(), 5); // the four words and the theme name
    let id = pkg.str_id("mid").expect("mid");
    assert_eq!(pkg.str(id), Some("mid"));
    assert_eq!(pkg.str_id("Beta").map(|i| i.0), Some(0)); // bytes sort: upper case first
    assert_eq!(pkg.str_id("missing"), None);
}

#[test]
fn a_bad_manifest_is_refused() {
    let mut m = manifest();
    m.id = "Bad Id".into();
    assert!(matches!(
        PackageBuilder::new().finish(m),
        Err(PackageError::Build(_)) | Err(PackageError::Invalid { .. })
    ));
    let mut m = manifest();
    m.themes.clear();
    assert!(PackageBuilder::new().finish(m).is_err());
}

#[test]
fn fragment_manifest_round_trips() {
    let mut m = manifest();
    m.kind = Kind::Fragment {
        slots: vec!["mods".into(), "top.right".into()],
    };
    let bytes = PackageBuilder::new().finish(m.clone()).expect("finish");
    assert_eq!(read(&bytes).expect("read").manifest(), &m);
}

#[test]
fn every_bit_flip_is_refused_and_no_prefix_panics() {
    let bytes = PackageBuilder::new().finish(manifest()).expect("finish");
    for i in 0..bytes.len() {
        for bit in [0u8, 3, 7] {
            let mut damaged = bytes.clone();
            damaged[i] ^= 1 << bit;
            assert!(read(&damaged).is_err(), "byte {i} bit {bit}");
        }
    }
    for n in 0..bytes.len() {
        assert!(read(&bytes[..n]).is_err(), "prefix {n}");
    }
}

#[test]
fn foreign_files_are_incompatible() {
    let mut bytes = PackageBuilder::new().finish(manifest()).expect("finish");
    bytes[0] = b'X';
    assert!(matches!(
        read(&bytes),
        Err(PackageError::Incompatible { .. })
    ));
    let mut bytes = PackageBuilder::new().finish(manifest()).expect("finish");
    bytes[8] = 2; // major 2
    assert!(matches!(
        read(&bytes),
        Err(PackageError::Incompatible { .. })
    ));
}

// ---------------------------------------------------------------------------------------------
// TMPL, STYL, TOKN: the top strip

fn rec(f: impl FnOnce(&mut StyleRecord)) -> StyleRecord {
    let mut r = StyleRecord::initial();
    f(&mut r);
    r
}

fn nodes(spec: &[(NodeKind, u32, StyleSetId, Option<StrId>, u16)]) -> Vec<Node> {
    spec.iter()
        .map(|(kind, subtree, style, part, flags)| Node {
            kind: kind.clone(),
            subtree: *subtree,
            style: *style,
            part: *part,
            flags: *flags,
        })
        .collect()
}

fn manifest_two_themes() -> Manifest {
    let mut m = manifest();
    m.themes = vec!["dark".into(), "light".into()];
    m
}

/// A small top strip: a HUD layer, a header, a turn button with a hover variant and a label.
/// `pre` interns words first, to show that the insertion order of strings does not matter.
fn top_strip(pre: &[&str]) -> PackageBuilder {
    let mut b = PackageBuilder::new();
    for w in pre {
        b.str(w);
    }
    b.media(
        vec![Bound {
            px: 1600.0,
            upper_inclusive: false,
        }],
        Vec::new(),
    );
    let sans = b.str("Alegreya");
    let serif = b.str("serif");
    let base = b.style_record(rec(|r| {
        r.display = style::Display::Flex;
        r.background_color = ColorV::Lit([12, 14, 20, 217]);
        r.color = ColorV::Tok(ui2_package::TokenId(0));
        r.font_family = vec![sans, serif];
        r.font_size = Len::px(14.0);
        r.padding_left = Len::px(8.0);
        r.width = LenAuto::Len(Len::pct(100.0));
        r.transitions = vec![Transition {
            prop: PropId::Color,
            dur_ms: 150.0,
            delay_ms: 0.0,
            ease: [0.25, 0.1, 0.25, 1.0],
        }];
    }));
    let base_wide = b.style_record(rec(|r| {
        r.display = style::Display::Flex;
        r.font_size = Len::px(16.0);
    }));
    let hover = b.style_record(rec(|r| {
        r.display = style::Display::Flex;
        r.color = ColorV::Tok(ui2_package::TokenId(1));
        r.cursor = style::Cursor::Pointer;
    }));
    // Equal records are one record.
    assert_eq!(
        b.style_record(rec(|r| {
            r.display = style::Display::Flex;
            r.font_size = Len::px(16.0);
        })),
        base_wide
    );

    let plain = b.style_set(StyleSet::plain(vec![base, base_wide]));
    let hoverable = b.style_set(StyleSet {
        conds: vec![Cond {
            up: 0,
            kind: CondKind::Hover,
        }],
        table: vec![base, hover, base_wide, hover],
    });
    let name = b.str("top-strip");
    let part = b.str("turn-button");
    let label = b.str("Next turn");
    b.template(Template {
        name,
        nodes: nodes(&[
            (NodeKind::Layer(Layer::Hud), 5, plain, None, 0),
            (NodeKind::Box { tag: Tag::Header }, 4, plain, None, 0),
            (
                NodeKind::Box { tag: Tag::Button },
                2,
                hoverable,
                Some(part),
                node_flags::FOCUSABLE,
            ),
            (NodeKind::Text(TextSrc::Static(label)), 1, plain, None, 0),
            (NodeKind::Box { tag: Tag::Div }, 1, plain, None, 0),
        ]),
    });
    let bone = b.str("--bone");
    let ink = b.str("--ink");
    let text = b.str("--ui-text");
    let dark = b.str("dark");
    let light = b.str("light");
    b.tokens(Tokens {
        roots: vec![(bone, [235, 226, 210, 255]), (ink, [255, 200, 80, 255])],
        aliases: vec![(text, ui2_package::TokenId(0))],
        themes: vec![
            Theme {
                name: dark,
                set: vec![],
            },
            Theme {
                name: light,
                set: vec![(ui2_package::TokenId(0), [20, 20, 20, 255])],
            },
        ],
    });
    b
}

#[test]
fn top_strip_round_trips() {
    let bytes = top_strip(&[])
        .finish(manifest_two_themes())
        .expect("finish");
    let pkg = read(&bytes).expect("read");

    let doc = pkg.template(TemplateId(0)).expect("document");
    assert_eq!(pkg.str(doc.name), Some("top-strip"));
    assert_eq!(doc.nodes.len(), 5);
    assert_eq!(doc.nodes[0].kind, NodeKind::Layer(Layer::Hud));
    assert_eq!(doc.nodes[2].flags, node_flags::FOCUSABLE);
    assert_eq!(
        doc.nodes[2].part.and_then(|p| pkg.str(p)),
        Some("turn-button")
    );
    let NodeKind::Text(TextSrc::Static(label)) = doc.nodes[3].kind else {
        panic!("a text leaf");
    };
    assert_eq!(pkg.str(label), Some("Next turn"));

    // Style lookup by viewport cell and condition mask.
    let styles = pkg.styles();
    assert_eq!(styles.cells(), 2);
    let button = pkg.style_set(doc.nodes[2].style).expect("set");
    let at = |viewport: [f32; 2], mask: u8| {
        styles
            .record(button.get(styles.cell(viewport), mask))
            .expect("record")
    };
    let normal = at([1280.0, 720.0], 0);
    assert_eq!(normal.padding_left, Len::px(8.0));
    assert_eq!(normal.font_size, Len::px(14.0));
    assert_eq!(at([1920.0, 1080.0], 0).font_size, Len::px(16.0));
    assert_eq!(at([1280.0, 720.0], 1).cursor, style::Cursor::Pointer);
    assert_eq!(at([1920.0, 1080.0], 1).cursor, style::Cursor::Pointer);
    // Font families went through the string table.
    let names: Vec<_> = normal.font_family.iter().map(|f| pkg.str(*f)).collect();
    assert_eq!(names, [Some("Alegreya"), Some("serif")]);
    assert_eq!(normal.transitions.len(), 1);
    assert_eq!(normal.transitions[0].prop, PropId::Color);

    // Tokens and themes.
    let tokens = pkg.tokens();
    assert_eq!(tokens.resolve(0, normal.color), [235, 226, 210, 255]);
    assert_eq!(tokens.resolve(1, normal.color), [20, 20, 20, 255]);
    let hovered = at([1280.0, 720.0], 1);
    assert_eq!(tokens.resolve(1, hovered.color), [255, 200, 80, 255]);
    let alias = pkg.str_id("--ui-text").and_then(|n| tokens.alias(n));
    assert_eq!(alias, Some(ui2_package::TokenId(0)));
}

#[test]
fn string_insertion_order_does_not_reach_the_bytes() {
    let a = top_strip(&[]).finish(manifest_two_themes()).expect("a");
    let b = top_strip(&["zzz", "--ink", "Next turn", "aaa"])
        .finish(manifest_two_themes())
        .expect("b");
    // Extra strings change the table, so compare packages built from the same strings in
    // another order: intern the same set first, in two orders.
    let words = [
        "Alegreya",
        "serif",
        "top-strip",
        "turn-button",
        "Next turn",
        "--bone",
        "--ink",
        "--ui-text",
        "dark",
        "light",
    ];
    let mut rev = words;
    rev.reverse();
    let c = top_strip(&words).finish(manifest_two_themes()).expect("c");
    let d = top_strip(&rev).finish(manifest_two_themes()).expect("d");
    assert_eq!(c, d);
    assert_eq!(a, c);
    assert_ne!(a, b); // two stray strings are in the table
}

#[test]
fn building_twice_gives_the_same_bytes_and_hash() {
    let a = top_strip(&[]).finish(manifest_two_themes()).expect("a");
    let b = top_strip(&[]).finish(manifest_two_themes()).expect("b");
    assert_eq!(a, b);
    assert_eq!(read(&a).expect("a").hash(), read(&b).expect("b").hash());
}

#[test]
fn a_nan_in_a_record_is_refused() {
    let mut b = PackageBuilder::new();
    let id = b.style_record(rec(|r| r.opacity = f32::NAN));
    b.style_set(StyleSet::plain(vec![id]));
    assert!(matches!(
        b.finish(manifest()),
        Err(PackageError::Invalid { at, .. }) if at == ui2_package::SectionTag::STYL
    ));
    let mut b = PackageBuilder::new();
    let id = b.style_record(rec(|r| r.width = LenAuto::Len(Len::px(f32::INFINITY))));
    b.style_set(StyleSet::plain(vec![id]));
    assert!(b.finish(manifest()).is_err());
}

#[test]
fn style_limits_are_refused() {
    let hover = Cond {
        up: 0,
        kind: CondKind::Hover,
    };
    let mut b = PackageBuilder::new();
    let id = b.style_record(StyleRecord::initial());
    b.style_set(StyleSet {
        conds: vec![hover; 5],
        table: vec![id; 32],
    });
    assert!(b.finish(manifest()).is_err(), "five conditions");

    let mut b = PackageBuilder::new();
    let id = b.style_record(StyleRecord::initial());
    b.style_set(StyleSet {
        conds: vec![hover],
        table: vec![id; 3],
    });
    assert!(b.finish(manifest()).is_err(), "table length");

    let mut b = PackageBuilder::new();
    let id = b.style_record(StyleRecord::initial());
    b.style_set(StyleSet::plain(vec![id, StyleId(9)]));
    b.media(
        vec![Bound {
            px: 1.0,
            upper_inclusive: false,
        }],
        Vec::new(),
    );
    assert!(b.finish(manifest()).is_err(), "record out of range");

    let mut b = PackageBuilder::new();
    let id = b.style_record(StyleRecord::initial());
    b.style_set(StyleSet::plain(vec![id; 17]));
    b.media(
        (1..17)
            .map(|i| Bound {
                px: i as f32,
                upper_inclusive: false,
            })
            .collect(),
        Vec::new(),
    );
    assert!(b.finish(manifest()).is_err(), "17 cells");
}

#[test]
fn template_structure_is_checked() {
    let plain = |b: &mut PackageBuilder| {
        let id = b.style_record(StyleRecord::initial());
        b.style_set(StyleSet::plain(vec![id]))
    };
    let div = |subtree| {
        (
            NodeKind::Box { tag: Tag::Div },
            subtree,
            StyleSetId(0),
            None,
            0,
        )
    };

    // Subtree larger than the list.
    let mut b = PackageBuilder::new();
    plain(&mut b);
    let name = b.str("t");
    b.template(Template {
        name,
        nodes: nodes(&[div(3), div(1)]),
    });
    assert!(b.finish(manifest()).is_err());

    // A child that overruns its parent.
    let mut b = PackageBuilder::new();
    plain(&mut b);
    let name = b.str("t");
    b.template(Template {
        name,
        nodes: nodes(&[div(2), div(2), div(1)]),
    });
    assert!(b.finish(manifest()).is_err());

    // A style set that does not exist.
    let mut b = PackageBuilder::new();
    let name = b.str("t");
    b.template(Template {
        name,
        nodes: nodes(&[div(1)]),
    });
    assert!(b.finish(manifest()).is_err());

    // A `for` body that is not a later template.
    let mut b = PackageBuilder::new();
    plain(&mut b);
    let name = b.str("t");
    let item = b.str("item");
    let looped = (
        NodeKind::For {
            tpl: TemplateId(0),
            item,
            index: None,
            list: ui2_package::ExprId(0),
            key: ui2_package::ExprId(0),
            virt: false,
            sort: None,
            sort_desc: None,
            filter: None,
        },
        1,
        StyleSetId(0),
        None,
        0,
    );
    b.template(Template {
        name,
        nodes: nodes(std::slice::from_ref(&looped)),
    });
    assert!(b.finish(manifest()).is_err(), "a body must come after");

    // The same `for`, with a body template after it, is fine.
    let mut b = PackageBuilder::new();
    plain(&mut b);
    let name = b.str("t");
    let item = b.str("item");
    let mut ok = looped;
    if let NodeKind::For { tpl, item: i, .. } = &mut ok.0 {
        *tpl = TemplateId(1);
        *i = item;
    }
    b.template(Template {
        name,
        nodes: nodes(&[ok]),
    });
    let body_name = b.str("body");
    b.template(Template {
        name: body_name,
        nodes: nodes(&[div(1)]),
    });
    // The loop's expressions must exist in the bindings table.
    b.expr(ui2_package::profile::expr::Expr::Path(vec!["list".into()]));
    assert!(b.finish(manifest()).is_ok());

    // A leaf with children.
    let mut b = PackageBuilder::new();
    plain(&mut b);
    let name = b.str("t");
    let text = b.str("x");
    let leaf = (
        NodeKind::Text(TextSrc::Static(text)),
        2,
        StyleSetId(0),
        None,
        0,
    );
    b.template(Template {
        name,
        nodes: nodes(&[leaf, div(1)]),
    });
    assert!(b.finish(manifest()).is_err());

    // Unknown flag bits.
    let mut b = PackageBuilder::new();
    plain(&mut b);
    let name = b.str("t");
    b.template(Template {
        name,
        nodes: nodes(&[(
            NodeKind::Box { tag: Tag::Div },
            1,
            StyleSetId(0),
            None,
            0x8000,
        )]),
    });
    assert!(b.finish(manifest()).is_err());
}

#[test]
fn token_tables_are_checked() {
    let mut b = PackageBuilder::new();
    let n = b.str("--a");
    let one = b.str("dark");
    b.tokens(Tokens {
        roots: vec![(n, [0; 4])],
        aliases: vec![],
        themes: vec![Theme {
            name: one,
            set: vec![(ui2_package::TokenId(4), [0; 4])],
        }],
    });
    assert!(b.finish(manifest()).is_err(), "override of a missing token");

    // The token table's themes must be the manifest's.
    let mut b = PackageBuilder::new();
    let one = b.str("dark");
    b.tokens(Tokens {
        roots: vec![],
        aliases: vec![],
        themes: vec![Theme {
            name: one,
            set: vec![],
        }],
    });
    assert!(b.finish(manifest_two_themes()).is_err());

    // Without themes of its own, the builder makes one empty theme per manifest theme.
    let bytes = PackageBuilder::new()
        .finish(manifest_two_themes())
        .expect("finish");
    assert_eq!(read(&bytes).expect("read").tokens().themes.len(), 2);
}

#[test]
fn every_property_has_one_record_field() {
    let mut seen = std::collections::BTreeSet::new();
    for (field, props) in StyleRecord::FIELDS {
        assert!(!props.is_empty(), "{field}");
        for p in *props {
            assert!(seen.insert(*p), "{} is held by two fields", p.name());
        }
    }
    assert_eq!(seen.len(), PropId::ALL.len());
    assert!(PropSel::All != PropSel::Prop(PropId::Color));
}

// ---------------------------------------------------------------------------------------------
// From CSS strings to a package

#[cfg(feature = "profile")]
#[test]
fn css_values_round_trip_through_a_package() {
    use ui2_package::profile::parse_value;
    let mut b = PackageBuilder::new();
    let mut record = StyleRecord::initial();
    for (prop, css) in [
        (PropId::Display, "flex"),
        (PropId::BackgroundColor, "rgba(12, 14, 20, 0.85)"),
        (
            PropId::BoxShadow,
            "rgba(0, 0, 0, 0.5) 0px 1px 2px 0px inset",
        ),
        (PropId::BorderTopLeftRadius, "8px 4px"),
        (PropId::FontFamily, "\"Alegreya Sans\", serif"),
        (PropId::FontVariantNumeric, "tabular-nums lining-nums"),
        (
            PropId::Transform,
            "translate(calc(50% - 4px), 0px) rotate(45deg)",
        ),
        (PropId::LineHeight, "1.5"),
        (PropId::ZIndex, "25"),
        (PropId::Width, "50%"),
    ] {
        let value = parse_value(prop, css).expect(css);
        b.set_value(&mut record, prop, &value).expect(css);
    }
    let id = b.style_record(record.clone());
    b.style_set(StyleSet::plain(vec![id]));
    let bytes = b.finish(manifest()).expect("finish");
    let pkg = read(&bytes).expect("read");
    let back = pkg.styles().record(StyleId(0)).expect("record");
    assert_eq!(back.display, style::Display::Flex);
    assert_eq!(back.background_color, ColorV::Lit([12, 14, 20, 217]));
    assert_eq!(back.box_shadow.len(), 1);
    assert!(back.box_shadow[0].inset);
    assert_eq!(back.border_top_left_radius.v, Len::px(4.0));
    assert!(back.font_variant_numeric.tabular && back.font_variant_numeric.lining);
    assert_eq!(back.z_index, style::IntAuto::Int(25));
    assert_eq!(back.width, LenAuto::Len(Len::pct(50.0)));
    let families: Vec<_> = back.font_family.iter().map(|f| pkg.str(*f)).collect();
    assert_eq!(families, [Some("Alegreya Sans"), Some("serif")]);
}

// ---------------------------------------------------------------------------------------------
// FONT, IMGS, SCRP, AREF

fn pseudo_bytes(seed: u32, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|i| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if i % 7 < 4 {
                (x >> 24) as u8
            } else {
                (i / 64) as u8
            }
        })
        .collect()
}

fn compress(raw: &[u8]) -> Vec<u8> {
    zstd::bulk::compress(raw, 19).expect("zstd")
}

fn face(b: &mut PackageBuilder, family: &str, weight: u16, raw: &[u8]) {
    let family = b.str(family);
    let face = Face {
        family,
        weight,
        italic: false,
        role: None,
        ranges: vec![(0x20, 0x7e), (0x400, 0x4ff)],
        blob: BlobRef::unplaced(BlobSection::Font, true),
        blake3: *blake3::hash(raw).as_bytes(),
    };
    b.font(face, &compress(raw), raw.len() as u32);
}

fn page(format: PageFormat, scale: u8, size: [u16; 2], seed: u32) -> (Page, Vec<u8>) {
    let raw = pseudo_bytes(
        seed,
        usize::from(size[0]) * usize::from(size[1]) * format.bytes_per_pixel() as usize,
    );
    let page = Page {
        format,
        scale,
        size,
        blob: BlobRef::unplaced(BlobSection::Imgs, true),
        blake3: *blake3::hash(&raw).as_bytes(),
    };
    (page, raw)
}

fn add_page(b: &mut PackageBuilder, page: &Page, raw: &[u8]) -> u16 {
    b.image_page(page.clone(), &compress(raw), raw.len() as u32)
}

fn rehash(bytes: &mut [u8]) {
    let n = bytes.len() - 32;
    let hash = blake3::hash(&bytes[..n]);
    bytes[n..].copy_from_slice(hash.as_bytes());
}

fn find(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("bytes are in the file")
}

#[test]
fn a_200_kb_font_round_trips_byte_for_byte() {
    let raw = pseudo_bytes(1, 200_000);
    let packed = compress(&raw);
    assert!(packed.len() < raw.len());
    let mut b = PackageBuilder::new();
    face(&mut b, "Alegreya", 400, &raw);
    let bytes = b.finish(manifest()).expect("finish");
    let pkg = read(&bytes).expect("read");

    let face = &pkg.fonts()[0];
    assert_eq!(pkg.str(face.family), Some("Alegreya"));
    assert_eq!(face.blob.raw_len, 200_000);
    assert_eq!(&*pkg.font_data(face).expect("font"), &raw[..]);
    assert_eq!(&*pkg.blob_owned(face.blob).expect("blob"), &raw[..]);
    pkg.verify_blobs().expect("hashes");

    // `blob` is a slice of the file: no copy.
    let stored = pkg.blob(face.blob);
    assert_eq!(stored, &packed[..]);
    let (start, end) = (
        stored.as_ptr() as usize,
        stored.as_ptr() as usize + stored.len(),
    );
    let file = pkg.bytes();
    assert!(start >= file.as_ptr() as usize && end <= file.as_ptr() as usize + file.len());
}

#[test]
fn faces_are_sorted_whatever_the_insertion_order() {
    let (a, b2, c) = (
        pseudo_bytes(2, 3000),
        pseudo_bytes(3, 4000),
        pseudo_bytes(4, 5000),
    );
    let build = |order: [usize; 3]| {
        let mut b = PackageBuilder::new();
        for i in order {
            match i {
                0 => face(&mut b, "Zeta", 400, &a),
                1 => face(&mut b, "Alegreya", 700, &b2),
                _ => face(&mut b, "Alegreya", 400, &c),
            }
        }
        b.finish(manifest()).expect("finish")
    };
    let x = build([0, 1, 2]);
    assert_eq!(x, build([2, 0, 1]));
    let pkg = read(&x).expect("read");
    let order: Vec<_> = pkg
        .fonts()
        .iter()
        .map(|f| (pkg.str(f.family).unwrap_or(""), f.weight))
        .collect();
    assert_eq!(order, [("Alegreya", 400), ("Alegreya", 700), ("Zeta", 400)]);
    for f in pkg.fonts() {
        pkg.font_data(f).expect("font");
    }
}

#[test]
fn a_damaged_zstd_frame_is_corrupt() {
    let raw = pseudo_bytes(5, 20_000);
    let packed = compress(&raw);
    let mut b = PackageBuilder::new();
    face(&mut b, "A", 400, &raw);
    let mut bytes = b.finish(manifest()).expect("finish");
    let at = find(&bytes, &packed[..32]);
    bytes[at..at + 4].copy_from_slice(&[0, 0, 0, 0]); // the frame magic
    rehash(&mut bytes);
    let pkg = read(&bytes).expect("the structure is fine");
    let face = &pkg.fonts()[0];
    let font_tag = ui2_package::SectionTag::FONT;
    assert_eq!(
        pkg.blob_owned(face.blob).err(),
        Some(PackageError::Corrupt { at: Some(font_tag) })
    );
    assert!(pkg.verify_blobs().is_err());

    // A frame that decodes to other bytes fails the hash.
    let mut b = PackageBuilder::new();
    face_with_hash(&mut b, &raw, [7; 32]);
    assert_eq!(
        b.finish(manifest()).err(),
        Some(PackageError::Corrupt { at: Some(font_tag) })
    );
}

fn face_with_hash(b: &mut PackageBuilder, raw: &[u8], blake3: [u8; 32]) {
    let family = b.str("A");
    let face = Face {
        family,
        weight: 400,
        italic: false,
        role: None,
        ranges: vec![],
        blob: BlobRef::unplaced(BlobSection::Font, true),
        blake3,
    };
    b.font(face, &compress(raw), raw.len() as u32);
}

#[test]
fn a_wrong_declared_length_or_checksum_is_refused() {
    let raw = pseudo_bytes(6, 10_000);
    let family = |b: &mut PackageBuilder| b.str("A");
    for raw_len in [9_999u32, 10_001, 0] {
        let mut b = PackageBuilder::new();
        let face = Face {
            family: family(&mut b),
            weight: 400,
            italic: false,
            role: None,
            ranges: vec![],
            blob: BlobRef::unplaced(BlobSection::Font, true),
            blake3: *blake3::hash(&raw).as_bytes(),
        };
        b.font(face, &compress(&raw), raw_len);
        assert!(b.finish(manifest()).is_err(), "raw_len {raw_len}");
    }
}

fn page_and_images(b: &mut PackageBuilder, names: [&str; 2]) -> (Vec<u8>, Vec<u8>) {
    let (rgba, rgba_raw) = page(PageFormat::Rgba8, 1, [64, 32], 10);
    let (mask, mask_raw) = page(PageFormat::A8, 1, [32, 32], 11);
    let (rgba2, rgba2_raw) = page(PageFormat::Rgba8, 2, [128, 64], 12);
    let p0 = add_page(b, &rgba, &rgba_raw);
    let p1 = add_page(b, &mask, &mask_raw);
    let p2 = add_page(b, &rgba2, &rgba2_raw);
    for name in names {
        let path = b.str(name);
        let is_mask = name.starts_with("icon");
        b.image(Img {
            path,
            size: [16.0, 16.0],
            mask: is_mask,
            x1: Sprite {
                page: if is_mask { p1 } else { p0 },
                rect: [1, 1, 16, 16],
            },
            x2: if is_mask {
                None
            } else {
                Some(Sprite {
                    page: p2,
                    rect: [2, 2, 32, 32],
                })
            },
        });
    }
    (rgba_raw, mask_raw)
}

fn node_with_image(b: &mut PackageBuilder, img: ui2_package::ImgId, aref: ui2_package::ARefId) {
    let record = b_record(b);
    let set = b.style_set(StyleSet::plain(vec![record]));
    let name = b.str("doc");
    let leaf = |kind| (kind, 1, set, None, 0);
    b.template(Template {
        name,
        nodes: nodes(&[
            (NodeKind::Box { tag: Tag::Div }, 3, set, None, 0),
            leaf(NodeKind::Image(ImageSrc::Icon { img, mask: true })),
            leaf(NodeKind::Image(ImageSrc::Asset(aref))),
        ]),
    });
}

fn b_record(b: &mut PackageBuilder) -> StyleId {
    b.style_record(StyleRecord::initial())
}

#[test]
fn pictures_pages_and_sprites_round_trip() {
    let mut b = PackageBuilder::new();
    let (rgba_raw, mask_raw) = page_and_images(&mut b, ["icon/turn.svg", "panel.png"]);
    let pkg = read(&b.finish(manifest()).expect("finish")).expect("read");
    let images: &Images = pkg.images();
    assert_eq!(images.pages.len(), 3);
    assert_eq!(
        &*pkg.page_data(&images.pages[0]).expect("page"),
        &rgba_raw[..]
    );
    assert_eq!(
        &*pkg.page_data(&images.pages[1]).expect("page"),
        &mask_raw[..]
    );
    let paths: Vec<_> = images.imgs.iter().map(|i| pkg.str(i.path)).collect();
    assert_eq!(paths, [Some("icon/turn.svg"), Some("panel.png")]);
    assert!(images.imgs[0].mask && images.imgs[0].x2.is_none());
    assert!(!images.imgs[1].mask && images.imgs[1].x2.is_some());
    pkg.verify_blobs().expect("hashes");
}

#[test]
fn picture_ids_follow_the_sorted_table_whatever_the_insertion_order() {
    // Two builds add the pictures in opposite orders; the node refers to the same picture by name.
    let build = |names: [&str; 2], pick: &str| {
        let mut b = PackageBuilder::new();
        page_and_images(&mut b, names);
        let which = names.iter().position(|n| *n == pick).expect("picked") as u32;
        let aref = b.asset_ref("portrait:chief");
        node_with_image(&mut b, ui2_package::ImgId(which), aref);
        b.finish(manifest()).expect("finish")
    };
    let x = build(["icon/turn.svg", "icon/end.svg"], "icon/turn.svg");
    let y = build(["icon/end.svg", "icon/turn.svg"], "icon/turn.svg");
    assert_eq!(x, y);
    let pkg = read(&x).expect("read");
    let doc = &pkg.templates()[0];
    let NodeKind::Image(ImageSrc::Icon { img, .. }) = doc.nodes[1].kind else {
        panic!("an icon");
    };
    let path = pkg.images().imgs[img.index()].path;
    assert_eq!(pkg.str(path), Some("icon/turn.svg"));
    let NodeKind::Image(ImageSrc::Asset(a)) = doc.nodes[2].kind else {
        panic!("an asset image");
    };
    assert_eq!(pkg.asset_ref(a), Some("portrait:chief"));
}

#[test]
fn asset_references_are_sorted_and_unique() {
    let mut b = PackageBuilder::new();
    let z = b.asset_ref("ui-tx:z");
    let a = b.asset_ref("portrait:a");
    assert_eq!(b.asset_ref("ui-tx:z"), z);
    assert_ne!(z, a);
    let pkg = read(&b.finish(manifest()).expect("finish")).expect("read");
    assert_eq!(
        pkg.asset_refs().collect::<Vec<_>>(),
        ["portrait:a", "ui-tx:z"]
    );
}

#[test]
fn bad_pictures_are_refused() {
    let one = |f: &dyn Fn(&mut Img, &mut Page)| {
        let mut b = PackageBuilder::new();
        let (mut p, raw) = page(PageFormat::Rgba8, 1, [64, 32], 10);
        let mut img = Img {
            path: b.str("a.png"),
            size: [16.0, 16.0],
            mask: false,
            x1: Sprite {
                page: 0,
                rect: [1, 1, 16, 16],
            },
            x2: None,
        };
        f(&mut img, &mut p);
        add_page(&mut b, &p, &raw);
        b.image(img);
        b.finish(manifest()).is_ok()
    };
    assert!(one(&|_, _| {}), "the baseline is fine");
    assert!(
        !one(&|i, _| i.x1.rect = [60, 1, 16, 16]),
        "sprite outside the page"
    );
    assert!(!one(&|i, _| i.x1.page = 3), "page out of range");
    assert!(!one(&|i, _| i.mask = true), "mask on an RGBA page");
    assert!(!one(&|i, _| i.size = [f32::NAN, 1.0]), "NaN size");
    assert!(
        !one(&|i, _| i.x2 = Some(Sprite {
            page: 0,
            rect: [1, 1, 4, 4]
        })),
        "2x on a 1x page"
    );
    assert!(!one(&|_, p| p.size = [2048, 1]), "page larger than 1024");
    assert!(!one(&|_, p| p.scale = 3), "scale 3");
}

#[test]
fn image_nodes_must_point_at_existing_pictures_and_references() {
    let mut b = PackageBuilder::new();
    node_with_image(&mut b, ui2_package::ImgId(0), ui2_package::ARefId(0));
    assert!(b.finish(manifest()).is_err());
}

#[test]
fn scripts_keep_their_order_and_bytes() {
    let mut b = PackageBuilder::new();
    b.script("z.js", b"var z = 1;\n");
    b.script("a.js", b"var a = 2;\n");
    b.script_reads(&["top", "game", "top"]);
    let bytes = b.finish(manifest()).expect("finish");
    let pkg = read(&bytes).expect("read");
    let scripts: Vec<_> = pkg.scripts().collect();
    assert_eq!(
        scripts,
        [
            ("z.js", &b"var z = 1;\n"[..]),
            ("a.js", &b"var a = 2;\n"[..])
        ]
    );
    assert_eq!(pkg.script_reads().collect::<Vec<_>>(), ["game", "top"]);
    let (_, text) = scripts[0];
    let (start, file) = (text.as_ptr() as usize, pkg.bytes());
    assert!(
        start >= file.as_ptr() as usize
            && start + text.len() <= file.as_ptr() as usize + file.len()
    );
}

#[test]
fn a_package_with_every_section_is_deterministic() {
    let build = || {
        let mut b = top_strip(&[]);
        face_for(&mut b);
        page_and_images(&mut b, ["a.png", "icon/b.svg"]);
        b.script("s.js", b"1");
        b.script_reads(&["top"]);
        b.asset_ref("portrait:x");
        b.finish(manifest_two_themes()).expect("finish")
    };
    let first = build();
    assert_eq!(first, build());
    let pkg = read(&first).expect("read");
    pkg.verify_blobs().expect("hashes");
    assert_eq!(pkg.hash(), read(&build()).expect("again").hash());
}

fn face_for(b: &mut PackageBuilder) {
    face(b, "Alegreya", 400, &pseudo_bytes(9, 8000));
}

// ---------------------------------------------------------------------------------------------
// BIND, L10N, EXTN, SRCM

mod common;

use common::{MapView, parse_type};
use ui2_package::profile::action::{Action, AnchorKind, Arg, ControlKind, Event};
use ui2_package::profile::expr::{self, Expr};
use ui2_package::profile::{SchemaView, TypeSig};
use ui2_package::{
    BindKind, Binding, ExprId, Lit, NodeRef, Scalar, Seg, SrcLoc, TplId, hotkey_mods,
};

fn e(src: &str) -> Expr {
    expr::parse(src).unwrap_or_else(|err| panic!("{src}: {err}"))
}

fn manifest_langs() -> Manifest {
    let mut m = manifest();
    m.languages = vec!["en_US".into(), "ru_RU".into()];
    m
}

fn ordered<T>(mut v: Vec<T>, reverse: bool) -> Vec<T> {
    if reverse {
        v.reverse();
    }
    v
}

fn node(tpl: u32, node: u32) -> NodeRef {
    NodeRef(TemplateId(tpl), node)
}

/// A skin with a loop, a button with every kind of binding, a tooltip, and the sorted tables of
/// the new sections filled in. `reverse` adds everything whose order must not matter backwards.
fn bound_package(reverse: bool) -> PackageBuilder {
    let mut b = PackageBuilder::new();
    let plain = {
        let id = b.style_record(StyleRecord::initial());
        b.style_set(StyleSet::plain(vec![id]))
    };
    let turn = b.expr(e("game.turn"));
    let big = b.expr(e("game.turn > 3"));
    let list = b.expr(e("game.cities"));
    let key = b.expr(e("c.id"));
    assert_eq!(
        b.expr(e("game.turn")),
        turn,
        "equal trees are one expression"
    );
    assert_eq!(
        (turn, big, list, key),
        (ExprId(0), ExprId(1), ExprId(2), ExprId(3))
    );
    let lit = b.str("Turn ");
    let text = b.tpl(vec![Seg::Lit(lit), Seg::Expr(turn)]);
    assert_eq!(b.tpl(vec![Seg::Lit(lit), Seg::Expr(turn)]), text);
    assert_eq!(text, TplId(0));

    let (doc, row, tip) = (b.str("doc"), b.str("row"), b.str("tip"));
    let item = b.str("c");
    let static_text = b.str("-");
    let looped = NodeKind::For {
        tpl: TemplateId(1),
        item,
        index: None,
        list,
        key,
        virt: false,
        sort: None,
        sort_desc: None,
        filter: None,
    };
    b.template(Template {
        name: doc,
        nodes: nodes(&[
            (NodeKind::Layer(Layer::Hud), 5, plain, None, 0),
            (NodeKind::Box { tag: Tag::Div }, 2, plain, None, 0),
            (looped, 1, plain, None, 0),
            (
                NodeKind::Box { tag: Tag::Button },
                1,
                plain,
                None,
                node_flags::FOCUSABLE,
            ),
            (NodeKind::Text(TextSrc::Bound(text)), 1, plain, None, 0),
        ]),
    });
    b.template(Template {
        name: row,
        nodes: nodes(&[
            (NodeKind::Box { tag: Tag::Div }, 2, plain, None, 0),
            (
                NodeKind::Text(TextSrc::Static(static_text)),
                1,
                plain,
                None,
                0,
            ),
        ]),
    });
    b.template(Template {
        name: tip,
        nodes: nodes(&[(NodeKind::Box { tag: Tag::Div }, 1, plain, None, 0)]),
    });

    // Bindings are added out of node order on purpose.
    let late = b.str("is-late");
    let open = b.str("prod.open");
    let t0 = TemplateId(0);
    b.binding(Binding {
        tpl: t0,
        node: 4,
        kind: BindKind::Text(text),
    });
    b.binding(Binding {
        tpl: t0,
        node: 4,
        kind: BindKind::Fit { min: Some(10.0) },
    });
    b.binding(Binding {
        tpl: t0,
        node: 3,
        kind: BindKind::ClassToggle {
            class: late,
            cond: big,
        },
    });
    b.binding(Binding {
        tpl: t0,
        node: 3,
        kind: BindKind::Event {
            ev: Event::Click,
            actions: vec![
                Action::Toggle {
                    path: "state.prod.open".into(),
                },
                Action::Call {
                    name: "ping".into(),
                    args: vec![Arg {
                        name: None,
                        value: e("game.turn"),
                    }],
                },
                Action::Stop,
            ],
        },
    });
    b.binding(Binding {
        tpl: t0,
        node: 3,
        kind: BindKind::Model(open),
    });
    b.binding(Binding {
        tpl: t0,
        node: 3,
        kind: BindKind::Control {
            kind: ControlKind::Checkbox,
            value: None,
            range: None,
            options: None,
        },
    });
    b.binding(Binding {
        tpl: t0,
        node: 2,
        kind: BindKind::Anchor {
            kind: AnchorKind::City,
            id: key,
        },
    });

    let paths = vec![
        ("game.turn", TypeSig::Int),
        ("game.cities", parse_type("list<struct game.cities[]>")),
    ];
    for (p, t) in ordered(paths, reverse) {
        b.used_path(p, t);
    }
    b.intent(
        "unit.action",
        vec![
            TypeSig::Key("unit".into()),
            parse_type("enum(fortify|sleep)"),
        ],
    );
    let states = vec![
        (
            "tabs",
            Lit::List(vec![Scalar::Str("a".into()), Scalar::Str("b".into())]),
        ),
        ("prod.open", Lit::Bool(false)),
        ("volume", Lit::Float(0.5)),
    ];
    for (p, l) in ordered(states, reverse) {
        b.state_default(p, l);
    }
    for h in ordered(vec!["ping", "alpha"], reverse) {
        b.handler(h);
    }
    let texts = vec![
        ("en_US", "top.turn", "Turn {0}"),
        ("ru_RU", "top.turn", "\u{425}\u{43e}\u{434} {0}"),
        ("en_US", "top.next", "Next"),
    ];
    for (lang, k, v) in ordered(texts, reverse) {
        b.l10n(lang, k, v);
    }
    let slots = vec![("mods", node(0, 1)), ("alpha.beta", node(0, 3))];
    for (n, at) in ordered(slots, reverse) {
        b.slot(n, at);
    }
    let parts = vec![("list", node(0, 1)), ("turn-button", node(0, 3))];
    for (n, at) in ordered(parts, reverse) {
        b.part(n, at);
    }
    b.screen("production", node(0, 0));
    b.island("clock", node(0, 4), &[("minutes", 4), ("hours", 4)]);
    let keys = vec![
        ("KeyB", hotkey_mods::CTRL | hotkey_mods::SHIFT),
        ("KeyB", 0),
        ("F5", 0),
    ];
    for (code, mods) in ordered(keys, reverse) {
        b.hotkey(code, mods, node(0, 3));
    }
    b.tooltip(node(0, 3), TemplateId(2));
    let at = |file, line| SrcLoc { file, line, col: 3 };
    let places = vec![
        vec![at(1, 1), at(1, 2), at(1, 3), at(0, 4), at(0, 5)],
        vec![at(1, 7), at(1, 8)],
        vec![at(0, 9)],
    ];
    if reverse {
        // The same files named in the other order, with the indices to match.
        let swapped = places
            .into_iter()
            .map(|t| {
                t.into_iter()
                    .map(|l| SrcLoc {
                        file: 1 - l.file,
                        ..l
                    })
                    .collect()
            })
            .collect();
        b.source_map(&["skin.html", "parts/top.html"], swapped);
    } else {
        b.source_map(&["parts/top.html", "skin.html"], places);
    }
    b
}

#[test]
fn bind_sections_round_trip() {
    let bytes = bound_package(false)
        .finish(manifest_langs())
        .expect("finish");
    let pkg = read(&bytes).expect("read");

    // Expressions and templates.
    assert_eq!(pkg.expr(ExprId(1)), Some(&e("game.turn > 3")));
    assert_eq!(pkg.expr(ExprId(9)), None);
    let segs = pkg.tpl(TplId(0)).expect("template");
    assert!(matches!(segs, [Seg::Lit(l), Seg::Expr(ExprId(0))] if pkg.str(*l) == Some("Turn ")));

    // Bindings of a node keep their order; the nodes are in order.
    let button = pkg.bindings_of(TemplateId(0), 3);
    assert_eq!(button.len(), 4);
    assert!(matches!(button[0].kind, BindKind::ClassToggle { .. }));
    assert!(matches!(&button[1].kind, BindKind::Event { actions, .. } if actions.len() == 3));
    assert!(matches!(button[2].kind, BindKind::Model(_)));
    assert!(matches!(button[3].kind, BindKind::Control { .. }));
    assert_eq!(pkg.bindings_of(TemplateId(0), 4).len(), 2);
    assert_eq!(pkg.bindings_of(TemplateId(0), 2).len(), 1);
    assert!(pkg.bindings_of(TemplateId(1), 0).is_empty());
    let BindKind::ClassToggle { class, .. } = button[0].kind else {
        panic!("class toggle");
    };
    assert_eq!(pkg.str(class), Some("is-late"));
    let BindKind::Model(model) = button[2].kind else {
        panic!("model");
    };
    assert_eq!(pkg.str(model), Some("prod.open"));

    // The tables.
    let paths: Vec<_> = pkg.used_paths().map(|(p, _)| p).collect();
    assert_eq!(paths, ["game.cities", "game.turn"]);
    let state: Vec<_> = pkg.state_defaults().map(|(p, _)| p).collect();
    assert_eq!(state, ["prod.open", "tabs", "volume"]);
    assert_eq!(
        pkg.state_defaults()
            .find(|(p, _)| *p == "tabs")
            .and_then(|(_, l)| l.type_sig()),
        Some(parse_type("list<str>"))
    );
    assert_eq!(pkg.handlers().collect::<Vec<_>>(), ["alpha", "ping"]);
    assert_eq!(pkg.intents().count(), 1);

    // Texts.
    assert_eq!(pkg.languages(), ["en_US", "ru_RU"]);
    assert_eq!(pkg.l10n("en_US", "top.turn"), Some("Turn {0}"));
    assert_eq!(
        pkg.l10n("ru_RU", "top.turn"),
        Some("\u{425}\u{43e}\u{434} {0}")
    );
    assert_eq!(pkg.l10n("ru_RU", "top.next"), None);
    assert_eq!(pkg.l10n("de_DE", "top.next"), None);
    assert_eq!(pkg.l10n("en_US", "no.such.key"), None);

    // Extension points.
    assert_eq!(
        pkg.slots().collect::<Vec<_>>(),
        [("alpha.beta", node(0, 3)), ("mods", node(0, 1))]
    );
    assert_eq!(pkg.parts().count(), 2);
    assert_eq!(
        pkg.screens().collect::<Vec<_>>(),
        [("production", node(0, 0))]
    );
    let island = &pkg.islands()[0];
    assert_eq!(pkg.str(island.id), Some("clock"));
    let keys: Vec<_> = island.keys.iter().map(|(k, n)| (pkg.str(*k), *n)).collect();
    assert_eq!(keys, [(Some("hours"), 4), (Some("minutes"), 4)]);
    let hotkeys: Vec<_> = pkg
        .hotkeys()
        .iter()
        .map(|h| (pkg.str(h.code), h.mods))
        .collect();
    assert_eq!(
        hotkeys,
        [(Some("F5"), 0), (Some("KeyB"), 0), (Some("KeyB"), 5)]
    );
    assert_eq!(pkg.tooltips(), [(node(0, 3), TemplateId(2))]);

    // Source places, whichever order the files were named in.
    assert_eq!(pkg.source_loc(TemplateId(0), 0), Some(("skin.html", 1, 3)));
    assert_eq!(
        pkg.source_loc(TemplateId(0), 3),
        Some(("parts/top.html", 4, 3))
    );
    assert_eq!(
        pkg.source_loc(TemplateId(2), 0),
        Some(("parts/top.html", 9, 3))
    );
    assert_eq!(pkg.source_loc(TemplateId(0), 9), None);
}

#[test]
fn a_package_without_the_optional_sections_is_valid_and_empty() {
    let bytes = PackageBuilder::new().finish(manifest()).expect("finish");
    let pkg = read(&bytes).expect("read");
    assert!(pkg.bindings().items.is_empty());
    assert_eq!(pkg.slots().count(), 0);
    assert_eq!(pkg.source_loc(TemplateId(0), 0), None);
    assert_eq!(pkg.l10n("en_US", "x"), None);
    assert_eq!(pkg.languages(), ["en_US"]);
}

#[test]
fn bind_sections_are_deterministic_whatever_the_insertion_order() {
    let a = bound_package(false).finish(manifest_langs()).expect("a");
    assert_eq!(
        a,
        bound_package(false)
            .finish(manifest_langs())
            .expect("again")
    );
    let b = bound_package(true).finish(manifest_langs()).expect("b");
    assert_eq!(a, b);
    assert_eq!(read(&a).expect("a").hash(), read(&b).expect("b").hash());
    // And a package with every section at once.
    let build = |reverse| {
        let mut b = bound_package(reverse);
        face_for(&mut b);
        page_and_images(&mut b, ["a.png", "icon/b.svg"]);
        b.script("s.js", b"1");
        b.asset_ref("portrait:x");
        b.finish(manifest_langs()).expect("finish")
    };
    assert_eq!(build(false), build(true));
}

fn schema_for_paths() -> MapView {
    MapView::default()
        .ty("game.turn", "int")
        .ty("game.cities", "list<struct game.cities[]>")
        .ty("game.cities[].id", "key<city>")
        .intent(
            "unit.action",
            &[("unit", "key<unit>"), ("action", "enum(fortify|sleep)")],
        )
}

#[test]
fn used_paths_and_intents_are_checked_against_the_schema() {
    let bytes = bound_package(false)
        .finish(manifest_langs())
        .expect("finish");
    let pkg = read(&bytes).expect("read");
    pkg.check_paths(&schema_for_paths()).expect("same schema");

    // A schema with more is fine.
    let richer = schema_for_paths().ty("game.extra", "int");
    pkg.check_paths(&richer).expect("a new field is compatible");

    let incompatible = |view: &dyn SchemaView| {
        matches!(
            pkg.check_paths(view),
            Err(PackageError::Incompatible { .. })
        )
    };
    // A path that changed type.
    assert!(incompatible(&schema_for_paths().ty("game.turn", "float")));
    // A path that is gone.
    let mut gone = schema_for_paths();
    gone.types.remove("game.turn");
    assert!(incompatible(&gone));
    // A list whose element is another record.
    assert!(incompatible(
        &schema_for_paths().ty("game.cities", "list<struct other>")
    ));
    // An intent that changed its arguments, or is gone.
    assert!(incompatible(
        &schema_for_paths().intent("unit.action", &[("unit", "key<unit>")])
    ));
    let mut no_intent = schema_for_paths();
    no_intent.intents.clear();
    assert!(incompatible(&no_intent));
}

#[test]
fn bad_data_for_the_new_sections_is_refused() {
    let t0 = TemplateId(0);
    let refused = |what: &str, change: &dyn Fn(&mut PackageBuilder)| {
        let mut b = bound_package(false);
        change(&mut b);
        assert!(b.finish(manifest_langs()).is_err(), "{what}");
    };
    let bind = |tpl: TemplateId, node, kind| Binding { tpl, node, kind };
    refused("a binding on a missing node", &|b| {
        b.binding(bind(t0, 99, BindKind::HotkeyLabel));
    });
    refused("a binding in a missing template", &|b| {
        b.binding(bind(TemplateId(9), 0, BindKind::HotkeyLabel));
    });
    refused("a template with a missing expression", &|b| {
        let t = b.tpl(vec![Seg::Expr(ExprId(99))]);
        b.binding(bind(t0, 0, BindKind::Text(t)));
    });
    refused("a binding with a missing template", &|b| {
        b.binding(bind(t0, 0, BindKind::Text(TplId(99))));
    });
    refused("a condition that is not an expression", &|b| {
        let class = b.str("x");
        b.binding(bind(
            t0,
            0,
            BindKind::ClassToggle {
                class,
                cond: ExprId(99),
            },
        ));
    });
    refused("an event without actions", &|b| {
        b.binding(bind(
            t0,
            0,
            BindKind::Event {
                ev: Event::Click,
                actions: vec![],
            },
        ));
    });
    refused("an action with a NaN", &|b| {
        b.binding(bind(
            t0,
            0,
            BindKind::Event {
                ev: Event::Click,
                actions: vec![Action::Set {
                    path: "state.x".into(),
                    value: Expr::Float(f64::NAN),
                }],
            },
        ));
    });
    refused("a NaN in an expression", &|b| {
        b.expr(Expr::Float(f64::INFINITY));
    });
    refused("an expression nested too deeply", &|b| {
        let mut deep = Expr::Int(1);
        for _ in 0..200 {
            deep = Expr::Unary(expr::UnOp::Not, Box::new(deep));
        }
        b.expr(deep);
    });
    refused("a call with the wrong number of arguments", &|b| {
        b.expr(Expr::Call(expr::Func::Clamp, vec![Expr::Int(1)]));
    });
    refused("a slider with an empty range", &|b| {
        b.binding(bind(
            t0,
            3,
            BindKind::Control {
                kind: ControlKind::Slider,
                value: None,
                range: Some([1.0, 1.0, 1.0]),
                options: None,
            },
        ));
    });
    refused("a fit that is not finite", &|b| {
        b.binding(bind(
            t0,
            4,
            BindKind::Fit {
                min: Some(f32::NAN),
            },
        ));
    });
    refused("one path with two types", &|b| {
        b.used_path("game.turn", TypeSig::Bool)
    });
    refused("one intent with two signatures", &|b| {
        b.intent("unit.action", vec![])
    });
    refused("two defaults of one state path", &|b| {
        b.state_default("volume", Lit::Float(0.7));
    });
    refused("an empty state list", &|b| {
        b.state_default("e", Lit::List(vec![]))
    });
    refused("a state list of mixed scalars", &|b| {
        b.state_default(
            "m",
            Lit::List(vec![Scalar::Int(1), Scalar::Str("x".into())]),
        );
    });
    refused("a state float that is not finite", &|b| {
        b.state_default("n", Lit::Float(f64::NAN));
    });
    refused("a text for a language the manifest lacks", &|b| {
        b.l10n("de_DE", "k", "v")
    });
    refused("one text twice", &|b| b.l10n("en_US", "top.next", "Other"));
    refused("a slot twice", &|b| b.slot("mods", node(0, 0)));
    refused("a slot on a missing node", &|b| b.slot("x", node(0, 99)));
    refused("a part in a missing template", &|b| b.part("x", node(9, 0)));
    refused("a screen on a missing node", &|b| b.screen("x", node(1, 5)));
    refused("an island twice", &|b| b.island("clock", node(0, 4), &[]));
    refused("an island key on a missing node", &|b| {
        b.island("other", node(0, 4), &[("k", 99)]);
    });
    refused("a hotkey with an unknown modifier", &|b| {
        b.hotkey("KeyZ", 8, node(0, 3))
    });
    refused("a hotkey twice", &|b| b.hotkey("F5", 0, node(0, 3)));
    refused("a tooltip with a missing template", &|b| {
        b.tooltip(node(0, 4), TemplateId(9))
    });
    refused("a tooltip twice", &|b| b.tooltip(node(0, 3), TemplateId(1)));
    let place = SrcLoc {
        file: 0,
        line: 1,
        col: 1,
    };
    refused("a source map that misses nodes", &|b| {
        b.source_map(&["a.html"], vec![vec![place]]);
    });
    refused("a source place in a missing file", &|b| {
        let at = SrcLoc { file: 5, ..place };
        b.source_map(&["a.html"], vec![vec![at; 5], vec![at; 2], vec![at]]);
    });
    for path in [
        "/abs/skin.html",
        "C:/skin.html",
        "a\\b.html",
        "../skin.html",
        "",
    ] {
        refused("a source path of the build machine", &|b| {
            b.source_map(&[path], vec![vec![place; 5], vec![place; 2], vec![place]]);
        });
    }
}

fn section_range(bytes: &[u8], tag: &[u8; 4]) -> Option<std::ops::Range<usize>> {
    let count = u32::from_le_bytes(bytes[12..16].try_into().expect("count")) as usize;
    (0..count).find_map(|i| {
        let at = 16 + 16 * i;
        if &bytes[at..at + 4] != tag {
            return None;
        }
        let word = |o: usize| {
            u32::from_le_bytes(bytes[at + o..at + o + 4].try_into().expect("word")) as usize
        };
        Some(word(8)..word(8) + word(12))
    })
}

#[test]
fn damaged_new_sections_are_refused_or_read_but_never_panic() {
    let bytes = bound_package(false)
        .finish(manifest_langs())
        .expect("finish");
    let mut refused = 0;
    let mut total = 0;
    for tag in [b"BIND", b"L10N", b"EXTN", b"SRCM"] {
        let range = section_range(&bytes, tag).expect("section present");
        assert!(!range.is_empty());
        for i in range {
            for bit in [0u8, 2, 5, 7] {
                let mut damaged = bytes.clone();
                damaged[i] ^= 1 << bit;
                rehash(&mut damaged);
                total += 1;
                if read(&damaged).is_err() {
                    refused += 1;
                }
            }
        }
    }
    // Most flips break an index, a length or a variant; the rest are other valid values.
    assert!(refused * 3 > total, "{refused} of {total}");
    for n in 0..bytes.len() {
        assert!(read(&bytes[..n]).is_err(), "prefix {n}");
    }
}

#[test]
fn nesting_that_would_overflow_the_stack_is_refused_when_decoding() {
    use ui2_package::Bindings;
    let mut deep = Expr::Int(1);
    for _ in 0..expr::MAX_DEPTH + 20 {
        deep = Expr::Unary(expr::UnOp::Neg, Box::new(deep));
    }
    let section = Bindings {
        exprs: vec![deep],
        ..Bindings::default()
    };
    let bytes = postcard::to_allocvec(&section).expect("encode");
    assert!(postcard::from_bytes::<Bindings>(&bytes).is_err());

    let mut ty = TypeSig::Int;
    for _ in 0..100 {
        ty = TypeSig::List(Box::new(ty));
    }
    let section = Bindings {
        used_paths: vec![(StrId(0), ty)],
        ..Bindings::default()
    };
    let bytes = postcard::to_allocvec(&section).expect("encode");
    assert!(postcard::from_bytes::<Bindings>(&bytes).is_err());
    // A type of ordinary depth decodes.
    let ok = Bindings {
        used_paths: vec![(StrId(0), parse_type("list<struct a.b[]>?"))],
        ..Bindings::default()
    };
    let bytes = postcard::to_allocvec(&ok).expect("encode");
    assert_eq!(postcard::from_bytes::<Bindings>(&bytes), Ok(ok));
}
