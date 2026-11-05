//! Mock worker (GS-D9): speaks the gateway-worker IPC (net-protocol 4.14) without a game core, so
//! the supervisor, the seat pipelines and the policies are tested before `game-host-mp` exists.
//! Built as the `mock-worker` binary of the `server` package (cargo gives tests its path in
//! `CARGO_BIN_EXE_mock-worker`). Not part of the shipped product.
//!
//! `mock-worker <mode> [key=value ...]`. Modes:
//!
//! - `echo` (default): `Init` -> `Catalog`, `Ready`, then `Basis`, a `Full` view and `Turn` for every
//!   human seat. Every `Cmd` is answered with `Results { Applied }` and a `Delta` view whose
//!   `turn` grew by one; `WantFull` with a `Full`; `Query` with a `QueryResult`; `Seat` with
//!   `SeatDone`; `SaveNow` and `Shutdown { save }` with `Saved`. `Shutdown` and EOF exit 0.
//! - `crash-after n=N [code=C]`: exits with `C` (default 1) when the N-th frame after `Init`
//!   arrives, without answering it.
//! - `hang [at=start]`: after `Ready` (with `at=start`: before it) never writes or reads again.
//! - `garbage [n=N]`: answers the N-th frame after `Init` with a frame that is not a `WorkerFrame`
//!   and then stays alive.
//! - `slow ms=MS`: like `echo`, but every answer waits MS milliseconds.
//! - `exit code=C`: exits with `C` right after reading `Init`.
//! - `fatal code=C`: sends `Fatal { code: C }` and exits with `C` after reading `Init`.
//!
//! `crash-after n=0` exits right after the first views, without waiting for a frame. `once=PATH`
//! (for `crash-after`, `hang`, `garbage`, `exit`, `fatal`) misbehaves only in the first process
//! that creates PATH; later ones are plain `echo`. `bad=TEXT`: a `Load` of a file with TEXT in
//! its name exits with code 4. The name of a loaded save sets the turn and the next save number.
//!
//! `turns=1` (every mode that plays) makes turns real enough for the turn timer: `EndTurn`
//! finishes a seat, when every seat is finished the turn grows by one and nobody is finished;
//! `CancelEndTurn` takes it back; `DeclareWar { team }` is a war of the seat on player `team`
//! (reported once in `human_wars`); `ForceEndTurn` finishes the seats named. Every change sends a
//! `Turn`. Options: `len=S base=S` (`turn_len_s`, `timer_base_s`), `seq=P,Q` (those seats move one
//! after the other, in this order: hybrid), `force_log=PATH` (a line per `ForceEndTurn`),
//! `ignore_force=1` (`ForceEndTurn` changes nothing), `over=T` (the game is over from turn T). `claim=refuse|ignore`: a `Claim` is refused or
//! never answered. `kick=refuse|ignore [ignore=N]`: a `Kick` is refused, or the first N are not answered. A `Claim` of an AI slot gives it a seat with `Basis`, `Full` and `SeatDone`.
//!
//! `replica=1` (every mode that plays) makes the mock a server of the replica stream (protocol v4) with made-up
//! bodies: a `ReplicaBlob` and `ReplicaBase` (epoch 77, `jseq` 0) with the first views, a `ReplicaJournal` batch
//! `J<n>` for every `Cmd` but `EndTurn`, a `ReplicaTurn` (a new body, and with `patch=1` a patch body) for an
//! `EndTurn`, `Net` messages echoed as a batch (`reject...` is `NetRejected`), `ReplicaSnapshot` answered with a
//! `ReplicaBase` for that seat.
//!
//! Every mode exits with code 2 and a `Fatal` when the first frame is not an `Init` of this
//! `IPC_VERSION`. Options for every mode: `heartbeat=MS` writes a `Heartbeat` frame every MS
//! milliseconds (not in `hang`). A `Describe` start answers `Choices`, `Catalog` and `Ready` and exits 0 in every mode
//! except `exit`, `fatal` and `hang at=start`; `no-choices=1` leaves `Choices` out.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use protocol::{
    Cmd, Codec, Command, CommandOutcome, CommandResult, GameSettings, GcBlobRef, GcPatchRef,
    HumanWar, IPC_VERSION, LobbyChoices, MapBasis, Postcard, QueryAnswer, SaveKind, SaveMeta,
    SaveRecord, SeatChange, SeatTurnInfo, SlotControl, SlotSetup, Start, ToWorker, TurnInfo,
    TurnMode, TurnPhase, TurnStatus, ViewHash, ViewUpdate, WorkerFrame, WorkerMsg, WorkerReady,
    WorldView,
};

