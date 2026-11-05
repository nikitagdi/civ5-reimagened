//! FGX of every unit model on the real install; runs only with `CIV5_ROOT` set (use `--release`).

use asset_pipeline::game::GameSource;
use asset_pipeline::models::fgx::{NameRole, Unresolved};
use asset_pipeline::models::material_desc::FgxReport;
use asset_pipeline::models::{NameDict, check_unit_models};

#[test]
fn every_unit_model_reads_and_its_textures_resolve() {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return;
    };
    let game = GameSource::open(root).unwrap();
    let report = check_unit_models(&game, &NameDict::builtin()).unwrap();
    for l in report.lines().iter().take(8) {
        println!("{l}");
    }
    assert!(report.models > 250, "{} models", report.models);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(
        report.unresolved_textures.is_empty(),
        "{:?}",
        report.unresolved_textures
    );
    assert!(report.shader_sets.contains_key("UnitShader_Skinned"));
    // The names the dictionary leaves (shader set, block, params), without a shader catalog.
    let required = report
        .unresolved_names
        .keys()
        .filter(|n| n.role != NameRole::Fallback)
        .count();
    assert!(required <= 9, "{required} unresolved required names");
}

#[test]
fn unknown_names_fail_only_with_a_catalog() {
    let mut report = FgxReport::default();
    report.unresolved_names.insert(
        Unresolved {
            role: NameRole::Param,
            hash: 0x1234_5678,
        },
        ["tank.fxsxml".to_owned()].into(),
    );
    report.unresolved_names.insert(
        Unresolved {
            role: NameRole::Fallback,
            hash: 0x9999_9999,
        },
        ["tank.fxsxml".to_owned()].into(),
    );
    assert!(report.failures(false).is_empty());
    let f = report.failures(true);
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(f[0].contains("#12345678"));

    report.unresolved_textures.push("tank.fxsxml: ...".into());
    assert_eq!(report.failures(false).len(), 1);
}
