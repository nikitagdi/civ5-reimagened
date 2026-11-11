//! The roots `meta`, `game`, `top`, `research` from a fake game, and the triggers: no change, no read and no
//! write. The UI v2 queries fill tourism, the strategic resources, the policy branches and the research queue; a
//! core without them leaves those at the schema's defaults.

use replica::testing::{FakeRead, catalog_entry};
use replica::{
    CatalogTable, GameStatus, Player, PlayerFlags, PlayerResource, PolicyBranchState, ReadError,
    ReplicaStep, ResourceUsage, State, TechState, TechStatus,
};
use ui2_model::r#gen::{
    EraRef, Game, GoldenAge, Meta, Mood, Phase, PolicyBranch, Research, Resource, Tech, Top,
    TurnMode, Ui,
};
use ui2_model::{AssetRef, Key, KeyedList, ModelStore, Owner, Schema, Text};
use ui2_source::session::testing::FakeSession;
use ui2_source::{ReplicaView, SourceFrame, SourceStats, UiSource};

const MEDIEVAL: i16 = 2;
const EDUCATION: i16 = 20;
const ASTRONOMY: i16 = 21;
const PHYSICS: i16 = 22;
const CHIVALRY: i16 = 23;
const GUILDS: i16 = 24;
const STEEL: i16 = 25;
const IRON: i16 = 3;
const HORSES: i16 = 4;
const WINE: i16 = 9;
const TRADITION: i16 = 0;
const LIBERTY: i16 = 1;

/// Catalog tables read once per core.
const CATALOG_TABLES: usize = 8;
/// The reads of a rebuild once the catalog is known: the top bar with its two lists, the game record, the
/// technologies, the units (turn), the notifications, the cities (map), the players. Shared records are read once.
const REBUILD: [&str; 9] = [
    "top_bar",
    "player_resources",
    "policy_branches",
    "game_info",
    "techs",
    "units",
    "notifications",
    "cities",
    "players",
];
/// The reads of a first frame: the catalog, then a rebuild.
fn first_frame() -> Vec<&'static str> {
    let mut v = vec!["catalog"; CATALOG_TABLES];
    v.extend(REBUILD);
    v
}
/// Roots of the replica's game written by a full rebuild: top, research, game, turn, unit (none selected), notes,
/// map, players, production (window closed).
const GAME_ROOTS: u32 = 9;
/// Roots of the session written on the first frame besides `meta`: chat.
const SESSION_ROOTS: u32 = 1;

fn tech_row(tech: i16, status: TechStatus, queue: Option<u8>, turns: Option<i16>) -> TechState {
    TechState {
        tech,
        status,
        queue,
        turns,
        progress: if queue == Some(0) { 310 } else { 0 },
        cost: 500,
    }
}

/// The game of the mockup's top bar: turn 87, 1250 AD, Medieval, researching Education, Astronomy next, then
/// Physics; Chivalry and Guilds could be started now, Steel is locked.
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
    t.tourism = Some(14);
    use TechStatus::{CanResearch, Have, Locked};
    f.techs = vec![
        tech_row(1, Have, None, None),
        tech_row(EDUCATION, CanResearch, Some(0), Some(3)),
        tech_row(ASTRONOMY, Locked, Some(1), Some(10)),
        tech_row(PHYSICS, Locked, Some(2), Some(11)),
        tech_row(CHIVALRY, CanResearch, None, Some(9)),
        tech_row(GUILDS, CanResearch, None, None),
        tech_row(STEEL, Locked, None, Some(12)),
    ];
    let res = |resource, usage, available, used| PlayerResource {
        resource,
        usage,
        available,
        used,
        total: available + used,
    };
    f.resources = vec![
        res(IRON, ResourceUsage::Strategic, 2, 4),
        res(HORSES, ResourceUsage::Strategic, 1, 0),
        res(WINE, ResourceUsage::Luxury, 1, 1),
    ];
    let branch = |branch, unlocked, owned, total| PolicyBranchState {
        branch,
        unlocked,
        owned,
        total,
    };
    f.branches = vec![branch(TRADITION, true, 3, 5), branch(LIBERTY, false, 0, 5)];
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
            vec![
                catalog_entry(EDUCATION, "TECH_EDUCATION", "Education", "TECH_ATLAS_1", 24),
                catalog_entry(ASTRONOMY, "TECH_ASTRONOMY", "Astronomy", "TECH_ATLAS_1", 25),
                catalog_entry(PHYSICS, "TECH_PHYSICS", "Physics", "TECH_ATLAS_1", 26),
                catalog_entry(CHIVALRY, "TECH_CHIVALRY", "Chivalry", "TECH_ATLAS_1", 27),
                catalog_entry(GUILDS, "TECH_GUILDS", "Guilds", "TECH_ATLAS_1", 28),
            ],
        ),
        (
            CatalogTable::Resources,
            vec![
                catalog_entry(IRON, "RESOURCE_IRON", "Iron", "RESOURCE_ATLAS", 5),
                catalog_entry(HORSES, "RESOURCE_HORSE", "Horses", "RESOURCE_ATLAS", 4),
                catalog_entry(WINE, "RESOURCE_WINE", "Wine", "RESOURCE_ATLAS", 9),
            ],
        ),
        (
            CatalogTable::PolicyBranches,
            vec![
                catalog_entry(TRADITION, "POLICY_BRANCH_TRADITION", "Tradition", "", -1),
                catalog_entry(LIBERTY, "POLICY_BRANCH_LIBERTY", "Liberty", "", -1),
            ],
        ),
    ];
    f
}