const BUILD_ID: &str = "mock-core";
/// The turn of the loaded save (0 for a new game); `Saved` reports it.
static TURN: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
const CATALOG: &[u8] = b"mock catalog";
/// What a save of this game says of it (`SETG`, `SEAT`, the game number): set at `Init`.
static GAME: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static SETTINGS: Mutex<Option<GameSettings>> = Mutex::new(None);
static SLOTS: Mutex<Vec<SlotSetup>> = Mutex::new(Vec::new());

type Out = Arc<Mutex<io::Stdout>>;

struct Opts {
    mode: String,
    kv: BTreeMap<String, String>,
}

impl Opts {
    fn parse() -> Self {
        let mut args = std::env::args().skip(1);
        let mode = args.next().unwrap_or_else(|| "echo".into());
        let kv = args
            .filter_map(|a| a.split_once('=').map(|(k, v)| (k.to_owned(), v.to_owned())))
            .collect();
        Self { mode, kv }
    }

    fn num(&self, key: &str, default: u64) -> u64 {
        self.kv
            .get(key)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    }
}

fn send(out: &Out, msgs: Vec<WorkerMsg>) {
    let body = Postcard.encode(&WorkerFrame { msgs }).expect("encode");
    write_raw(out, &body);
}

fn write_raw(out: &Out, body: &[u8]) {
    let mut o = out.lock().unwrap();
    // A write error means the gateway is gone: nothing left to do.
    if wire::ipc::write_frame(&mut *o, body).is_err() {
        std::process::exit(0);
    }
}

/// A well-formed, empty map basis (its hash checks out).
fn mock_basis() -> MapBasis {
    let mut m = MapBasis::default();
    m.basis_hash = protocol::basis_hash_of(&m);
    m
}

/// Pick lists of the mock world: one of each, four slots on the one map size.
fn choices() -> LobbyChoices {
    let c = |t: &str| protocol::LobbyChoice {
        type_name: t.into(),
        name: t.into(),
    };
    LobbyChoices {
        hash: 7,
        civs: ["A", "B", "C", "D"]
            .iter()
            .map(|n| protocol::CivChoice {
                id: format!("CIVILIZATION_{n}"),
                name: (*n).into(),
                leader: format!("Leader {n}"),
                traits: vec![format!("Trait of {n}: text")],
                unique: vec![format!("Unit: Unique {n}")],
            })
            .collect(),
        handicaps: vec![c("h")],
        world_sizes: vec![protocol::WorldSizeChoice {
            id: "WORLDSIZE_DUEL".into(),
            name: "Duel".into(),
            width: 8,
            height: 4,
            default_players: 2,
            max_players: 4,
            default_minor: 0,
            max_minor: 0,
        }],
        map_scripts: vec![c("m")],
        speeds: vec![c("s")],
        eras: vec![c("e")],
        timers: vec![c("FAST")],
        options: vec![],
        ..LobbyChoices::default()
    }
}

fn view(player: i8, turn: i32) -> WorldView {
    WorldView {
        turn,
        year: -4000,
        human: player,
        human_team: player,
        status: TurnStatus::HumanTurn,
        blocker: None,
        map: Default::default(),
        units: vec![],
        cities: vec![],
        players: vec![],
        top_bar: Default::default(),
        notifications: vec![],
        peace_offers: vec![],
        trade_routes: vec![],
    }
}

