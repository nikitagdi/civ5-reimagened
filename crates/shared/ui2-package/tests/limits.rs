//! The limits and the references of the format, one test per rule: a file that breaks the rule is
//! refused when it is read. Each case starts from the small package of `tests/common/mini.rs`,
//! changes one value of one section and lays the file out again with a valid hash, so what is
//! under test is the rule and not the hash.
#![cfg(feature = "write")]

use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

mod common;

use common::mini;
use common::raw::{self, Entry};
use ui2_package::sections::strings::StringTable;
use ui2_package::sections::templates::Templates;
use ui2_package::style::{Len, LenAuto};
use ui2_package::{
    ARefId, ARefs, Bindings, Bound, Cond, CondKind, ExprId, Extn, Fonts, ImageSrc, Images, ImgId,
    L10n, Manifest, NodeKind, NodeRef, Package, PackageError, Scripts, SectionTag, SrcMap, StrId,
    StyleId, StyleSetId, Styles, TemplateId, TextSrc, TokenId, Tokens, TplId, node_flags,
};

fn read(bytes: &[u8]) -> Result<Package, PackageError> {
    Package::read(Arc::from(bytes))
}

/// The mini package with one structural section changed must be refused as invalid in that section.
fn refused<T: Serialize + DeserializeOwned>(tag: &[u8; 4], what: &str, f: impl FnOnce(&mut T)) {
    let bytes = raw::with::<T>(&mini::build(), tag, f);
    match read(&bytes) {
        Err(PackageError::Invalid { at, what: why }) if at == SectionTag(*tag) => {
            assert!(!why.is_empty(), "{what}: the error says nothing");
        }
        other => panic!("{what}: {other:?}"),
    }
}

/// The same for the index of a blob section.
fn refused_index<T: Serialize + DeserializeOwned>(
    tag: &[u8; 4],
    what: &str,
    f: impl FnOnce(&mut T),
) {
    let bytes = raw::with_index::<T>(&mini::build(), tag, f);
    match read(&bytes) {
        Err(PackageError::Invalid { at, .. }) if at == SectionTag(*tag) => {}
        other => panic!("{what}: {other:?}"),
    }
}

const STR_OUT: StrId = StrId(100_000);

#[test]
fn the_base_package_is_accepted() {
    // The cases below mean something only if the unchanged file reads.
    let bytes = mini::build();
    let pkg = read(&bytes).expect("read");
    pkg.verify_blobs().expect("blobs");
    // Every section survives decode, re-encode and layout unchanged.
    assert_eq!(raw::assemble(&raw::split(&bytes)), bytes);
    assert!(read(&raw::with::<Styles>(&bytes, b"STYL", |_| {})).is_ok());
}

#[test]
fn five_conditions_on_a_node() {
    let hover = Cond {
        up: 0,
        kind: CondKind::Hover,
    };
    refused::<Styles>(b"STYL", "5 conditions", |s| {
        let id = StyleId(0);
        s.sets[0].conds = vec![hover; 5];
        s.sets[0].table = vec![id; 32 * s.cells()];
    });
    // Four are the limit.
    let ok = raw::with::<Styles>(&mini::build(), b"STYL", |s| {
        s.sets[0].conds = vec![hover; 4];
        s.sets[0].table = vec![StyleId(0); 16 * s.cells()];
    });
    assert!(read(&ok).is_ok());
}

#[test]
fn seventeen_viewport_cells() {
    let bounds = |n: usize| -> Vec<Bound> {
        (1..=n)
            .map(|i| Bound {
                px: i as f32 * 100.0,
                upper_inclusive: false,
            })
            .collect()
    };
    // 16 cells (15 bounds on one axis) are the limit; the tables are resized to match.
    let with_cells = |w: usize, h: usize| {
        raw::with::<Styles>(&mini::build(), b"STYL", |s| {
            s.media_w = bounds(w);
            s.media_h = bounds(h);
            let cells = s.cells();
            for set in &mut s.sets {
                set.table = vec![StyleId(0); cells << set.conds.len()];
            }
        })
    };
    assert!(read(&with_cells(15, 0)).is_ok(), "16 cells");
    assert!(read(&with_cells(3, 3)).is_ok(), "16 cells, two axes");
    for (w, h) in [(16, 0), (0, 16), (3, 4), (8, 1)] {
        match read(&with_cells(w, h)) {
            Err(PackageError::Invalid { at, .. }) => assert_eq!(at, SectionTag::STYL, "{w}x{h}"),
            other => panic!("{w}x{h}: {other:?}"),
        }
    }
}