fn tech(id: i16, name: &str, icon: i16, turns: Option<i16>, progress: i32) -> Tech {
    Tech {
        id: Key::from_i32(i32::from(id)),
        name: Text::raw(name),
        icon: AssetRef::new(&format!("icon:TECH_ATLAS_1:{icon}")),
        turns,
        progress,
        cost: 500,
        fraction: if progress == 0 { 0.0 } else { 0.62 },
    }
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
    session: FakeSession,
    ui: Ui,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            store: ModelStore::new(Schema::v1()),
            src: UiSource::new(Schema::v1()),
            session: FakeSession::default(),
            ui: Ui::default(),
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
                session: &self.session,
                dismissed: &std::collections::BTreeSet::new(),
                now_ms: 0,
            },
            &mut w,
            &self.ui,
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
    assert_eq!(
        stats.roots_built,
        1 + SESSION_ROOTS + GAME_ROOTS,
        "meta, top, research, game, turn, players"
    );
    assert_eq!(stats.deferred, 0);
    assert_eq!(f.calls, first_frame());
    assert_eq!(stats.getter_calls, 17);

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
            tourism: Some(14),
            resources: KeyedList::from_vec(vec![
                Resource {
                    id: Key::from_i32(i32::from(IRON)),
                    name: Text::raw("Iron"),
                    icon: AssetRef::new("icon:RESOURCE_ATLAS:5"),
                    available: 2,
                    used: 4,
                    total: 6,
                },
                Resource {
                    id: Key::from_i32(i32::from(HORSES)),
                    name: Text::raw("Horses"),
                    icon: AssetRef::new("icon:RESOURCE_ATLAS:4"),
                    available: 1,
                    used: 0,
                    total: 1,
                },
            ])
            .unwrap(),
            policies: KeyedList::from_vec(vec![
                PolicyBranch {
                    id: TRADITION,
                    name: Text::raw("Tradition"),
                    unlocked: true,
                    owned: 3,
                    total: 5,
                },
                PolicyBranch {
                    id: LIBERTY,
                    name: Text::raw("Liberty"),
                    unlocked: false,
                    owned: 0,
                    total: 5,
                },
            ])
            .unwrap(),
        },
        "the luxury (Wine) is not on the top bar"
    );

    let research = rig.store.get::<Research>().expect("research is written");
    assert_eq!(
        research.current,
        Some(tech(EDUCATION, "Education", 24, Some(3), 310))
    );
    // Astronomy next, then Physics; the choices by turns, a technology without turns last.
    assert_eq!(
        research.next,
        Some(tech(ASTRONOMY, "Astronomy", 25, Some(10), 0))
    );
    assert_eq!(
        research.queue.iter().cloned().collect::<Vec<_>>(),
        vec![
            tech(ASTRONOMY, "Astronomy", 25, Some(10), 0),
            tech(PHYSICS, "Physics", 26, Some(11), 0),
        ]
    );
    let options: Vec<Key> = research.options.keys().collect();
    assert_eq!(
        options,
        [EDUCATION, CHIVALRY, GUILDS].map(|t| Key::from_i32(i32::from(t)))
    );
    assert!(research.known);
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
    assert_eq!(f.calls, REBUILD);
    assert_eq!(stats.roots_built, GAME_ROOTS);
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(333));
    // Only the gold changed: the delta is one op.
    let delta = rig.store.take_frame().expect("a delta");
    assert_eq!(delta.ops.len(), 1);

    // The jseq alone (a frame that missed the step) is a trigger too.
    f.calls.clear();
    rig.frame(Some(view(State::Live, 1, 2, &mut f)), &[]);
    assert_eq!(f.calls, REBUILD);
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
    assert_eq!(stats.deferred, GAME_ROOTS);
    assert!(!rig.store.get::<Meta>().is_some_and(|m| m.live));
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(312));

    // Catching up: the same.
    let mut v = view(State::Live, 1, 4, &mut f);
    v.catching_up = true;
    let stats = rig.frame(Some(v), &[]);
    assert!(f.calls.is_empty());
    assert_eq!(stats.deferred, GAME_ROOTS);
    assert!(
        rig.store
            .get::<Meta>()
            .is_some_and(|m| m.live && m.catching_up)
    );

    // Live again with no new signal: the deferred roots are built once.
    f.top.gold = 400;
    let stats = rig.frame(Some(view(State::Live, 1, 4, &mut f)), &[]);
    assert_eq!(f.calls, REBUILD);
    assert_eq!(stats.deferred, 0);
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(400));
    f.calls.clear();
    rig.frame(Some(view(State::Live, 1, 4, &mut f)), &[]);
    assert!(f.calls.is_empty());
}