fn turn_info(humans: &[i8], turn: i32) -> TurnInfo {
    TurnInfo {
        turn,
        phase: TurnPhase::Humans,
        mode: TurnMode::Hybrid,
        seats: humans
            .iter()
            .map(|p| SeatTurnInfo {
                player: *p,
                finished: false,
                may_act: true,
                simultaneous: true,
            })
            .collect(),
        turn_len_s: 0,
        timer_base_s: 0,
        human_wars: vec![],
    }
}

fn humans_of(slots: &[SlotSetup]) -> Vec<i8> {
    slots
        .iter()
        .filter(|s| matches!(s.control, SlotControl::Human { .. }))
        .map(|s| s.player)
        .collect()
}

/// The turn of a `turns=1` mock.
struct Turns {
    turn: i32,
    finished: BTreeSet<i8>,
    /// Seats that move one after the other.
    seq: Vec<i8>,
    len: u32,
    base: u32,
    wars: Vec<HumanWar>,
    ignore_force: bool,
    /// From this turn on the phase is `GameOver` (0: never).
    over_turn: i32,
    force_log: Option<String>,
    /// What the last `Turn` said, without the wars.
    last: Option<TurnInfo>,
}

impl Turns {
    fn new(opts: &Opts) -> Self {
        let seq = opts
            .kv
            .get("seq")
            .map(|v| v.split(',').filter_map(|p| p.parse().ok()).collect())
            .unwrap_or_default();
        Self {
            turn: 1,
            finished: BTreeSet::new(),
            seq,
            len: opts.num("len", 0) as u32,
            base: opts.num("base", 0) as u32,
            wars: Vec::new(),
            ignore_force: opts.num("ignore_force", 0) == 1,
            over_turn: opts.num("over", 0) as i32,
            force_log: opts.kv.get("force_log").cloned(),
            last: None,
        }
    }

    fn info(&mut self, players: &[i8]) -> TurnInfo {
        // The first unfinished seat of `seq` that is here moves now.
        let acting = self
            .seq
            .iter()
            .find(|p| players.contains(p) && !self.finished.contains(p))
            .copied();
        TurnInfo {
            turn: self.turn,
            phase: if self.over_turn > 0 && self.turn >= self.over_turn {
                TurnPhase::GameOver { winner: None }
            } else {
                TurnPhase::Humans
            },
            mode: if self.seq.is_empty() {
                TurnMode::Simultaneous
            } else {
                TurnMode::Hybrid
            },
            seats: players
                .iter()
                .map(|p| {
                    let simultaneous = !self.seq.contains(p);
                    let finished = self.finished.contains(p);
                    SeatTurnInfo {
                        player: *p,
                        finished,
                        may_act: !finished && (simultaneous || acting == Some(*p)),
                        simultaneous,
                    }
                })
                .collect(),
            turn_len_s: self.len,
            timer_base_s: self.base,
            human_wars: std::mem::take(&mut self.wars),
        }
    }

    /// Everybody finished: the next turn.
    fn advance(&mut self, players: &[i8]) {
        if !players.is_empty() && players.iter().all(|p| self.finished.contains(p)) {
            self.turn += 1;
            self.finished.clear();
        }
    }

    /// A `Turn` if something changed since the last one (or wars came).
    fn changed(&mut self, players: &[i8]) -> Option<WorkerMsg> {
        let info = self.info(players);
        let mut stored = info.clone();
        stored.human_wars.clear();
        if self.last.as_ref() == Some(&stored) && info.human_wars.is_empty() {
            return None;
        }
        self.last = Some(stored);
        Some(WorkerMsg::Turn(info))
    }
}

struct Seat {
    rev: u32,
    view: WorldView,
    last_cmd: Option<u32>,
}

fn full(player: i8, seat: &Seat) -> WorkerMsg {
    WorkerMsg::View {
        player,
        update: ViewUpdate::Full {
            rev: seat.rev,
            view: seat.view.clone(),
            hash: ViewHash::of(&seat.view).value(),
        },
    }
}

/// The mock's replica stream (`replica=1`).
struct Replica {
    epoch: u32,
    jseq: u64,
    turn: i32,
    n: u64,
    patch: bool,
}