#[test]
fn a_style_table_of_the_wrong_length() {
    refused::<Styles>(b"STYL", "one record too many", |s| {
        s.sets[0].table.push(StyleId(0));
    });
    refused::<Styles>(b"STYL", "an empty table", |s| s.sets[1].table.clear());
    refused::<Styles>(b"STYL", "a condition without its records", |s| {
        s.sets[0].conds.push(Cond {
            up: 0,
            kind: CondKind::Active,
        });
    });
}

#[test]
fn a_nan_or_an_infinity() {
    refused::<Styles>(b"STYL", "NaN in a record", |s| {
        s.records[0].opacity = f32::NAN
    });
    refused::<Styles>(b"STYL", "infinity in a record", |s| {
        s.records[1].width = LenAuto::Len(Len::px(f32::INFINITY));
    });
    refused::<Styles>(b"STYL", "NaN breakpoint", |s| {
        s.media_w = vec![Bound {
            px: f32::NAN,
            upper_inclusive: false,
        }];
        let cells = s.cells();
        for set in &mut s.sets {
            set.table = vec![StyleId(0); cells << set.conds.len()];
        }
    });
    refused::<Bindings>(b"BIND", "NaN in an expression", |b| {
        b.exprs
            .push(ui2_package::profile::expr::Expr::Float(f64::NAN));
    });
    refused_index::<Images>(b"IMGS", "NaN picture size", |i| {
        i.imgs[0].size = [f32::NAN, 4.0]
    });
}

#[test]
fn thirty_three_sections() {
    let bytes = mini::build();
    let mut entries = raw::split(&bytes);
    let mut n = 0u8;
    while entries.len() < 33 {
        entries.push(Entry {
            tag: [b'Z', b'Z', b'0' + n / 10, b'0' + n % 10],
            flags: 0,
            payload: vec![0; 8],
        });
        n += 1;
    }
    assert_eq!(
        read(&raw::assemble(&entries)).err(),
        Some(PackageError::Corrupt { at: None })
    );
    // Thirty-two, with unknown optional sections, are fine.
    entries.pop();
    assert!(read(&raw::assemble(&entries)).is_ok());
}

#[test]
fn a_file_over_sixteen_mebibytes() {
    // The size is checked before anything else: no need for a real package.
    let big = vec![0u8; (16 << 20) + 1];
    assert_eq!(read(&big).err(), Some(PackageError::Corrupt { at: None }));
    // Exactly 16 MiB is still looked at (and fails for another reason: it is not a package).
    let max = vec![0u8; 16 << 20];
    assert!(read(&max).is_err());
    // A real package padded to the limit reads.
    let bytes = mini::build();
    let pad = (16 << 20) - bytes.len();
    let mut entries = raw::split(&bytes);
    entries.push(Entry {
        tag: *b"ZZZZ",
        flags: 0,
        payload: vec![0; pad - 64],
    });
    let padded = raw::assemble(&entries);
    assert!(padded.len() <= 16 << 20);
    assert!(read(&padded).is_ok());
}

#[test]
fn a_for_that_points_back() {
    let set_tpl = |body: u32| {
        move |t: &mut Templates| {
            for n in &mut t.items[0].nodes {
                if let NodeKind::For { tpl, .. } = &mut n.kind {
                    *tpl = TemplateId(body);
                }
            }
        }
    };
    refused::<Templates>(b"TMPL", "a for with itself as the body", set_tpl(0));
    refused::<Templates>(
        b"TMPL",
        "a for with a body that does not exist",
        set_tpl(99),
    );
    // An `if` in the body template pointing at an earlier one.
    refused::<Templates>(b"TMPL", "an if that points back", |t| {
        let n = &mut t.items[1].nodes[1];
        n.kind = NodeKind::If {
            tpl: TemplateId(0),
            cond: ExprId(0),
        };
    });
    // The forward form is accepted.
    assert!(read(&raw::with::<Templates>(&mini::build(), b"TMPL", set_tpl(2))).is_ok());
}

