//! The game of the reference HUD for the root tests: four humans (Catherine of Russia is the seat), turn 87, a
//! running turn timer, a few units. Each test file takes what it needs.
#![allow(dead_code)]

use std::collections::BTreeSet;

use protocol::{Conn, SeatControl, SeatTurn, SeatView, TimerState, TurnMode, TurnPhase, TurnState};
use replica::testing::{FakeRead, catalog_entry};
use replica::{
    Activity, CatalogTable, City, CityFlags, Notification, OrderType, Player, PlayerFlags,
    ReplicaStep, State, Unit, UnitFlags,
};
use ui2_model::r#gen::Ui;
use ui2_model::{FrameDelta, Key, ModelStore, Owner, Schema};
use ui2_source::session::testing::FakeSession;
use ui2_source::{ReplicaView, SourceFrame, SourceStats, UiSource};

// `GC_PLAYER_F_*`
pub const ALIVE: u16 = 0x0001;
pub const HUMAN: u16 = 0x0002;
pub const MINOR: u16 = 0x0004;
pub const BARBARIAN: u16 = 0x0008;
pub const MET: u16 = 0x0010;
pub const AT_WAR: u16 = 0x0020;
// `GC_UNIT_F_*`
pub const U_AUTOMATED: u16 = 0x0010;
pub const U_CAN_PROMOTE: u16 = 0x0080;
pub const U_CARGO: u16 = 0x0200;

pub const CATHERINE: i8 = 0;
pub const HARUN: i8 = 1;
pub const MONTEZUMA: i8 = 2;
pub const RAMESSES: i8 = 3;
pub const GENEVA: i8 = 22;
pub const BARBARIANS: i8 = 63;

pub const KNIGHT: i16 = 10;
pub const WORKER: i16 = 2;
pub const LONGSWORDSMAN: i16 = 11;
pub const JAGUAR: i16 = 30;

/// The id of the Knight "Bogatyr", the selected unit of the reference HUD.
pub const BOGATYR: i32 = 1;

/// The time of the first frame of a test, ms.
pub const T0: u64 = 1_000_000;

pub fn player(id: i8, flags: u16, name: &str, civ: &str, nick: Option<&str>) -> Player {
    Player {
        id,
        team: id,
        civ_type: Some(i16::from(id)),
        leader_type: Some(i16::from(id)),
        flags: PlayerFlags(flags),
        color_primary: 0x8b00_00ff_u32.wrapping_add(u32::from(id.cast_unsigned()) << 8),
        color_secondary: 0xffd7_00ff,
        name: name.into(),
        civ_name: civ.into(),
        era: Some(2),
        nick: nick.map(Into::into),
        slot: 1,
        art_techs: 0,
    }
}

