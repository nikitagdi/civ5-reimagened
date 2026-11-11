//! The skin index of the web build: its shape is the one the application's registry accepts, and the registry
//! opens the package the index names.

use std::path::Path;
use std::sync::Arc;

use ui2_convert::index::{self, Entry};
use ui2_host::SkinRegistry;
use ui2_package::Package;

fn fixture() -> (String, Vec<u8>) {
    let bytes = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/top-strip/top.ui2"),
    )
    .expect("top.ui2");
    let pkg = Package::read(Arc::from(bytes.as_slice())).expect("package");
    let hash = pkg
        .hash()
        .0
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    (format!("engraved-{}.ui2", &hash[..16]), bytes)
}

#[test]
fn index_is_accepted_by_the_registry_and_opens_the_package() {
    let (file, bytes) = fixture();
    let entry = index::entry_of(&file, &bytes).expect("entry");
    let text = index::index_text(std::slice::from_ref(&entry));
    let reg = SkinRegistry::parse(&text).expect("the registry reads the index");
    let e = reg.get("engraved").expect("engraved");
    assert_eq!(e.file, file);
    assert_eq!(e.hash.len(), 64, "the full hash");
    assert!(e.hash.starts_with(&file["engraved-".len()..][..16]));
    assert!(e.default);
    assert!(!e.themes.is_empty());
    assert_eq!(e.api.to_string(), "1.0");
    assert_eq!(e.path(), format!("ui2/{file}"));
    let pkg = reg
        .open("engraved", Arc::from(bytes.as_slice()))
        .expect("the registry opens the package");
    assert_eq!(pkg.manifest().id, "engraved");
}

#[test]
fn entry_rejects_a_file_name_that_does_not_match_the_package() {
    let (_, bytes) = fixture();
    assert!(index::entry_of("engraved-0000000000000000.ui2", &bytes).is_err());
    assert!(index::entry_of("engraved.ui2", &bytes).is_err());
}

#[test]
fn publish_writes_the_folder_and_removes_old_packages() {
    let (file, bytes) = fixture();
    let tmp = std::env::temp_dir().join(format!("ui2-index-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let src = tmp.join("src");
    let web = tmp.join("web");
    std::fs::create_dir_all(&src).expect("mkdir");
    std::fs::create_dir_all(web.join("ui2")).expect("mkdir");
    std::fs::write(src.join(&file), &bytes).expect("write");
    std::fs::write(web.join("ui2/old-0123456789abcdef.ui2"), b"stale").expect("write");

    let entries: Vec<Entry> = index::publish(&web, &[src.join(&file)]).expect("publish");
    assert_eq!(entries.len(), 1);
    assert!(web.join("ui2").join(&file).is_file());
    assert!(!web.join("ui2/old-0123456789abcdef.ui2").exists());
    let text = std::fs::read_to_string(web.join("ui2/skins.json")).expect("skins.json");
    assert!(!text.contains('\r'));
    SkinRegistry::parse(&text).expect("parse");
    let _ = std::fs::remove_dir_all(&tmp);
}
