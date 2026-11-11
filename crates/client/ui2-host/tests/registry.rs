//! The skin index, the checks of a package against it, and the fallback after a failure.

mod support;

use std::sync::Arc;

use ui2_host::{
    ApiVersion, Fallback, LoadFailure, RegistryError, SkinRegistry, V2Init, V2Ui, hash_hex,
};
use ui2_package::profile::TypeSig;
use ui2_runtime::testkit::Kit;

fn index(entries: &[serde_json::Value]) -> String {
    serde_json::Value::Array(entries.to_vec()).to_string()
}

fn entry(id: &str, hash: &str, default: bool) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "file": format!("{id}-{}.ui2", &hash[..16]),
        "hash": hash,
        "title_key": format!("TXT_KEY_SKIN_{}", id.to_uppercase()),
        "themes": ["dark", "light"],
        "api": "1.0",
        "default": default,
    })
}

const H1: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const H2: &str = "ffeeddccbbaa9988";

#[test]
fn the_index_parses() {
    let mut b = entry("vellum", H2, false);
    // Unknown fields are ignored; the API may be written as a pair.
    b["api"] = serde_json::json!([1, 2]);
    b["extra"] = serde_json::json!(true);
    let reg = SkinRegistry::parse(&index(&[entry("engraved", H1, true), b])).expect("index");
    assert_eq!(reg.entries().len(), 2);
    let e = reg.get("engraved").expect("engraved");
    assert_eq!(e.file, format!("engraved-{}.ui2", &H1[..16]));
    assert_eq!(e.path(), format!("ui2/engraved-{}.ui2", &H1[..16]));
    assert_eq!(e.title_key, "TXT_KEY_SKIN_ENGRAVED");
    assert_eq!(e.themes, ["dark", "light"]);
    assert_eq!(e.api, ApiVersion { major: 1, minor: 0 });
    assert_eq!(
        reg.get("vellum").map(|e| e.api.to_string()),
        Some("1.2".into())
    );
    assert_eq!(reg.default_skin().map(|e| e.id.as_str()), Some("engraved"));
    // `v2:vellum`, `v2`, and a skin another build has.
    assert_eq!(
        reg.resolve(Some("vellum")).map(|e| e.id.as_str()),
        Some("vellum")
    );
    assert_eq!(reg.resolve(None).map(|e| e.id.as_str()), Some("engraved"));
    assert_eq!(
        reg.resolve(Some("gilded")).map(|e| e.id.as_str()),
        Some("engraved")
    );
    assert!(
        SkinRegistry::parse("[]")
            .expect("empty")
            .resolve(None)
            .is_none()
    );
}

#[test]
fn bad_indexes_are_refused() {
    let bad = |v: serde_json::Value| SkinRegistry::parse(&index(&[v])).unwrap_err();
    assert!(matches!(
        SkinRegistry::parse("{}"),
        Err(RegistryError::Json(_))
    ));
    let mut e = entry("a", H1, false);
    e["file"] = "../a.ui2".into();
    assert!(matches!(bad(e), RegistryError::Entry { index: 0, .. }));
    let mut e = entry("a", H1, false);
    e["hash"] = "ABCDEF0123456789".into();
    assert!(matches!(bad(e), RegistryError::Entry { .. }));
    let mut e = entry("a", H1, false);
    e["hash"] = "0123".into();
    assert!(matches!(bad(e), RegistryError::Entry { .. }));
    let mut e = entry("a", H1, false);
    e["api"] = "one".into();
    assert!(matches!(bad(e), RegistryError::Entry { .. }));
    let mut e = entry("a", H1, false);
    e["id"] = "A b".into();
    assert!(matches!(bad(e), RegistryError::Entry { .. }));
    let mut e = entry("a", H1, false);
    e.as_object_mut().expect("object").remove("hash");
    assert!(matches!(bad(e), RegistryError::Entry { .. }));
    assert_eq!(
        SkinRegistry::parse(&index(&[entry("a", H1, false), entry("a", H2, false)])).unwrap_err(),
        RegistryError::Duplicate("a".into())
    );
    assert_eq!(
        SkinRegistry::parse(&index(&[entry("a", H1, true), entry("b", H2, true)])).unwrap_err(),
        RegistryError::SeveralDefaults("a".into(), "b".into())
    );
}

