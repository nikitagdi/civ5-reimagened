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
