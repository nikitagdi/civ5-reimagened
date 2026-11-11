//! The game of the reference HUD for the root tests: four humans (Catherine of Russia is the seat), turn 87, a
//! running turn timer, a few units. Each test file takes what it needs.
#![allow(dead_code)]

use protocol::{Conn, SeatControl, SeatTurn, SeatView, TimerState, TurnMode, TurnPhase, TurnState};
use replica::testing::FakeRead;
use replica::{Activity, Player, PlayerFlags, ReplicaStep, State, Unit, UnitFlags};
use ui2_model::r#gen::Ui;
use ui2_model::{FrameDelta, ModelStore, Owner, Schema};
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

/// The game at turn 87 seen by Catherine: the players and units above, no blocker.
pub fn game() -> FakeRead {
    let mut f = FakeRead::new(CATHERINE.cast_unsigned(), 0);
    f.info.turn = 87;
    f.info.year = 1250;
    f.info.humans = 4;
    f.info.humans_left = 1;
    f.players = players();
    f.units = units();
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
    pub now_ms: u64,
    /// The replica's generation and last batch as the next frame shows them.
    pub generation: u64,
    pub jseq: u64,
}

impl Rig {
    pub fn new(session: FakeSession) -> Rig {
        Rig {
            store: ModelStore::new(Schema::v1()),
            src: UiSource::new(Schema::v1()),
            session,
            ui: Ui::default(),
            now_ms: T0,
            generation: 1,
            jseq: 0,
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
            read: Some(read),
        };
        let mut w = self.store.writer(Owner::Source);
        self.src.frame(
            SourceFrame {
                replica: Some(view),
                steps,
                session: &self.session,
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
