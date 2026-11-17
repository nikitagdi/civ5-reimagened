//! Source-only military profile/type checks; browser layout remains a separate check.
use std::{path::Path, process::Command};
use ui2_convert::{Input, check_skin};

#[test]
fn military_source_satisfies_the_static_profile_and_model() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("node")
        .arg(dir.join("tests/economy-static.mjs"))
        .output()
        .expect("node");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let dump = String::from_utf8(output.stdout).unwrap();
    let run = check_skin(&dir.join("../../skins/engraved"), Input::Dump(&dump));
    let errors: Vec<_> = run
        .errors
        .iter()
        .filter(|e| {
            let place = e.place.to_string();
            [
                "economy",
                "military",
                "trade_routes",
                "espionage",
                "victory",
                "demographics",
            ]
            .iter()
            .any(|screen| {
                place.contains(&format!("{screen}.html"))
                    || place.contains(&format!("{screen}.css"))
                    || place.contains(&format!("{screen}.js"))
                    || e.why.contains(&format!("{screen}."))
            })
        })
        .map(ToString::to_string)
        .collect();
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    assert!(
        run.bindings
            .as_ref()
            .expect("typed bindings")
            .paths
            .contains_key("military.units")
    );
}