impl Replica {
    const EPOCH: u32 = 77;

    fn new(opts: &Opts) -> Self {
        Self {
            epoch: Self::EPOCH,
            jseq: 0,
            turn: 1,
            n: 0,
            patch: opts.num("patch", 0) == 1,
        }
    }

    /// A body the gateway keeps: made up, but with the right hash.
    fn blob(&mut self, what: &str) -> (WorkerMsg, GcBlobRef) {
        self.n += 1;
        let bytes = format!("mock {what} {} of turn {}", self.n, self.turn).into_bytes();
        let hash = protocol::xxh64(0, &bytes);
        let r = GcBlobRef {
            hash,
            size: bytes.len() as u32,
        };
        (WorkerMsg::ReplicaBlob { hash, bytes }, r)
    }

    fn base(&mut self, for_player: Option<i8>) -> Vec<WorkerMsg> {
        let (blob, snapshot) = self.blob("snapshot");
        vec![
            blob,
            WorkerMsg::ReplicaBase {
                epoch: self.epoch,
                turn: self.turn,
                jseq: self.jseq,
                raw_hash: 1000 + self.n,
                snapshot,
                for_player,
            },
        ]
    }

    fn batch(&mut self, bytes: Vec<u8>) -> WorkerMsg {
        self.jseq += 1;
        WorkerMsg::ReplicaJournal {
            epoch: self.epoch,
            first_jseq: self.jseq,
            batches: vec![bytes],
        }
    }

    fn next_turn(&mut self) -> Vec<WorkerMsg> {
        self.turn += 1;
        let ended = self.turn - 1;
        let from = 1000 + self.n;
        let (blob, snapshot) = self.blob("snapshot");
        // The AI progress of the phase that just ended (2 of 5), as the real worker sends it while the AI moves.
        let mut msgs = vec![
            // What the real worker sends with `record` on: the AI phase's journal, which the replica stream drops.
            WorkerMsg::RecJournal {
                phase: protocol::RecPhase::Ai,
                turn: ended,
                bytes: format!("ai-journal of turn {ended}").into_bytes(),
            },
            WorkerMsg::RecMark {
                turn: ended,
                what: "a step, Ai".into(),
                begin: true,
            },
            WorkerMsg::AiTurn {
                turn: ended,
                events: vec![
                    protocol::AiTurnEvent {
                        player: 3,
                        ended: false,
                    },
                    protocol::AiTurnEvent {
                        player: 3,
                        ended: true,
                    },
                ],
            },
            blob,
        ];
        let patch = self.patch.then(|| {
            let (b, r) = self.blob("patch");
            msgs.push(b);
            GcPatchRef {
                from_raw_hash: from,
                blob: r,
            }
        });
        msgs.push(WorkerMsg::ReplicaTurn {
            epoch: self.epoch,
            turn: self.turn,
            jseq: self.jseq,
            raw_hash: 1000 + self.n,
            snapshot,
            patch,
        });
        msgs.push(self.trace(ended));
        msgs
    }

    /// The worker's timeline of the end of `ended`: an END_TURN, 3 s of AI, the turn change and the pack.
    fn trace(&self, ended: i32) -> WorkerMsg {
        use turntrace::{Clock, Kind, MonoClock, Rec, Src, TurnTrace};
        let t0 = MonoClock::default().now_us();
        let rec = |kind, span: &str, at: u64, dur: u64, args: Vec<(String, i64)>| Rec {
            kind,
            src: Src::Worker,
            seat: -1,
            span: span.into(),
            lane: "core".into(),
            id: 1,
            parent: 0,
            t0_us: t0 + at,
            dur_us: dur,
            args,
        };
        let t = TurnTrace {
            game: 1,
            turn: ended,
            recs: vec![
                rec(Kind::Mark, "w.end_turn", 0, 0, vec![("cmd".into(), 1)]),
                rec(Kind::Span, "w.apply", 0, 2_000, vec![]),
                rec(
                    Kind::Span,
                    "w.ai",
                    10_000,
                    3_000_000,
                    vec![("pump_us".into(), 2_990_000), ("slices".into(), 80)],
                ),
                rec(Kind::Span, "w.turn_change", 3_010_000, 300_000, vec![]),
                rec(
                    Kind::Span,
                    "w.pack",
                    3_310_000,
                    80_000,
                    vec![("bytes".into(), 534_000)],
                ),
            ],
        };
        WorkerMsg::Trace {
            turn: ended,
            bytes: Postcard.encode(&t).expect("a trace encodes"),
        }
    }
}

