//! `StaticSource`: a sample file plays the part of the live game. The roots come out as the replica source's
//! would, nothing is read, and a host swaps one source for the other without changing its frame code.

use std::collections::BTreeSet;

use ui2_model::r#gen::{Game, Meta, Phase, Research, Top, Ui};
use ui2_model::{AssetRef, Key, KeyedList, ModelStore, Owner, Sample, Schema, Text};
use ui2_source::session::testing::FakeSession;
use ui2_source::{SourceFrame, SourceStats, StaticSource};

const HUD: &str = r#"{
  "model": {
    "game": { "turn": 87, "year": 1250, "year_text": { "k": "year.ad", "a": ["1250"] },
              "era": { "id": 2, "name": "Medieval" }, "me": "0:0", "phase": "humans",
              "mode": "simultaneous", "winner": null },
    "top": {
      "science_rate": 48, "gold": 312, "gold_rate": 21, "happiness": 7, "mood": "content",
      "golden_age": { "active": false, "turns": 0, "progress": 412, "threshold": 650 },
      "culture": 200, "culture_rate": 31, "policy_cost": 360, "policy_turns": 6,
      "faith": 140, "faith_rate": 9, "tourism": 14,
      "resources": [
        { "id": "0:3", "name": "Iron", "icon": "icon:RESOURCE_ATLAS:5", "available": 2, "used": 4, "total": 6 }
      ],
      "policies": [ { "id": 0, "name": "Tradition", "unlocked": true, "owned": 3, "total": 5 } ]
    }
  },
  "state": { "tab": "a" },
  "anchors": { "city:3": [1320, 640] }
}"#;

/// The real sample of the first skin, if the checkout has it.
const SKIN_HUD: &str = include_str!("../../../../skins/engraved/sample/hud.json");

struct Rig {
    store: ModelStore,
    session: FakeSession,
    ui: Ui,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            store: ModelStore::new(Schema::v1()),
            session: FakeSession::default(),
            ui: Ui::default(),
        }
    }

    /// One frame as the host makes it: no replica, nothing dismissed.
    fn frame(&mut self, src: &mut StaticSource) -> SourceStats {
        let mut w = self.store.writer(Owner::Source);
        src.frame(
            SourceFrame {
                replica: None,
                steps: &[],
                session: &self.session,
                dismissed: &BTreeSet::new(),
                now_ms: 0,
            },
            &mut w,
            &self.ui,
        )
    }
}

fn source(text: &str) -> StaticSource {
    let sample = Sample::parse(Schema::v1(), text).unwrap_or_else(|e| panic!("sample: {e:?}"));
    StaticSource::new(sample)
}

#[test]
fn the_sample_gives_the_top_bar_of_the_mockup() {
    let mut rig = Rig::new();
    let mut src = source(HUD);
    let stats = rig.frame(&mut src);
    assert_eq!(stats.roots_built, 2, "game and top");
    assert_eq!(
        (stats.getter_calls, stats.deferred, stats.spent_us),
        (0, 0, 0)
    );
    assert!(src.errors().is_empty());

    let top = rig.store.get::<Top>().expect("top is written");
    assert_eq!(
        (top.gold, top.science_rate, top.tourism),
        (312, 48, Some(14))
    );
    assert_eq!(top.policy_turns, Some(6));
    assert_eq!(
        top.resources,
        KeyedList::from_vec(vec![ui2_model::r#gen::Resource {
            id: Key::from_i32(3),
            name: Text::raw("Iron"),
            icon: AssetRef::new("icon:RESOURCE_ATLAS:5"),
            available: 2,
            used: 4,
            total: 6,
        }])
        .unwrap()
    );
    assert_eq!(top.policies.len(), 1);

    let game = rig.store.get::<Game>().expect("game is written");
    assert_eq!(
        (game.turn, game.year, game.phase),
        (87, 1250, Phase::Humans)
    );
    assert_eq!(game.year_text, Text::key("year.ad", &["1250"]));
    assert_eq!(game.me, Key::from_i32(0));
    assert!(
        rig.store.get::<Research>().is_none(),
        "only what the sample holds"
    );
    assert!(rig.store.take_frame().is_some());
}

#[test]
fn roots_are_written_once_and_again_after_a_reset() {
    let mut rig = Rig::new();
    let mut src = source(HUD);
    assert_eq!(rig.frame(&mut src).roots_built, 2);
    assert!(rig.store.take_frame().is_some());

    for _ in 0..50 {
        assert_eq!(rig.frame(&mut src), SourceStats::default());
    }
    assert!(rig.store.take_frame().is_none(), "no write after the first");

    // The roots are written again into a new store after a reset; the same values give no delta in the old one.
    src.reset();
    assert_eq!(rig.frame(&mut src).roots_built, 2);
    assert!(
        rig.store.take_frame().is_none(),
        "the same values change nothing"
    );
    let mut fresh = Rig::new();
    src.reset();
    assert_eq!(fresh.frame(&mut src).roots_built, 2);
    assert_eq!(fresh.store.get::<Top>().map(|t| t.gold), Some(312));
}

#[test]
fn a_new_sample_replaces_the_roots() {
    let mut rig = Rig::new();
    let mut src = source(HUD);
    rig.frame(&mut src);
    rig.store.take_frame();
    let changed = HUD.replace("\"gold\": 312", "\"gold\": 999");
    src.set_sample(Sample::parse(Schema::v1(), &changed).expect("sample"));
    rig.frame(&mut src);
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(999));
    let delta = rig.store.take_frame().expect("a delta");
    assert_eq!(delta.ops.len(), 1, "only the gold changed");
}

#[test]
fn the_parts_for_other_owners_are_handed_out_not_written() {
    let src = source(HUD);
    assert!(src.ui().is_none());
    assert_eq!(src.state().get("tab").and_then(|v| v.as_str()), Some("a"));
    assert_eq!(src.anchors().get("city:3"), Some(&(1320.0, 640.0)));
    assert_eq!(src.sample().roots().len(), 2);
}

#[test]
fn the_skin_sample_gives_what_the_sample_applies() {
    let sample = Sample::parse(Schema::v1(), SKIN_HUD).unwrap_or_else(|e| panic!("{e:?}"));

    let mut direct = ModelStore::new(Schema::v1());
    sample.apply(&mut direct).expect("apply");

    let mut rig = Rig::new();
    let mut src = StaticSource::new(sample);
    let stats = rig.frame(&mut src);
    assert!(src.errors().is_empty(), "{:?}", src.errors());
    assert!(stats.roots_built >= 3, "meta, game, top at least");

    assert_eq!(rig.store.get::<Meta>(), direct.get::<Meta>());
    assert_eq!(rig.store.get::<Game>(), direct.get::<Game>());
    assert_eq!(rig.store.get::<Top>(), direct.get::<Top>());
    assert_eq!(rig.store.get::<Research>(), direct.get::<Research>());
    assert!(rig.store.get::<Top>().is_some());
    assert!(rig.store.get::<Game>().is_some_and(|g| g.turn > 0));
}

#[test]
fn an_empty_sample_writes_nothing() {
    let mut rig = Rig::new();
    let mut src = source("{\"model\": {}}");
    assert_eq!(rig.frame(&mut src), SourceStats::default());
    assert!(rig.store.take_frame().is_none());
}
