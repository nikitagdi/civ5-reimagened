//! The first-slice roots (`meta`, `game`, `top`, `research`) from a fake game, and the triggers: no change, no
//! read and no write.

use replica::testing::{FakeRead, catalog_entry};
use replica::{CatalogTable, GameStatus, Player, PlayerFlags, ReadError, ReplicaStep, State};
use ui2_model::r#gen::{EraRef, Game, GoldenAge, Meta, Mood, Phase, Research, Tech, Top, TurnMode};
use ui2_model::{AssetRef, Key, ModelStore, Owner, Schema, Text};
use ui2_source::{ReplicaView, SourceFrame, SourceStats, UiSource};

const MEDIEVAL: i16 = 2;
const EDUCATION: i16 = 20;

/// The game of the mockup's top bar: turn 87, 1250 AD, Medieval, researching Education.
fn mockup() -> FakeRead {
    let mut f = FakeRead::new(0, 0);
    f.info.turn = 87;
    f.info.year = 1250;
    let t = &mut f.top;
    t.science_per_turn = 48;
    t.gold = 312;
    t.gold_per_turn = 21;
    t.happiness = 7;
    t.golden_age_progress = 412;
    t.golden_age_threshold = 650;
    t.culture = 200;
    t.culture_per_turn = 31;
    t.next_policy_cost = 360;
    t.faith = 140;
    t.faith_per_turn = 9;
    t.era = MEDIEVAL;
    t.research_tech = Some(EDUCATION);
    t.research_turns = 3;
    t.research_progress = 310;
    t.research_cost = 500;
    f.catalogs = vec![
        (
            CatalogTable::Eras,
            vec![catalog_entry(
                MEDIEVAL,
                "ERA_MEDIEVAL",
                "Medieval",
                "ERA_ATLAS",
                2,
            )],
        ),
        (
            CatalogTable::Technologies,
            vec![catalog_entry(
                EDUCATION,
                "TECH_EDUCATION",
                "Education",
                "TECH_ATLAS_1",
                24,
            )],
        ),
    ];
    f
}

fn view<'a>(
    state: State,
    generation: u64,
    applied_jseq: u64,
    read: &'a mut FakeRead,
) -> ReplicaView<'a> {
    ReplicaView {
        state,
        catching_up: false,
        generation,
        applied_jseq,
        read: Some(read),
    }
}

struct Rig {
    store: ModelStore,
    src: UiSource,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            store: ModelStore::new(Schema::v1()),
            src: UiSource::new(Schema::v1()),
        }
    }

    fn frame<'a>(
        &mut self,
        replica: Option<ReplicaView<'a>>,
        steps: &'a [ReplicaStep],
    ) -> SourceStats {
        let mut w = self.store.writer(Owner::Source);
        self.src.frame(
            SourceFrame {
                replica,
                steps,
                now_ms: 0,
            },
            &mut w,
        )
    }
}

#[test]
fn the_mockup_top_bar() {
    let mut f = mockup();
    let mut rig = Rig::new();
    let stats = rig.frame(
        Some(view(State::Live, 1, 0, &mut f)),
        &[ReplicaStep::Loaded { turn: 87 }],
    );
    assert_eq!(stats.roots_built, 4, "meta, top, research, game");
    assert_eq!(stats.deferred, 0);
    // The catalog (two tables), one top bar and one game record: shared records are read once a frame.
    assert_eq!(f.calls, vec!["catalog", "catalog", "top_bar", "game_info"]);
    assert_eq!(stats.getter_calls, 4);

    let top = rig.store.get::<Top>().expect("top is written");
    assert_eq!(
        *top,
        Top {
            science_rate: 48,
            gold: 312,
            gold_rate: 21,
            happiness: 7,
            mood: Mood::Content,
            golden_age: GoldenAge {
                active: false,
                turns: 0,
                progress: 412,
                threshold: 650,
            },
            culture: 200,
            culture_rate: 31,
            policy_cost: 360,
            policy_turns: Some(6),
            faith: 140,
            faith_rate: 9,
            tourism: None,
            resources: Default::default(),
            policies: Default::default(),
        }
    );

    let research = rig.store.get::<Research>().expect("research is written");
    assert_eq!(
        research.current,
        Some(Tech {
            id: Key::from_i32(i32::from(EDUCATION)),
            name: Text::raw("Education"),
            icon: AssetRef::new("icon:TECH_ATLAS_1:24"),
            turns: Some(3),
            progress: 310,
            cost: 500,
            fraction: 0.62,
        })
    );
    assert_eq!(research.next, None);
    assert!(research.queue.is_empty() && research.options.is_empty());
    assert!(
        !research.known,
        "the queue needs a query the core does not serve yet"
    );
    assert!(!research.blocked);

    let game = rig.store.get::<Game>().expect("game is written");
    assert_eq!(
        *game,
        Game {
            turn: 87,
            year: 1250,
            year_text: Text::key("year.ad", &["1250"]),
            era: EraRef {
                id: MEDIEVAL,
                name: Text::raw("Medieval"),
            },
            me: Key::from_i32(0),
            phase: Phase::Humans,
            mode: TurnMode::Simultaneous,
            winner: None,
        }
    );

    let meta = rig.store.get::<Meta>().expect("meta is written");
    assert!(meta.live && !meta.catching_up);
    assert_eq!(&*meta.schema, "1.0");
    assert!(rig.store.take_frame().is_some());
}

#[test]
fn no_trigger_no_read_no_write() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    assert!(rig.store.take_frame().is_some());
    let calls = f.call_count();

    for _ in 0..100 {
        let stats = rig.frame(
            Some(view(State::Live, 1, 0, &mut f)),
            &[ReplicaStep::Idle, ReplicaStep::Working],
        );
        assert_eq!(stats, SourceStats::default());
    }
    assert_eq!(f.call_count(), calls, "no read of the game");
    assert!(rig.store.take_frame().is_none(), "no write to the model");
}

