//! Full authored skin coverage without Chromium; computed layout is checked separately.
use std::{fs, path::Path, process::Command};
use ui2_convert::{
    Source, bindings, check,
    ir::{Dom, Ir},
    script,
};

#[test]
fn integrated_skin_has_valid_static_profile_bindings_scripts_and_samples() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scan = Command::new("node")
        .current_dir(&root)
        .args([
            "tools/playtest/scenarios/ui2-screen-policy-profile.mjs",
            "-",
        ])
        .output()
        .expect("source scanner");
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let envelope: serde_json::Value = serde_json::from_slice(&scan.stdout).unwrap();
    let ir: Ir = serde_json::from_value(envelope["scan"].clone()).unwrap();
    let dir = root.join("skins/engraved");
    let source = Source::load(&dir).unwrap();
    let (statics, errors) = check::check(&source.skin, &ir, &dir);
    assert!(errors.is_empty(), "{errors:#?}");
    let (_, errors) = ui2_convert::plan::plan(&Dom::new(&ir.dom), &statics);
    assert!(errors.is_empty(), "{errors:#?}");
    let (_, errors) = bindings::check(
        &Dom::new(&ir.dom),
        &statics,
        &source.skin,
        ui2_model::Schema::v1(),
    );
    assert!(errors.is_empty(), "{errors:#?}");
    let (_, errors) = bindings::check_samples(&dir, &source.skin, ui2_model::Schema::v1());
    assert!(errors.is_empty(), "{errors:#?}");
    for file in &source.skin.scripts.files {
        let errors = script::check(file, &fs::read_to_string(dir.join(file)).unwrap());
        assert!(errors.is_empty(), "{errors:#?}");
    }
}
