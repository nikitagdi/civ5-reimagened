//! The writer is a pure function of its data: same data, same bytes, whatever the order the
//! order-free parts were added in; and the small fixture package is byte-for-byte what the writer
//! makes today.
#![cfg(feature = "write")]

use std::sync::Arc;

mod common;

use common::{mini, raw};
use ui2_package::{Package, SectionTag};

const FIXTURE: &[u8] = include_bytes!("fixtures/mini.ui2");

fn read(bytes: &[u8]) -> Package {
    Package::read(Arc::from(bytes)).expect("read")
}

#[test]
fn two_builds_in_a_row_give_the_same_bytes_and_hash() {
    let (a, b) = (mini::build(), mini::build());
    assert_eq!(a, b);
    assert_eq!(read(&a).hash(), read(&b).hash());
    assert_eq!(&read(&a).hash().0[..], &a[a.len() - 32..]);
}

#[test]
fn another_insertion_order_gives_the_same_bytes() {
    let forward = mini::build();
    let backward = mini::build_reversed();
    assert_eq!(forward, backward);
    // Strings alone, interned in different orders before anything else.
    let words = ["zeta", "alpha", "mid", "Beta", "alpha"];
    let make = |rev: bool| {
        let mut b = mini::builder(false);
        let mut list = words.to_vec();
        if rev {
            list.reverse();
        }
        for w in list {
            b.str(w);
        }
        b.finish(mini::manifest()).expect("finish")
    };
    assert_eq!(make(false), make(true));
}

#[test]
fn the_file_is_laid_out_by_the_rules() {
    let bytes = mini::build();
    assert_eq!(bytes.len() % 8, 0);
    let entries = raw::split(&bytes);
    // Ascending tags, every payload starts on a multiple of 8.
    assert!(entries.windows(2).all(|w| w[0].tag < w[1].tag));
    for e in &entries {
        let at = raw::range_of(&bytes, &e.tag).start;
        assert_eq!(at % 8, 0, "{}", SectionTag(e.tag));
    }
    // Every section of the format is there.
    for tag in [
        *b"MANI", *b"STRS", *b"TMPL", *b"STYL", *b"STYP", *b"TOKN", *b"BIND", *b"L10N", *b"EXTN",
        *b"SRCM", *b"FONT", *b"IMGS", *b"SCRP", *b"AREF",
    ] {
        assert!(entries.iter().any(|e| e.tag == tag), "{}", SectionTag(tag));
    }
    // The writer's layout is the raw layout: laying the same sections out again changes nothing.
    assert_eq!(raw::assemble(&entries), bytes);
}

#[test]
fn the_fixture_is_what_the_writer_makes() {
    let bytes = mini::build();
    assert!(
        bytes.as_slice() == FIXTURE,
        "tests/fixtures/mini.ui2 differs from the writer's output ({} bytes now, {} in the \
         fixture); if the format changed on purpose, run `cargo test -p ui2-package --features \
         write --test determinism -- --ignored write_the_fixture` and commit the new file",
        bytes.len(),
        FIXTURE.len()
    );
    // And it reads, with its hash in the name the host would give the file.
    let pkg = read(FIXTURE);
    assert_eq!(pkg.manifest().id, "mini");
    assert_eq!(pkg.hash().short().len(), 16);
    pkg.verify_blobs().expect("blobs");
}

/// Rewrites the fixture from the writer. Run by hand after a deliberate change of the format.
#[test]
#[ignore = "rewrites tests/fixtures/mini.ui2"]
fn write_the_fixture() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mini.ui2");
    std::fs::write(path, mini::build()).expect("write the fixture");
}
