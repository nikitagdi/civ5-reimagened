//! Source-level popup profile checks which do not launch a browser.
use std::{path::Path, process::Command};
use ui2_convert::{
    bindings, check,
    ir::{Dom, Ir},
    l10n, script,
    skin::Skin,
};

#[test]
fn popup_sources_fit_the_profile_without_chromium() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let skin_dir = manifest.join("../../skins/engraved");
    let skin = Skin::parse(&std::fs::read_to_string(skin_dir.join("skin.toml")).unwrap()).unwrap();
    let output = Command::new("node")
        .arg(manifest.join("tests/popup-scan.mjs"))
        .arg(&skin_dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ir: Ir = serde_json::from_slice(&output.stdout).unwrap();
    let (statics, mut errors) = check::check(&skin, &ir, &skin_dir);
    let (bound, bound_errors) =
        bindings::check(&Dom::new(&ir.dom), &statics, &skin, ui2_model::Schema::v1());
    errors.extend(bound_errors);
    let (_, text_errors) = l10n::check(&skin_dir, &skin, &bound, &[], &[]);
    assert!(text_errors.is_empty(), "{text_errors:#?}");
    let names = [
        "confirm",
        "message",
        "goody_choice",
        "notification_log",
        "game_menu",
        "end_game",
        "advisor",
    ];
    for name in names {
        let file = format!("js/{name}.js");
        if let Ok(source) = std::fs::read_to_string(skin_dir.join(&file)) {
            errors.extend(script::check(&file, &source));
        }
    }
    // Existing HUD and technology sources are maintained separately.
    let own: Vec<_> = errors
        .into_iter()
        .filter(|e| {
            names.iter().any(|n| {
                let json = serde_json::to_string(e).unwrap();
                json.contains(&format!("parts/{n}.html"))
                    || json.contains("parts/notification_log_panel.html")
                    || json.contains(&format!("css/{n}.css"))
                    || json.contains(&format!("js/{n}.js"))
            })
        })
        .collect();
    assert!(own.is_empty(), "{own:#?}");
}
