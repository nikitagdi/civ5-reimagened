//! The sample mode (`?ui2sample=`): the first skin's `sample/hud.json` plays the game; the strip shows its values
//! without a replica or a network, its `ui` part opens what it says, and a bad sample changes nothing.

mod support;

use std::path::PathBuf;

use ui2_host::{Env, V2Frame, V2Init, V2Ui};

fn sample(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../skins/engraved/sample")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn frame(ui: &mut V2Ui, now: f64) {
    let r = ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: now,
    });
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

#[test]
fn the_strip_shows_the_sample_without_a_game() {
    let text = sample("hud.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("json");
    let gold = json["model"]["top"]["gold"].to_string();
    let turn = json["model"]["game"]["turn"].to_string();
    let mut ui = V2Ui::new(
        support::strip(),
        V2Init {
            env: Env {
                viewport_px: (1280, 720),
                ..Env::default()
            },
            ..V2Init::default()
        },
    )
    .expect("the skin loads");
    ui.set_sample(&text).expect("the sample reads");
    assert!(ui.sample_mode());
    frame(&mut ui, 16.0);
    let texts = ui.texts();
    assert!(texts.contains(&gold), "gold {gold} in {texts:?}");
    assert!(
        texts.contains(&format!("Turn {turn}")),
        "turn {turn} in {texts:?}"
    );
    // Later frames write nothing more: the sample is a still picture.
    let rev = ui.store().revision();
    frame(&mut ui, 32.0);
    assert_eq!(ui.store().revision(), rev);
    ui.place_sample_anchors(1.0);
}

#[test]
fn a_sample_that_does_not_fit_the_model_is_refused() {
    let mut ui = V2Ui::new(support::strip(), V2Init::default()).expect("the skin loads");
    let err = ui
        .set_sample(r#"{"model": {"top": {"gold": "lots"}}}"#)
        .expect_err("refused");
    assert!(err.contains("top"), "{err}");
    assert!(!ui.sample_mode());
}