fn main() -> ExitCode {
    let opts = Opts::parse();
    // stderr is for logs only (net-protocol 4.14).
    eprintln!("mock-worker started in mode {}", opts.mode);
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    let mut reader = wire::ipc::FrameReader::new(io::stdin().lock());

    // The first frame is `Init`.
    let init = match reader.next() {
        Ok(Some(body)) => match Postcard.decode::<ToWorker>(&body) {
            Ok(ToWorker::Init(init)) => init,
            _ => return fail(&out, "first frame is not Init", 2),
        },
        _ => return fail(&out, "no Init", 2),
    };
    if init.ipc != IPC_VERSION {
        return fail(&out, "foreign IPC_VERSION", 2);
    }
    match opts.mode.as_str() {
        "exit" if armed(&opts) => return ExitCode::from(opts.num("code", 1) as u8),
        "fatal" if armed(&opts) => return fail(&out, "mock fatal", opts.num("code", 1) as u8),
        "hang" if opts.kv.get("at").is_some_and(|v| v == "start") && armed(&opts) => hang(),
        _ => {}
    }

    let hb = opts.num("heartbeat", 0);
    if hb > 0 && opts.mode != "hang" {
        let out = out.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(hb));
                send(
                    &out,
                    vec![WorkerMsg::Heartbeat {
                        phase: TurnPhase::Humans,
                        queued: 0,
                    }],
                );
            }
        });
    }

    GAME.store(init.game, std::sync::atomic::Ordering::Relaxed);
    if let Start::New(setup) = &init.start {
        *SETTINGS.lock().unwrap() = Some(setup.settings.clone());
    }
    if let Start::Load { file, .. } = &init.start
        && let Ok(bytes) = std::fs::read(file)
        && let Ok(info) = game_host::saves::read_header(&bytes)
    {
        *SETTINGS.lock().unwrap() = Some(info.settings);
    }
    let slots = match &init.start {
        Start::New(setup) => setup.slots.clone(),
        Start::Load { seats, .. } => seats.clone(),
        Start::Describe => {
            let mut msgs = Vec::new();
            if opts.num("no-choices", 0) == 0 {
                msgs.push(WorkerMsg::Choices(choices()));
            }
            msgs.push(catalog());
            msgs.push(WorkerMsg::Ready(ready(vec![], 0)));
            send(&out, msgs);
            return ExitCode::SUCCESS;
        }
    };
    // A save name `ring/<seq:08>-t<turn:04>.c5sv` tells where the game stands.
    let (base_seq, base_turn) = match &init.start {
        Start::Load { file, .. } => {
            if opts
                .kv
                .get("bad")
                .is_some_and(|b| file.contains(b.as_str()))
            {
                return fail(&out, "the save does not load", 4);
            }
            parse_save(file)
        }
        _ => (0, 0),
    };
    TURN.store(base_turn, std::sync::atomic::Ordering::Relaxed);
    let humans = humans_of(&slots);
    let slow = if opts.mode == "slow" {
        Duration::from_millis(opts.num("ms", 100))
    } else {
        Duration::ZERO
    };

    *SLOTS.lock().unwrap() = slots.clone();
    send(
        &out,
        vec![catalog(), WorkerMsg::Ready(ready(slots.clone(), base_turn))],
    );
    if opts.mode == "hang" && armed(&opts) {
        hang();
    }
    if opts.mode == "crash-after" && opts.num("n", 1) == 0 && armed(&opts) {
        return ExitCode::from(opts.num("code", 1) as u8);
    }
    let all_players: BTreeSet<i8> = slots.iter().map(|s| s.player).collect();
    let mut turns = (opts.num("turns", 0) == 1).then(|| Turns::new(&opts));
    let mut seats: BTreeMap<i8, Seat> = humans
        .iter()
        .map(|p| {
            let seat = Seat {
                rev: 1,
                view: view(*p, 1),
                last_cmd: None,
            };
            (*p, seat)
        })
        .collect();
    let mut replica = (opts.num("replica", 0) == 1).then(|| Replica::new(&opts));
    let mut first = vec![WorkerMsg::Basis {
        players: humans.clone(),
        basis: mock_basis(),
    }];
    first.extend(seats.iter().map(|(p, s)| full(*p, s)));
    first.push(match &mut turns {
        Some(t) => t.changed(&humans).expect("the first Turn"),
        None => WorkerMsg::Turn(turn_info(&humans, 1)),
    });
    if let Some(r) = &mut replica {
        first.extend(r.base(None));
    }
    send(&out, first);

    let mut frames = 0u64;
    let mut kicks_ignored = 0u64;
    let mut seq = base_seq;
    loop {
        let body = match reader.next() {
            Ok(Some(body)) => body,
            // EOF on stdin is `Shutdown { save: true }`.
            Ok(None) => return shutdown(&out, &seats, &mut seq, true),
            Err(_) => return fail(&out, "bad frame from the gateway", 2),
        };
        frames += 1;
        let Ok(msg) = Postcard.decode::<ToWorker>(&body) else {
            return fail(&out, "undecodable frame from the gateway", 2);
        };
        match opts.mode.as_str() {
            "crash-after" if frames >= opts.num("n", 1) && armed(&opts) => {
                return ExitCode::from(opts.num("code", 1) as u8);
            }
            "garbage" if frames == opts.num("n", 1) && armed(&opts) => {
                write_raw(&out, &[0xff; 9]);
                hang();
            }
            _ => {}
        }
        std::thread::sleep(slow);
        match msg {
            ToWorker::Init(_) => return fail(&out, "second Init", 2),
            ToWorker::Cmd { player, cmd } => {
                if let Some(r) = &mut replica
                    && seats.contains_key(&player)
                {
                    let msgs = if matches!(cmd.cmd, Command::EndTurn { .. }) {
                        r.next_turn()
                    } else {
                        vec![r.batch(format!("J{}", r.jseq + 1).into_bytes())]
                    };
                    send(&out, msgs);
                }
                if let Some(seat) = seats.get_mut(&player) {
                    let msgs = match &mut turns {
                        Some(t) => turn_cmd(t, player, seat, &cmd),
                        None => applied(player, seat, &cmd),
                    };
                    let mut msgs = msgs;
                    if let Some(t) = &mut turns {
                        let players: Vec<i8> = seats.keys().copied().collect();
                        t.advance(&players);
                        msgs.extend(t.changed(&players));
                    }
                    send(&out, msgs);
                }
            }
            ToWorker::Query { qid, .. } => send(
                &out,
                vec![WorkerMsg::QueryResult {
                    qid,
                    answer: QueryAnswer::CombatPreview(None),
                }],
            ),
            ToWorker::WantFull { player, basis } => {
                if let Some(seat) = seats.get_mut(&player) {
                    seat.rev += 1;
                    let mut msgs = Vec::new();
                    if basis {
                        msgs.push(WorkerMsg::Basis {
                            players: vec![player],
                            basis: mock_basis(),
                        });
                    }
                    msgs.push(full(player, seat));
                    send(&out, msgs);
                }
            }
            ToWorker::Seat { player, change } => {
                let mut msgs = Vec::new();
                let mut error = None;
                match &change {
                    SeatChange::Claim { .. } => {
                        match opts.kv.get("claim").map(String::as_str) {
                            Some("ignore") => continue,
                            Some("refuse") => error = Some("the core refuses".to_owned()),
                            _ => {}
                        }
                        if error.is_none() && seats.contains_key(&player) {
                            error = Some("the slot is taken".to_owned());
                        } else if error.is_none() && !all_players.contains(&player) {
                            error = Some("there is no such slot".to_owned());
                        }
                        if error.is_none() {
                            let seat = Seat {
                                rev: 1,
                                view: view(player, 1),
                                last_cmd: None,
                            };
                            msgs.push(WorkerMsg::Basis {
                                players: vec![player],
                                basis: mock_basis(),
                            });
                            msgs.push(full(player, &seat));
                            seats.insert(player, seat);
                        }
                    }
                    SeatChange::Kick => match opts.kv.get("kick").map(String::as_str) {
                        // `ignore=N`: the first N kicks of this process are not answered.
                        Some("ignore") if kicks_ignored < opts.num("ignore", 1) => {
                            kicks_ignored += 1;
                            continue;
                        }
                        Some("refuse") => error = Some("the core refuses".to_owned()),
                        _ => {
                            seats.remove(&player);
                        }
                    },
                }
                msgs.push(WorkerMsg::SeatDone {
                    player,
                    change,
                    error,
                });
                let players: Vec<i8> = seats.keys().copied().collect();
                match &mut turns {
                    Some(t) => msgs.extend(t.changed(&players)),
                    None => msgs.push(WorkerMsg::Turn(turn_info(&players, 1))),
                }
                send(&out, msgs);
            }
            ToWorker::Focus { .. } => {}
            ToWorker::Net { player, msgs } => match &mut replica {
                Some(r) if !msgs.starts_with(b"reject") => send(&out, vec![r.batch(msgs)]),
                _ => send(
                    &out,
                    vec![WorkerMsg::NetRejected {
                        player,
                        reason: "the mock refuses".into(),
                    }],
                ),
            },
            ToWorker::ReplicaSnapshot { player } => {
                if let Some(r) = &mut replica {
                    send(&out, r.base(Some(player)));
                }
            }
            ToWorker::ForceEndTurn { players } => {
                if let Some(t) = &mut turns {
                    if let Some(path) = &t.force_log {
                        let line = format!("{players:?}\n");
                        let mut f = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(path)
                            .expect("force log");
                        f.write_all(line.as_bytes()).expect("force log");
                    }
                    if !t.ignore_force {
                        for p in players {
                            if seats.contains_key(&p) {
                                t.finished.insert(p);
                            }
                        }
                        let now: Vec<i8> = seats.keys().copied().collect();
                        t.advance(&now);
                        send(&out, t.changed(&now).into_iter().collect());
                    }
                }
            }
            ToWorker::SaveNow { .. } => {
                seq += 1;
                send(&out, vec![saved(&seats, seq)]);
            }
            ToWorker::Shutdown { save } => return shutdown(&out, &seats, &mut seq, save),
        }
    }
}

