//! Version compatibility of a package with the client: the container format (`Package::read`)
//! and the manifest's profile, model and script API versions (`Package::check_compat`).
#![cfg(feature = "write")]

use std::sync::Arc;

use ui2_package::profile::Profile;
use ui2_package::{
    FaceFamily, FontRoles, Kind, Manifest, PROFILE_VERSION, Package, PackageBuilder, PackageError,
    Tool, Versions,
};

const CLIENT: Versions = Versions {
    profile: (1, 0),
    model: (1, 3),
    api: (1, 2),
};

fn manifest(profile: (u16, u16), model: (u16, u16), api: (u16, u16)) -> Manifest {
    Manifest {
        kind: Kind::Skin,
        id: "compat".into(),
        version: "1.0.0".into(),
        title_key: "skin.title".into(),
        profile,
        model,
        api,
        default: false,
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
        schema_hash: 0,
    }
}

fn bytes(profile: (u16, u16), model: (u16, u16), api: (u16, u16)) -> Vec<u8> {
    PackageBuilder::new()
        .finish(manifest(profile, model, api))
        .expect("finish")
}

fn package(profile: (u16, u16), model: (u16, u16), api: (u16, u16)) -> Package {
    Package::read(Arc::from(bytes(profile, model, api))).expect("read")
}

fn rehash(bytes: &mut [u8]) {
    let n = bytes.len() - 32;
    let hash = blake3::hash(&bytes[..n]);
    bytes[n..].copy_from_slice(hash.as_bytes());
}

fn incompatible(r: Result<(), PackageError>) -> String {
    match r {
        Err(PackageError::Incompatible { need }) => need,
        other => panic!("expected Incompatible, got {other:?}"),
    }
}

#[test]
fn the_same_or_older_versions_are_compatible() {
    assert_eq!(
        package((1, 0), (1, 3), (1, 2)).check_compat(&CLIENT),
        Ok(())
    );
    // An older model minor, and any profile or script API minor of the same major.
    assert_eq!(
        package((1, 7), (1, 0), (1, 9)).check_compat(&CLIENT),
        Ok(())
    );
}

#[test]
fn a_newer_format_major_is_refused_and_a_newer_minor_is_read() {
    let mut b = bytes((1, 0), (1, 0), (1, 0));
    b[8] = 2; // format major 2
    rehash(&mut b);
    assert!(matches!(
        Package::read(Arc::from(b)),
        Err(PackageError::Incompatible { .. })
    ));
    let mut b = bytes((1, 0), (1, 0), (1, 0));
    b[10] = 1; // format minor 1, no unknown sections
    rehash(&mut b);
    let pkg = Package::read(Arc::from(b)).expect("a newer minor reads");
    assert_eq!(pkg.format_minor(), 1);
    assert_eq!(pkg.check_compat(&CLIENT), Ok(()));
}

#[test]
fn another_profile_major_is_refused() {
    let need = incompatible(package((2, 0), (1, 0), (1, 0)).check_compat(&CLIENT));
    assert!(need.contains("UI profile 2.0"), "{need}");
    assert!(need.contains("1.0"), "{need}");
    let old = Versions {
        profile: (2, 0),
        ..CLIENT
    };
    incompatible(package((1, 0), (1, 0), (1, 0)).check_compat(&old));
}

#[test]
fn a_newer_model_minor_or_another_model_major_is_refused() {
    let need = incompatible(package((1, 0), (1, 4), (1, 0)).check_compat(&CLIENT));
    assert!(need.contains("view model 1.4"), "{need}");
    assert!(need.contains("1.3"), "{need}");
    incompatible(package((1, 0), (2, 0), (1, 0)).check_compat(&CLIENT));
    incompatible(package((1, 0), (0, 9), (1, 0)).check_compat(&CLIENT));
}

#[test]
fn another_script_api_major_is_refused() {
    let need = incompatible(package((1, 0), (1, 0), (2, 0)).check_compat(&CLIENT));
    assert!(need.contains("script API 2.0"), "{need}");
    incompatible(package((1, 0), (1, 0), (0, 5)).check_compat(&CLIENT));
}

#[test]
fn the_profile_constant_is_the_embedded_profile_version() {
    assert_eq!(PROFILE_VERSION, Profile::v1().version());
}