#[test]
fn an_applied_batch_rebuilds_without_the_catalog() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    rig.store.take_frame();
    f.calls.clear();

    f.top.gold = 333;
    let stats = rig.frame(
        Some(view(State::Live, 1, 1, &mut f)),
        &[ReplicaStep::Applied { jseq: 1 }],
    );
    assert_eq!(f.calls, vec!["top_bar", "game_info"]);
    assert_eq!(stats.roots_built, 3);
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(333));
    // Only the gold changed: the delta is one op.
    let delta = rig.store.take_frame().expect("a delta");
    assert_eq!(delta.ops.len(), 1);

    // The jseq alone (a frame that missed the step) is a trigger too.
    f.calls.clear();
    rig.frame(Some(view(State::Live, 1, 2, &mut f)), &[]);
    assert_eq!(f.calls, vec!["top_bar", "game_info"]);
}

#[test]
fn a_replica_that_cannot_be_read_defers_and_marks_meta() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    rig.store.take_frame();
    f.calls.clear();

    // Resyncing: nothing is read, the model keeps its roots, `meta.live` goes false.
    let stats = rig.frame(
        Some(view(State::Resyncing, 1, 3, &mut f)),
        &[ReplicaStep::Resyncing, ReplicaStep::Applied { jseq: 3 }],
    );
    assert!(f.calls.is_empty());
    assert_eq!(stats.roots_built, 1, "meta only");
    assert_eq!(stats.deferred, 3);
    assert!(!rig.store.get::<Meta>().is_some_and(|m| m.live));
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(312));

    // Catching up: the same.
    let mut v = view(State::Live, 1, 4, &mut f);
    v.catching_up = true;
    let stats = rig.frame(Some(v), &[]);
    assert!(f.calls.is_empty());
    assert_eq!(stats.deferred, 3);
    assert!(
        rig.store
            .get::<Meta>()
            .is_some_and(|m| m.live && m.catching_up)
    );

    // Live again with no new signal: the deferred roots are built once.
    f.top.gold = 400;
    let stats = rig.frame(Some(view(State::Live, 1, 4, &mut f)), &[]);
    assert_eq!(f.calls, vec!["top_bar", "game_info"]);
    assert_eq!(stats.deferred, 0);
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(400));
    f.calls.clear();
    rig.frame(Some(view(State::Live, 1, 4, &mut f)), &[]);
    assert!(f.calls.is_empty());
}

#[test]
fn without_a_replica_only_meta_is_written() {
    let mut rig = Rig::new();
    let stats = rig.frame(None, &[]);
    assert_eq!(stats.roots_built, 1);
    let meta = rig.store.get::<Meta>().expect("meta");
    assert!(!meta.live);
    assert!(rig.store.get::<Top>().is_none());
    assert_eq!(rig.frame(None, &[]), SourceStats::default());
}

#[test]
fn a_core_without_queries_leaves_the_roots_unwritten() {
    let mut f = mockup();
    f.error = Some(ReadError::Unsupported);
    let mut rig = Rig::new();
    let stats = rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    assert_eq!(stats.roots_built, 1, "meta only");
    assert!(rig.store.get::<Top>().is_none());
    assert!(rig.store.get::<Game>().is_none());
    // Failing reads are not retried every frame.
    let n = f.call_count();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    assert_eq!(f.call_count(), n);
}

#[test]
fn research_blocker_ai_phase_and_the_winner() {
    let mut f = mockup();
    f.info.blocker = 1;
    f.info.status = GameStatus::AiRunning;
    f.info.turn_mode = Some(replica::TurnMode::Hybrid);
    f.info.year = -2000;
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    assert!(rig.store.get::<Research>().is_some_and(|r| r.blocked));
    let game = rig.store.get::<Game>().expect("game");
    assert_eq!(game.phase, Phase::Ai);
    assert_eq!(game.mode, TurnMode::Hybrid);
    assert_eq!(game.year_text, Text::key("year.bc", &["2000"]));
    assert_eq!(game.winner, None);
    assert!(!f.calls.contains(&"players"));

    f.info.status = GameStatus::GameOver;
    f.info.winner = 3;
    f.players = vec![player(1, 1), player(4, 3)];
    rig.frame(Some(view(State::Live, 1, 1, &mut f)), &[]);
    let game = rig.store.get::<Game>().expect("game");
    assert_eq!(game.phase, Phase::GameOver);
    assert_eq!(game.winner, Some(Key::from_i32(4)));
}

#[test]
fn reset_reads_everything_again() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    f.calls.clear();
    rig.src.reset();
    let stats = rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    assert_eq!(stats.roots_built, 4);
    assert_eq!(f.calls, vec!["catalog", "catalog", "top_bar", "game_info"]);
}

#[test]
fn a_new_generation_rebuilds_everything() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 5, &mut f)), &[]);
    f.calls.clear();
    // A snapshot load restarts jseq numbering; the generation tells.
    rig.frame(Some(view(State::Live, 2, 5, &mut f)), &[]);
    assert_eq!(f.calls, vec!["top_bar", "game_info"]);
}

fn player(id: i8, team: i8) -> Player {
    Player {
        id,
        team,
        civ_type: None,
        leader_type: None,
        flags: PlayerFlags::default(),
        color_primary: 0,
        color_secondary: 0,
        name: String::new(),
        civ_name: String::new(),
        era: None,
        nick: None,
        slot: -1,
        art_techs: 0,
    }
}
