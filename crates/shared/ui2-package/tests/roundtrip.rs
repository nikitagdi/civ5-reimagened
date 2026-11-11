//! Write and read back. Each section family adds its cases here.
#![cfg(feature = "write")]

use std::sync::Arc;

use ui2_package::{
    FaceFamily, FontRoles, Kind, Manifest, Package, PackageBuilder, PackageError, Tool,
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
    assert_eq!(pkg.str_count(), 0);
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
    assert_eq!(pkg.str_count(), 4);
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