pub fn players() -> Vec<Player> {
    vec![
        player(
            CATHERINE,
            ALIVE | HUMAN | MET,
            "Catherine",
            "Russia",
            Some("anna"),
        ),
        player(
            HARUN,
            ALIVE | HUMAN | MET,
            "Harun al-Rashid",
            "Arabia",
            Some("bob"),
        ),
        player(
            MONTEZUMA,
            ALIVE | HUMAN | MET | AT_WAR,
            "Montezuma",
            "Aztec",
            Some("carl"),
        ),
        player(
            RAMESSES,
            ALIVE | HUMAN | MET,
            "Ramesses II",
            "Egypt",
            Some("dora"),
        ),
        player(GENEVA, ALIVE | MINOR | MET, "Geneva", "Geneva", None),
        player(
            BARBARIANS,
            ALIVE | BARBARIAN,
            "Barbarians",
            "Barbarians",
            None,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
pub fn unit(
    id: i32,
    owner: i8,
    unit_type: i16,
    name: &str,
    moves: i16,
    activity: Activity,
) -> Unit {
    Unit {
        id,
        owner,
        unit_type,
        x: 10 + i16::try_from(id).unwrap_or(0),
        y: 8,
        hp: 100,
        max_hp: 100,
        moves_left: moves,
        max_moves: 120,
        strength: 8,
        ranged: 0,
        range: 0,
        domain: 2,
        activity,
        level: 1,
        experience: 0,
        flags: UnitFlags::default(),
        transport_id: None,
        build_type: None,
        facing: None,
        name: name.into(),
    }
}

/// Five units of Catherine wait for orders (the Knight among them); a fortified one, an automated worker, one
/// without moves, and a foreign unit do not count.
pub fn units() -> Vec<Unit> {
    let mut knight = unit(BOGATYR, CATHERINE, KNIGHT, "Bogatyr", 120, Activity::Awake);
    knight.max_moves = 240;
    knight.strength = 20;
    knight.flags = UnitFlags(U_CAN_PROMOTE);
    knight.level = 2;
    knight.experience = 15;
    let mut worker = unit(7, CATHERINE, WORKER, "Worker", 120, Activity::Awake);
    worker.flags = UnitFlags(U_AUTOMATED);
    worker.strength = 0;
    vec![
        knight,
        unit(
            2,
            CATHERINE,
            LONGSWORDSMAN,
            "Longswordsman",
            120,
            Activity::Awake,
        ),
        unit(
            3,
            CATHERINE,
            LONGSWORDSMAN,
            "Longswordsman",
            60,
            Activity::Awake,
        ),
        unit(
            4,
            CATHERINE,
            LONGSWORDSMAN,
            "Longswordsman",
            120,
            Activity::Awake,
        ),
        unit(
            5,
            CATHERINE,
            LONGSWORDSMAN,
            "Longswordsman",
            120,
            Activity::Awake,
        ),
        unit(
            6,
            CATHERINE,
            LONGSWORDSMAN,
            "Longswordsman",
            120,
            Activity::Sleep,
        ),
        worker,
        unit(
            8,
            CATHERINE,
            LONGSWORDSMAN,
            "Longswordsman",
            0,
            Activity::Awake,
        ),
        unit(1, MONTEZUMA, JAGUAR, "Jaguar", 120, Activity::Awake),
    ]
}

// `GC_CITY_F_*`
pub const C_CAPITAL: u16 = 0x0001;
pub const C_CAN_SHOOT: u16 = 0x0010;
pub const C_VISIBLE: u16 = 0x0020;

pub const MOSCOW: i32 = 1;
pub const NOVGOROD: i32 = 2;
pub const KAZAN: i32 = 3;
pub const BAGHDAD: i32 = 1;
pub const TENOCHTITLAN: i32 = 1;

// `BuildingTypes`
pub const GRANARY: i16 = 3;
pub const UNIVERSITY: i16 = 20;
pub const MACHU_PICCHU: i16 = 40;

#[allow(clippy::too_many_arguments)]
pub fn city(id: i32, owner: i8, name: &str, size: i16, x: i16, y: i16, flags: u16) -> City {
    City {
        id,
        owner,
        production_order: None,
        x,
        y,
        population: size,
        flags: CityFlags(flags),
        hp: 200,
        max_hp: 200,
        defense: 20,
        production_item: None,
        production_turns: None,
        growth_turns: None,
        production_stored: 0,
        production_needed: 0,
        yields: [10, 20, 5, 8, 4, 2],
        founder: None,
        overflow: None,
        name: name.into(),
    }
}

/// Catherine's three cities as the banners show them, a foreign city in sight and one in the fog.
pub fn cities() -> Vec<City> {
    let producing = |mut c: City, order, item, turns, stored, needed| {
        c.production_order = Some(order);
        c.production_item = Some(item);
        c.production_turns = Some(turns);
        c.production_stored = stored;
        c.production_needed = needed;
        c
    };
    let mut moscow = producing(
        city(
            MOSCOW,
            CATHERINE,
            "Moscow",
            14,
            40,
            20,
            C_CAPITAL | C_VISIBLE | C_CAN_SHOOT,
        ),
        OrderType::Construct,
        MACHU_PICCHU,
        6,
        180,
        420,
    );
    moscow.growth_turns = Some(4);
    moscow.defense = 31;
    moscow.hp = 150;
    moscow.yields = [24, 38, 21, 30, 9, 4];
    moscow.overflow = Some(12);
    let mut novgorod = producing(
        city(NOVGOROD, CATHERINE, "Novgorod", 8, 20, 12, C_VISIBLE),
        OrderType::Train,
        LONGSWORDSMAN,
        2,
        90,
        120,
    );
    novgorod.growth_turns = Some(7);
    let kazan = producing(
        city(KAZAN, CATHERINE, "Kazan", 5, 30, 25, C_VISIBLE),
        OrderType::Construct,
        GRANARY,
        3,
        20,
        60,
    );
    // A fogged foreign city: whatever else the record holds is not shown.
    let mut tenochtitlan = city(
        TENOCHTITLAN,
        MONTEZUMA,
        "Tenochtitlan",
        9,
        60,
        30,
        C_CAPITAL,
    );
    tenochtitlan.defense = 25;
    vec![
        moscow,
        novgorod,
        kazan,
        city(BAGHDAD, HARUN, "Baghdad", 11, 50, 10, C_CAPITAL | C_VISIBLE),
        tenochtitlan,
    ]
}

/// The catalog tables the roots name things with.
pub fn catalogs() -> Vec<(CatalogTable, Vec<replica::CatalogEntry>)> {
    vec![
        (
            CatalogTable::Units,
            vec![
                catalog_entry(WORKER, "UNIT_WORKER", "Worker", "UNIT_ATLAS_1", 1),
                catalog_entry(KNIGHT, "UNIT_KNIGHT", "Knight", "UNIT_ATLAS_1", 25),
                catalog_entry(
                    LONGSWORDSMAN,
                    "UNIT_LONGSWORDSMAN",
                    "Longswordsman",
                    "UNIT_ATLAS_1",
                    26,
                ),
                catalog_entry(JAGUAR, "UNIT_AZTEC_JAGUAR", "Jaguar", "UNIT_ATLAS_2", 3),
            ],
        ),
        (
            CatalogTable::Buildings,
            vec![
                catalog_entry(GRANARY, "BUILDING_GRANARY", "Granary", "BW_ATLAS_1", 3),
                catalog_entry(
                    UNIVERSITY,
                    "BUILDING_UNIVERSITY",
                    "University",
                    "BW_ATLAS_1",
                    20,
                ),
                catalog_entry(
                    MACHU_PICCHU,
                    "BUILDING_MACHU_PICHU",
                    "Machu Picchu",
                    "BW_ATLAS_2",
                    4,
                ),
            ],
        ),
    ]
}

/// The hash the core gives a notification type name.
pub fn note_type(name: &str) -> u32 {
    ui2_source::build::notes::type_hash(name)
}

pub fn note(id: i32, kind: &str, message: &str, plot: (i16, i16)) -> Notification {
    Notification {
        id,
        kind: note_type(kind),
        x: plot.0,
        y: plot.1,
        data1: 0,
        data2: 0,
        turn: 86,
        dismissed: false,
        message: message.into(),
        summary: message.into(),
    }
}

/// The four notifications of the reference HUD, one the core already dismissed and one of a type the table does
/// not know.
pub fn notes() -> Vec<Notification> {
    let mut old = note(1, "NOTIFICATION_TECH", "Philosophy learned", (-1, -1));
    old.dismissed = true;
    vec![
        old,
        note(
            2,
            "NOTIFICATION_CITY_GROWTH",
            "Novgorod has grown to size 8",
            (20, 12),
        ),
        note(
            3,
            "NOTIFICATION_GREAT_PERSON_ACTIVE_PLAYER",
            "Great Scientist born in Moscow",
            (40, 20),
        ),
        note(
            4,
            "NOTIFICATION_DIPLOMACY_DECLARATION",
            "Montezuma denounced you",
            (-1, -1),
        ),
        note(
            5,
            "NOTIFICATION_BARBARIAN",
            "Barbarians spotted near Kazan",
            (31, 26),
        ),
        note(6, "NOTIFICATION_SOMETHING_NEW", "A mod speaks", (-1, -1)),
    ]
}

/// The game at turn 87 seen by Catherine: the players, units, cities and notifications above, no blocker.
pub fn game() -> FakeRead {
    let mut f = FakeRead::new(CATHERINE.cast_unsigned(), 0);
    f.info.turn = 87;
    f.info.year = 1250;
    f.info.humans = 4;
    f.info.humans_left = 1;
    f.players = players();
    f.units = units();
    f.cities = cities();
    f.notes = notes();
    f.catalogs = catalogs();
    f
}

/// The timer of the turn: 42.9 s left at [`T0`], two minutes long.
pub fn timer() -> TimerState {
    TimerState {
        group: 0,
        deadline_ms: T0 + 42_900,
        len_ms: 120_000,
        left_ms_when_stopped: None,
    }
}

/// The session of the reference HUD: Catherine, Harun and Montezuma ended the turn, Ramesses still moves.
pub fn session() -> FakeSession {
    let seat = |player, finished| SeatTurn {
        player,
        finished,
        may_act: true,
        group: 0,
    };
    let view = |player: i8, nick: &str| SeatView {
        player,
        control: SeatControl::Human { nick: nick.into() },
        conn: Conn::Online,
        host: player == CATHERINE,
    };
    FakeSession {
        connected: true,
        me: Some(CATHERINE),
        turn: Some(TurnState {
            turn: 87,
            phase: TurnPhase::Humans,
            mode: TurnMode::Simultaneous,
            seats: vec![
                seat(CATHERINE, true),
                seat(HARUN, true),
                seat(MONTEZUMA, true),
                seat(RAMESSES, false),
            ],
            timers: vec![timer()],
            paused: false,
        }),
        seats: vec![
            view(CATHERINE, "anna"),
            view(HARUN, "bob"),
            view(MONTEZUMA, "carl"),
            view(RAMESSES, "dora"),
        ],
        chat: Vec::new(),
        chat_total: 0,
    }
}

/// A model store, the source, the session and the UI's own root, driven frame by frame.
pub struct Rig {
    pub store: ModelStore,
    pub src: UiSource,
    pub session: FakeSession,
    pub ui: Ui,
    /// Notifications dismissed here (the actions module's set).
    pub dismissed: BTreeSet<Key>,
    pub now_ms: u64,
    /// The replica's generation and last batch as the next frame shows them.
    pub generation: u64,
    pub jseq: u64,
    /// The turn the replica's journal reached, as the next frame shows it (ReplicaView::turn).
    pub journal_turn: Option<i32>,
}

impl Rig {
    pub fn new(session: FakeSession) -> Rig {
        // The tests look at roots, not at frames: one frame builds everything unless a test sets a budget itself.
        let mut src = UiSource::new(Schema::v1());
        src.set_read_budget(u32::MAX);
        Rig {
            store: ModelStore::new(Schema::v1()),
            src,
            session,
            ui: Ui::default(),
            dismissed: BTreeSet::new(),
            now_ms: T0,
            generation: 1,
            jseq: 0,
            journal_turn: None,
        }
    }

    /// One frame over a live replica.
    pub fn frame(&mut self, read: &mut FakeRead) -> SourceStats {
        self.frame_in(State::Live, read, &[])
    }

    /// One frame after a journal batch.
    pub fn batch(&mut self, read: &mut FakeRead) -> SourceStats {
        self.jseq += 1;
        let steps = [ReplicaStep::Applied { jseq: self.jseq }];
        self.frame_in(State::Live, read, &steps)
    }

    pub fn frame_in(
        &mut self,
        state: State,
        read: &mut FakeRead,
        steps: &[ReplicaStep],
    ) -> SourceStats {
        let view = ReplicaView {
            state,
            catching_up: false,
            generation: self.generation,
            applied_jseq: self.jseq,
            turn: self.journal_turn,
            read: Some(read),
        };
        let mut w = self.store.writer(Owner::Source);
        self.src.frame(
            SourceFrame {
                replica: Some(view),
                steps,
                session: &self.session,
                dismissed: &self.dismissed,
                now_ms: self.now_ms,
            },
            &mut w,
            &self.ui,
        )
    }

    /// The frame's delta, if any.
    pub fn delta(&mut self) -> Option<FrameDelta> {
        self.store.take_frame()
    }

    /// A first frame, its delta taken and the reads forgotten.
    pub fn start(&mut self, read: &mut FakeRead) {
        self.frame(read);
        self.store.take_frame();
        read.calls.clear();
    }
}