/// An order of a `turns=1` mock: the turn orders, else as in `applied`.
fn turn_cmd(t: &mut Turns, player: i8, seat: &mut Seat, cmd: &Cmd) -> Vec<WorkerMsg> {
    let done = |id| WorkerMsg::Results {
        player,
        results: vec![CommandResult {
            id,
            outcome: CommandOutcome::Applied,
        }],
    };
    match cmd.cmd {
        Command::EndTurn { .. } => {
            t.finished.insert(player);
            vec![done(cmd.id)]
        }
        Command::CancelEndTurn => {
            t.finished.remove(&player);
            vec![done(cmd.id)]
        }
        Command::DeclareWar { team } => {
            t.wars.push(HumanWar {
                from: player,
                to: team,
            });
            vec![done(cmd.id)]
        }
        _ => applied(player, seat, cmd),
    }
}

/// An applied order and the view it changed: `turn` counts the applied orders.
fn applied(player: i8, seat: &mut Seat, cmd: &Cmd) -> Vec<WorkerMsg> {
    let old = seat.view.clone();
    seat.view.turn += 1;
    seat.rev += 1;
    seat.last_cmd = Some(cmd.id);
    vec![
        WorkerMsg::View {
            player,
            update: ViewUpdate::Delta {
                base_rev: seat.rev - 1,
                rev: seat.rev,
                diff: protocol::diff(&old, &seat.view, None),
                hash: ViewHash::of(&seat.view).value(),
                basis_hash: None,
            },
        },
        WorkerMsg::Results {
            player,
            results: vec![CommandResult {
                id: cmd.id,
                outcome: CommandOutcome::Applied,
            }],
        },
    ]
}

