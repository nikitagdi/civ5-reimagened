//! The samples that skins ship in `skins/<id>/sample/` must pass the schema check.

use std::path::{Path, PathBuf};

use ui2_model::generated as g;
use ui2_model::{ModelStore, Sample, Schema};

fn skins_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../skins")
}

fn load(skin: &str, name: &str) -> Sample {
    let path = skins_dir().join(skin).join("sample").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Sample::parse(Schema::v1(), &text)
        .unwrap_or_else(|errs| panic!("{}:\n{}", path.display(), errs.join("\n")))
}

#[test]
fn engraved_hud_sample_is_valid_and_has_the_numbers_of_the_mockup() {
    let sample = load("engraved", "hud.json");
    let mut store = ModelStore::new(Schema::v1());
    sample.apply(&mut store).expect("applies");
    store.take_frame().expect("a frame");

    let top = store.get::<g::Top>().unwrap();
    assert_eq!(top.science_rate, 48);
    assert_eq!((top.gold, top.gold_rate), (312, 21));
    assert_eq!(top.happiness, 7);
    assert_eq!(
        (top.golden_age.progress, top.golden_age.threshold),
        (412, 650)
    );
    assert_eq!(top.culture_rate, 31);
    assert_eq!(top.policy_turns, Some(6));
    assert_eq!((top.faith, top.faith_rate), (140, 9));
    assert_eq!(top.tourism, Some(4));
    let have: Vec<i32> = top.resources.iter().map(|r| r.available).collect();
    assert_eq!(have, [2, 4, 0]);

    let game = store.get::<g::Game>().unwrap();
    assert_eq!((game.turn, game.year), (87, 1250));

    let current = store
        .get::<g::Research>()
        .unwrap()
        .current
        .as_ref()
        .expect("current research");
    assert_eq!(current.turns, Some(3));
    assert_eq!((current.fraction * 100.0).round() as i32, 62);
}

#[test]
fn engraved_hud_sample_is_the_top_strip_fixture() {
    let fixture = include_str!("fixtures/top-sample.json");
    let a = Sample::parse(Schema::v1(), fixture).unwrap();
    let b = load("engraved", "hud.json");
    // The hud sample grew past the top strip (the helm, the unit panel, notifications): every root of the
    // fixture is in it with the same value.
    for (rid, v) in a.roots() {
        let other = b.roots().iter().find(|(r, _)| r == rid).map(|(_, v)| v);
        assert_eq!(other, Some(v), "root {rid:?}");
    }
}
