//! A small package with every section, made by the crate's own writer from generated data: the
//! base of the corrupt-input, limit and determinism tests, and the source of
//! `tests/fixtures/mini.ui2`. Nothing in it is compressed with zstd, so its bytes do not depend on
//! the compressor's version.

use ui2_package::profile::TypeSig;
use ui2_package::profile::action::{Action, Event};
use ui2_package::profile::expr::{self, Expr};
use ui2_package::{
    BindKind, Binding, BlobRef, BlobSection, Cond, CondKind, Face, FaceFamily, FontRoles, ImageSrc,
    Img, Kind, Layer, Lit, Manifest, Node, NodeKind, NodeRef, PackageBuilder, Page, PageFormat,
    Scalar, Seg, Sprite, SrcLoc, StyleRecord, StyleSet, StyleSetId, Tag, Template, TemplateId,
    TextSrc, Theme, Tokens, Tool, hotkey_mods, node_flags,
};

pub fn manifest() -> Manifest {
    Manifest {
        kind: Kind::Skin,
        id: "mini".into(),
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
        themes: vec!["dark".into(), "light".into()],
        languages: vec!["en_US".into(), "ru_RU".into()],
        font_roles: FontRoles {
            sans: FaceFamily("Sans".into()),
            display: FaceFamily("Display".into()),
        },
        schema_hash: 7,
    }
}