/// A save: the file in the working directory (a real C5SV with a dummy core part) and the message.
fn saved(seats: &BTreeMap<i8, Seat>, seq: u64) -> WorkerMsg {
    let turn = TURN.load(std::sync::atomic::Ordering::Relaxed);
    let file = game_host::saves::rel_path(protocol::SaveKind::Ring, seq, turn);
    let settings = SETTINGS.lock().unwrap().clone();
    if let Some(settings) = settings {
        let header = game_host::saves::SaveHeader {
            seq,
            game: GAME.load(std::sync::atomic::Ordering::Relaxed),
            turn,
            saved_at_ms: 1_000 + seq,
            kind: protocol::SaveKind::Ring,
            build_id: BUILD_ID.into(),
        };
        let sections = game_host::saves::SaveSections {
            seats: SLOTS.lock().unwrap().clone(),
            settings,
            journals: vec![],
            cmds: vec![],
            city_memo: vec![],
        };
        let bytes = game_host::saves::encode(&header, b"mock core", &sections);
        let _ = std::fs::create_dir_all("ring");
        let _ = std::fs::write(&file, bytes);
    }
    WorkerMsg::Saved(SaveRecord {
        meta: SaveMeta {
            seq,
            build_id: BUILD_ID.into(),
            turn,
            civ_name: "Mockland".into(),
            saved_at_ms: 0,
        },
        file,
        kind: SaveKind::Ring,
        applied_upto: seats
            .iter()
            .filter_map(|(p, s)| s.last_cmd.map(|c| (*p, c)))
            .collect(),
    })
}