#[test]
fn an_index_outside_its_table_one_kind_at_a_time() {
    // Strings.
    refused::<Templates>(b"TMPL", "template name", |t| t.items[0].name = STR_OUT);
    refused::<Templates>(b"TMPL", "node part", |t| {
        t.items[0].nodes[1].part = Some(STR_OUT)
    });
    refused::<Templates>(b"TMPL", "static text", |t| {
        t.items[1].nodes[1].kind = NodeKind::Text(TextSrc::Static(STR_OUT));
    });
    refused::<Templates>(b"TMPL", "for item", |t| {
        if let NodeKind::For { item, .. } = &mut t.items[0].nodes[2].kind {
            *item = STR_OUT;
        }
    });
    // Style sets and style records.
    refused::<Templates>(b"TMPL", "style set", |t| {
        t.items[0].nodes[1].style = StyleSetId(99);
    });
    refused::<Styles>(b"STYL", "style record in a set", |s| {
        s.sets[0].table[0] = StyleId(99);
    });
    refused::<Styles>(b"STYL", "font family", |s| {
        s.records[0].font_family = vec![STR_OUT]
    });
    refused::<Styles>(b"STYL", "condition class", |s| {
        s.sets[1].conds[0].kind = CondKind::Class(STR_OUT);
    });
    // Expressions and text templates.
    refused::<Templates>(b"TMPL", "for list expression", |t| {
        if let NodeKind::For { list, .. } = &mut t.items[0].nodes[2].kind {
            *list = ExprId(99);
        }
    });
    refused::<Templates>(b"TMPL", "bound text template", |t| {
        t.items[0].nodes[4].kind = NodeKind::Text(TextSrc::Bound(TplId(99)));
    });
    refused::<Templates>(b"TMPL", "bound image expression", |t| {
        t.items[1].nodes[3].kind = NodeKind::Image(ImageSrc::Bound(ExprId(99)));
    });
    // Pictures and game asset references.
    refused::<Templates>(b"TMPL", "picture", |t| {
        t.items[1].nodes[2].kind = NodeKind::Image(ImageSrc::Pkg(ImgId(99)));
    });
    refused::<Templates>(b"TMPL", "asset reference", |t| {
        t.items[1].nodes[3].kind = NodeKind::Image(ImageSrc::Asset(ARefId(99)));
    });
    // Tokens.
    refused::<Tokens>(b"TOKN", "alias token", |t| t.aliases[0].1 = TokenId(99));
    refused::<Tokens>(b"TOKN", "theme override token", |t| {
        t.themes[1].set[0].0 = TokenId(99);
    });
    refused::<Tokens>(b"TOKN", "token name", |t| t.roots[0].0 = STR_OUT);
    // Bindings.
    refused::<Bindings>(b"BIND", "binding template", |b| {
        b.items[0].tpl = TemplateId(99)
    });
    refused::<Bindings>(b"BIND", "binding node", |b| b.items[0].node = 99);
    refused::<Bindings>(b"BIND", "used path", |b| b.used_paths[0].0 = STR_OUT);
    refused::<Bindings>(b"BIND", "handler", |b| b.handlers[0] = STR_OUT);
    refused::<Bindings>(b"BIND", "state path", |b| b.state[0].0 = STR_OUT);
    refused::<Bindings>(b"BIND", "text template segment", |b| {
        b.tpls[0].push(ui2_package::Seg::Expr(ExprId(99)));
    });
    // Texts.
    refused::<L10n>(b"L10N", "text key", |l| l.langs[0].entries[0].0 = STR_OUT);
    // Extension points.
    refused::<Extn>(b"EXTN", "slot node", |e| {
        e.slots[0].1 = NodeRef(TemplateId(0), 99)
    });
    refused::<Extn>(b"EXTN", "part template", |e| {
        e.parts[0].1 = NodeRef(TemplateId(99), 0)
    });
    refused::<Extn>(b"EXTN", "screen name", |e| e.screens[0].0 = STR_OUT);
    refused::<Extn>(b"EXTN", "island key node", |e| e.islands[0].keys[0].1 = 99);
    refused::<Extn>(b"EXTN", "hotkey node", |e| {
        e.hotkeys[0].node = NodeRef(TemplateId(7), 0)
    });
    refused::<Extn>(b"EXTN", "tooltip template", |e| {
        e.tooltips[0].1 = TemplateId(99)
    });
    // The source map.
    refused::<SrcMap>(b"SRCM", "source file", |m| m.templates[0][0].file = 99);
    // The blob sections: strings, pages, byte ranges.
    refused_index::<Fonts>(b"FONT", "font family", |f| f.faces[0].family = STR_OUT);
    refused_index::<Fonts>(b"FONT", "font blob", |f| f.faces[0].blob.offset = 1 << 20);
    refused_index::<Images>(b"IMGS", "sprite page", |i| i.imgs[0].x1.page = 9);
    refused_index::<Images>(b"IMGS", "picture path", |i| i.imgs[0].path = STR_OUT);
    refused_index::<Scripts>(b"SCRP", "script path", |s| s.files[0].0 = STR_OUT);
    refused_index::<Scripts>(b"SCRP", "script read", |s| s.reads[0] = STR_OUT);
    refused_index::<Scripts>(b"SCRP", "script blob", |s| s.files[0].1.len = 1 << 20);
    refused::<ARefs>(b"AREF", "asset reference", |a| a.items[0] = STR_OUT);
}