#[test]
fn a_package_is_checked_against_the_index() {
    let pkg = support::strip();
    let bytes = Arc::clone(pkg.bytes());
    let full = hash_hex(&pkg.hash());
    assert_eq!(full.len(), 64);
    assert!(full.starts_with(&pkg.hash().short()));

    // The full hash and the 16-digit prefix both match.
    for hash in [full.as_str(), &full[..16]] {
        let reg = SkinRegistry::parse(&index(&[entry("kit", hash, true)])).expect("index");
        let opened = reg.open("kit", Arc::clone(&bytes)).expect("opens");
        assert_eq!(opened.hash(), pkg.hash());
        V2Ui::new(opened, V2Init::default()).expect("runs");
    }

    let fail =
        |reg: &SkinRegistry, id: &str, bytes: Arc<[u8]>| reg.open(id, bytes).expect_err("refused");
    // Another file than the index names.
    let reg = SkinRegistry::parse(&index(&[entry("kit", H1, true)])).expect("index");
    let e = fail(&reg, "kit", Arc::clone(&bytes));
    assert_eq!(e.skin, "kit");
    assert_eq!(
        e.failure,
        LoadFailure::HashMismatch {
            index: H1.into(),
            file: full.clone(),
        }
    );
    // Not in the index.
    assert_eq!(
        fail(&reg, "nope", Arc::clone(&bytes)).failure,
        LoadFailure::UnknownSkin
    );
    // Damaged bytes.
    let mut damaged = bytes.to_vec();
    let mid = damaged.len() / 2;
    damaged[mid] ^= 0x55;
    assert!(matches!(
        fail(&reg, "kit", Arc::from(damaged)).failure,
        LoadFailure::Package(_)
    ));
    // The right file under another skin's name.
    let reg = SkinRegistry::parse(&index(&[entry("other", &full, true)])).expect("index");
    assert_eq!(
        fail(&reg, "other", Arc::clone(&bytes)).failure,
        LoadFailure::IdMismatch {
            index: "other".into(),
            package: "kit".into(),
        }
    );
}

#[test]
fn an_incompatible_or_broken_package_is_a_load_error() {
    // A model path this client does not have.
    let mut kit = support::strip_kit(&support::fira());
    kit.used_path("top.no_such_field", TypeSig::Int);
    let pkg = kit.finish().expect("package");
    let full = hash_hex(&pkg.hash());
    let reg = SkinRegistry::parse(&index(&[entry("kit", &full, true)])).expect("index");
    let e = reg
        .open("kit", Arc::clone(pkg.bytes()))
        .expect_err("refused");
    assert!(matches!(e.failure, LoadFailure::Incompatible(_)), "{e}");
    // `V2Ui::new` runs the same checks for a package that comes without an index (samples, dev reload).
    let e = V2Ui::new(pkg, V2Init::default()).err().expect("refused");
    assert!(matches!(e.failure, LoadFailure::Incompatible(_)), "{e}");

    // A font that does not parse.
    let pkg = support::strip_kit(b"not a font").finish().expect("package");
    let e = V2Ui::new(pkg, V2Init::default()).err().expect("refused");
    assert!(matches!(e.failure, LoadFailure::Fonts(_)), "{e}");

    // No document.
    let pkg = Kit::new().finish().expect("package");
    let e = V2Ui::new(pkg, V2Init::default()).err().expect("refused");
    assert!(matches!(e.failure, LoadFailure::Tree(_)), "{e}");
    assert!(e.to_string().starts_with("skin kit: "));
}

#[test]
fn a_failed_skin_falls_back_without_a_loop() {
    let mut reg = SkinRegistry::parse(&index(&[
        entry("engraved", H1, true),
        entry("vellum", H2, false),
    ]))
    .expect("index");
    // A non-default skin fails: the default one.
    assert_eq!(reg.fail("vellum"), Fallback::Skin("engraved".into()));
    assert!(reg.is_broken("vellum"));
    assert_eq!(
        reg.resolve(Some("vellum")).map(|e| e.id.as_str()),
        Some("engraved")
    );
    // The default fails too: classic, and nothing is offered again.
    assert_eq!(reg.fail("engraved"), Fallback::Classic);
    assert!(reg.resolve(Some("vellum")).is_none());
    assert!(reg.resolve(None).is_none());
    // The default failing first: classic straight away.
    let mut reg = SkinRegistry::parse(&index(&[entry("engraved", H1, true)])).expect("index");
    assert_eq!(reg.fail("engraved"), Fallback::Classic);
}