fn shutdown(out: &Out, seats: &BTreeMap<i8, Seat>, seq: &mut u64, save: bool) -> ExitCode {
    if save {
        *seq += 1;
        send(out, vec![saved(seats, *seq)]);
    }
    ExitCode::SUCCESS
}

fn catalog() -> WorkerMsg {
    WorkerMsg::Catalog {
        hash: protocol::xxh64(0, CATALOG),
        bytes: CATALOG.to_vec(),
    }
}

fn ready(seats: Vec<SlotSetup>, turn: i32) -> WorkerReady {
    WorkerReady {
        build_id: BUILD_ID.into(),
        width: 8,
        height: 4,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 1,
        seats,
        turn,
    }
}

/// `Fatal` and exit with `code`.
fn fail(out: &Out, message: &str, code: u8) -> ExitCode {
    send(
        out,
        vec![WorkerMsg::Fatal {
            message: message.into(),
            code,
        }],
    );
    let _ = io::stdout().flush();
    ExitCode::from(code)
}

/// `(seq, turn)` of a save file name; `(0, 0)` if it is not one.
fn parse_save(file: &str) -> (u64, i32) {
    let name = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let parse = || {
        let (seq, turn) = name.strip_suffix(".c5sv")?.split_once("-t")?;
        Some((seq.parse().ok()?, turn.parse().ok()?))
    };
    parse().unwrap_or((0, 0))
}

/// `once=PATH`: act only in the first process(es) (`times=K`, default 1; the marker files are
/// created then); every later process behaves like `echo`. Without `once` a mode always acts.
fn armed(opts: &Opts) -> bool {
    let Some(path) = opts.kv.get("once") else {
        return true;
    };
    // `times=K`: the first K processes act (markers PATH, PATH.1, ...).
    (0..opts.num("times", 1)).any(|i| {
        let marker = if i == 0 {
            path.clone()
        } else {
            format!("{path}.{i}")
        };
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
            .is_ok()
    })
}

/// Neither reads nor writes any more; only a kill ends it.
fn hang() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}