fn e(src: &str) -> Expr {
    expr::parse(src).unwrap_or_else(|err| panic!("{src}: {err}"))
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

fn nodes(spec: Vec<(NodeKind, u32, StyleSetId, u16)>) -> Vec<Node> {
    spec.into_iter()
        .map(|(kind, subtree, style, flags)| Node {
            kind,
            subtree,
            style,
            part: None,
            flags,
        })
        .collect()
}

/// A little pseudo-random byte string; the same for the same seed.
pub fn bytes(seed: u32, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect()
}

/// The builder of the mini package. With `reverse`, everything whose order must not matter is
/// added in the opposite order; the bytes are the same.
pub fn builder(reverse: bool) -> PackageBuilder {
    let mut b = PackageBuilder::new();
    let base = b.style_record(StyleRecord::initial());
    let hover = {
        let mut r = StyleRecord::initial();
        r.cursor = ui2_package::style::Cursor::Pointer;
        b.style_record(r)
    };
    let plain = b.style_set(StyleSet::plain(vec![base]));
    let hoverable = b.style_set(StyleSet {
        conds: vec![Cond {
            up: 0,
            kind: CondKind::Hover,
        }],
        table: vec![base, hover],
    });

    let turn = b.expr(e("game.turn"));
    let big = b.expr(e("game.turn > 3"));
    let list = b.expr(e("game.cities"));
    let key = b.expr(e("c.id"));
    let lit = b.str("Turn ");
    let text = b.tpl(vec![Seg::Lit(lit), Seg::Expr(turn)]);

    let (doc, row, tip) = (b.str("doc"), b.str("row"), b.str("tip"));
    let item = b.str("c");
    let dash = b.str("-");
    let icon = add_icon(&mut b);
    let aref = b.asset_ref("portrait:chief");
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
        nodes: nodes(vec![
            (NodeKind::Layer(Layer::Hud), 5, plain, 0),
            (NodeKind::Box { tag: Tag::Div }, 2, plain, 0),
            (looped, 1, plain, 0),
            (
                NodeKind::Box { tag: Tag::Button },
                1,
                hoverable,
                node_flags::FOCUSABLE,
            ),
            (NodeKind::Text(TextSrc::Bound(text)), 1, plain, 0),
        ]),
    });
    b.template(Template {
        name: row,
        nodes: nodes(vec![
            (NodeKind::Box { tag: Tag::Div }, 4, plain, 0),
            (NodeKind::Text(TextSrc::Static(dash)), 1, plain, 0),
            (
                NodeKind::Image(ImageSrc::Icon {
                    img: icon,
                    mask: true,
                }),
                1,
                plain,
                0,
            ),
            (NodeKind::Image(ImageSrc::Asset(aref)), 1, plain, 0),
        ]),
    });
    b.template(Template {
        name: tip,
        nodes: nodes(vec![(NodeKind::Box { tag: Tag::Div }, 1, plain, 0)]),
    });

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
                Action::Stop,
            ],
        },
    });
    b.binding(Binding {
        tpl: t0,
        node: 3,
        kind: BindKind::Model(open),
    });
    for (p, t) in ordered(
        vec![
            ("game.turn", TypeSig::Int),
            ("game.cities", TypeSig::List(Box::new(TypeSig::Int))),
        ],
        reverse,
    ) {
        b.used_path(p, t);
    }
    b.intent("unit.action", vec![TypeSig::Key("unit".into())]);
    for (p, l) in ordered(
        vec![
            ("tabs", Lit::List(vec![Scalar::Str("a".into())])),
            ("prod.open", Lit::Bool(false)),
        ],
        reverse,
    ) {
        b.state_default(p, l);
    }
    for h in ordered(vec!["ping", "alpha"], reverse) {
        b.handler(h);
    }
    for (lang, k, v) in ordered(
        vec![
            ("en_US", "top.turn", "Turn {0}"),
            ("ru_RU", "top.turn", "\u{425}\u{43e}\u{434} {0}"),
            ("en_US", "top.next", "Next"),
        ],
        reverse,
    ) {
        b.l10n(lang, k, v);
    }
    for (n, at) in ordered(vec![("mods", node(0, 1)), ("alpha", node(0, 3))], reverse) {
        b.slot(n, at);
    }
    b.part("turn-button", node(0, 3));
    b.screen("production", node(0, 0));
    b.island("clock", node(0, 4), &[("minutes", 4), ("hours", 4)]);
    for (code, mods) in ordered(vec![("KeyB", hotkey_mods::CTRL), ("F5", 0)], reverse) {
        b.hotkey(code, mods, node(0, 3));
    }
    b.tooltip(node(0, 3), TemplateId(2));
    let at = |file, line| SrcLoc { file, line, col: 3 };
    let places = vec![
        vec![at(1, 1), at(1, 2), at(1, 3), at(0, 4), at(0, 5)],
        vec![at(1, 7), at(1, 8), at(0, 9), at(0, 10)],
        vec![at(0, 11)],
    ];
    if reverse {
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

    let (bone, ink, ui_text) = (b.str("--bone"), b.str("--ink"), b.str("--ui-text"));
    let (dark, light) = (b.str("dark"), b.str("light"));
    b.tokens(Tokens {
        roots: vec![(bone, [235, 226, 210, 255]), (ink, [255, 200, 80, 255])],
        aliases: vec![(ui_text, ui2_package::TokenId(0))],
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

    // A font and a picture page, stored as they are.
    let font = bytes(1, 96);
    let family = b.str("Sans");
    b.font(
        Face {
            family,
            weight: 400,
            italic: false,
            role: None,
            ranges: vec![(0x20, 0x7e)],
            blob: BlobRef::unplaced(BlobSection::Font, false),
            blake3: *blake3::hash(&font).as_bytes(),
        },
        &font,
        font.len() as u32,
    );
    b.script("main.js", b"var turn = 1;\n");
    b.script_reads(&["top", "game"]);
    b
}

/// The mini package.
pub fn build() -> Vec<u8> {
    builder(false).finish(manifest()).expect("the mini package")
}

/// The same with the insertion order of the order-free parts reversed.
pub fn build_reversed() -> Vec<u8> {
    builder(true).finish(manifest()).expect("the mini package")
}

/// One 4x4 mask picture on its own A8 page.
fn add_icon(b: &mut PackageBuilder) -> ui2_package::ImgId {
    let raw = bytes(2, 16);
    let page = Page {
        format: PageFormat::A8,
        scale: 1,
        size: [4, 4],
        blob: BlobRef::unplaced(BlobSection::Imgs, false),
        blake3: *blake3::hash(&raw).as_bytes(),
    };
    let index = b.image_page(page, &raw, 16);
    let path = b.str("icon/dot.svg");
    b.image(Img {
        path,
        size: [4.0, 4.0],
        mask: true,
        x1: Sprite {
            page: index,
            rect: [0, 0, 4, 4],
        },
        x2: None,
    })
}