#[test]
fn a_subtree_that_does_not_tile_its_template() {
    refused::<Templates>(b"TMPL", "subtree larger than the list", |t| {
        t.items[0].nodes[0].subtree = 6;
    });
    refused::<Templates>(b"TMPL", "subtree of zero", |t| {
        t.items[0].nodes[1].subtree = 0
    });
    refused::<Templates>(b"TMPL", "a child that overruns its parent", |t| {
        // Node 2 lies in the box of node 1, which ends at node 3.
        t.items[0].nodes[2].subtree = 2;
    });
    refused::<Templates>(b"TMPL", "a leaf with children", |t| {
        t.items[0].nodes[4].subtree = 2;
    });
    refused::<Templates>(b"TMPL", "unknown flag bits", |t| {
        t.items[0].nodes[3].flags = node_flags::ALL + 1;
    });
}

#[test]
fn the_other_rules_of_the_sections() {
    refused::<StringTable>(b"STRS", "unsorted strings", |s| s.items.swap(0, 1));
    refused::<StringTable>(b"STRS", "repeated strings", |s| {
        let first = s.items[0].clone();
        s.items[1] = first;
    });
    refused::<Manifest>(b"MANI", "an id outside the grammar", |m| {
        m.id = "Bad Id".into()
    });
    refused::<Manifest>(b"MANI", "no theme", |m| m.themes.clear());
    refused::<Tokens>(b"TOKN", "themes that are not the manifest's", |t| {
        t.themes.pop();
    });
    refused::<L10n>(b"L10N", "a language the manifest lacks", |l| {
        l.langs[1].tag = "de_DE".into();
    });
    refused::<ARefs>(b"AREF", "references out of order", |a| {
        a.items = vec![StrId(5), StrId(2)];
    });
    refused_index::<Images>(b"IMGS", "a page larger than 1024", |i| {
        i.pages[0].size = [2048, 1]
    });
    refused_index::<Images>(b"IMGS", "a sprite that leaves its page", |i| {
        i.imgs[0].x1.rect = [3, 3, 4, 4];
    });
    refused_index::<Scripts>(b"SCRP", "a compressed script", |s| s.files[0].1.zstd = true);
    refused_index::<Fonts>(b"FONT", "weight 0", |f| f.faces[0].weight = 0);
    refused_index::<Fonts>(b"FONT", "a stored blob with another raw length", |f| {
        f.faces[0].blob.raw_len += 1;
    });
}
