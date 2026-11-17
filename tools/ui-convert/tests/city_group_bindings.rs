//! Validate city-group source bindings without starting a browser.
use std::path::Path;
use std::process::Command;
use ui2_convert::{
    Source, bindings, check,
    ir::{Dom, Ir},
};

#[test]
fn city_group_bindings_match_schema_and_state() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scan = Command::new("node")
        .arg(dir.join("tests/city_group_scan.mjs"))
        .output()
        .expect("node source scan");
    assert!(
        scan.status.success(),
        "{}",
        String::from_utf8_lossy(&scan.stderr)
    );
    let ir: Ir = serde_json::from_slice(&scan.stdout).unwrap();
    let skin_dir = dir.join("../../skins/engraved");
    let source = Source::load(&skin_dir).unwrap();
    let (statics, errors) = check::check(&source.skin, &ir, &skin_dir);
    // This scan intentionally has no shared fonts or token declarations.
    let syntax: Vec<_> = errors
        .iter()
        .filter(|e| {
            ["E1", "E2", "E3", "E4"]
                .iter()
                .any(|prefix| e.code.starts_with(prefix))
        })
        .collect();
    assert!(syntax.is_empty(), "{syntax:#?}");
    let (_, errors) = bindings::check(
        &Dom::new(&ir.dom),
        &statics,
        &source.skin,
        ui2_model::Schema::v1(),
    );
    assert!(errors.is_empty(), "{errors:#?}");
    let (_, errors) = bindings::check_samples(&skin_dir, &source.skin, ui2_model::Schema::v1());
    assert!(errors.is_empty(), "{errors:#?}");
}