#[test]
fn without_a_replica_only_the_session_roots_are_written() {
    let mut rig = Rig::new();
    let stats = rig.frame(None, &[]);
    assert_eq!(stats.roots_built, 1 + SESSION_ROOTS, "meta and chat");
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
    assert_eq!(
        stats.roots_built,
        3 + SESSION_ROOTS,
        "meta, chat, unit and production (nothing selected, no window: no read)"
    );
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
    assert_eq!(game.winner, None);

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
    assert_eq!(stats.roots_built, 1 + SESSION_ROOTS + GAME_ROOTS);
    assert_eq!(f.calls, first_frame());
}

#[test]
fn a_new_generation_rebuilds_everything() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 5, &mut f)), &[]);
    f.calls.clear();
    // A snapshot load restarts jseq numbering; the generation tells.
    rig.frame(Some(view(State::Live, 2, 5, &mut f)), &[]);
    assert_eq!(f.calls, REBUILD);
}

#[test]
fn a_core_without_the_ui2_queries_leaves_their_fields_empty_and_is_not_asked_again() {
    let mut f = mockup();
    f.ui2_error = Some(ReadError::Unsupported);
    f.top.tourism = None;
    let mut rig = Rig::new();
    let stats = rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    assert_eq!(
        stats.roots_built,
        1 + SESSION_ROOTS + GAME_ROOTS,
        "the roots are written all the same"
    );
    let top = rig.store.get::<Top>().expect("top");
    assert_eq!((top.gold, top.tourism), (312, None));
    assert!(top.resources.is_empty() && top.policies.is_empty());
    let research = rig.store.get::<Research>().expect("research");
    assert!(research.current.is_some(), "the top bar still names it");
    assert_eq!(research.next, None);
    assert!(research.queue.is_empty() && research.options.is_empty());
    assert!(!research.known);

    // The next batch does not ask the missing queries again.
    f.calls.clear();
    f.top.gold = 320;
    rig.frame(Some(view(State::Live, 1, 1, &mut f)), &[]);
    assert_eq!(
        f.calls,
        vec![
            "top_bar",
            "game_info",
            "units",
            "notifications",
            "cities",
            "players"
        ]
    );
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(320));

    // A new core (reset) is asked again; this one serves them.
    f.ui2_error = None;
    rig.src.reset();
    f.calls.clear();
    rig.frame(Some(view(State::Live, 1, 1, &mut f)), &[]);
    assert_eq!(f.calls, first_frame());
    assert!(rig.store.get::<Research>().is_some_and(|r| r.known));
    assert_eq!(rig.store.get::<Top>().map(|t| t.policies.len()), Some(2));
}

#[test]
fn a_failing_ui2_query_keeps_the_last_root() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    rig.store.take_frame();
    // The core fails (not "unsupported"): the root keeps its last value and is asked again on the next trigger.
    f.ui2_error = Some(ReadError::Core("broken reply".into()));
    f.top.gold = 999;
    f.calls.clear();
    let stats = rig.frame(Some(view(State::Live, 1, 1, &mut f)), &[]);
    assert_eq!(
        stats.roots_built,
        GAME_ROOTS - 2,
        "all but top and research"
    );
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(312));
    assert!(rig.store.get::<Research>().is_some_and(|r| r.known));
    f.ui2_error = None;
    f.calls.clear();
    rig.frame(Some(view(State::Live, 1, 2, &mut f)), &[]);
    assert_eq!(f.calls, REBUILD);
    assert_eq!(rig.store.get::<Top>().map(|t| t.gold), Some(999));
}

#[test]
fn the_research_queue_follows_a_batch() {
    let mut f = mockup();
    let mut rig = Rig::new();
    rig.frame(Some(view(State::Live, 1, 0, &mut f)), &[]);
    rig.store.take_frame();
    // Education is done: Astronomy is researched, Physics next.
    for t in &mut f.techs {
        t.queue = match t.tech {
            EDUCATION => {
                t.status = TechStatus::Have;
                t.turns = None;
                None
            }
            ASTRONOMY => Some(0),
            PHYSICS => Some(1),
            _ => t.queue,
        };
    }
    f.top.research_tech = Some(ASTRONOMY);
    rig.frame(
        Some(view(State::Live, 1, 1, &mut f)),
        &[ReplicaStep::Applied { jseq: 1 }],
    );
    let r = rig.store.get::<Research>().expect("research");
    assert_eq!(
        r.current.as_ref().map(|t| t.id),
        Some(Key::from_i32(i32::from(ASTRONOMY)))
    );
    assert_eq!(
        r.next.as_ref().map(|t| t.id),
        Some(Key::from_i32(i32::from(PHYSICS)))
    );
    assert_eq!(r.queue.len(), 1);
    assert_eq!(
        r.options.keys().collect::<Vec<_>>(),
        [CHIVALRY, GUILDS].map(|t| Key::from_i32(i32::from(t)))
    );
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
