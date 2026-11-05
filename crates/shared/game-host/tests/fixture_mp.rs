//! The multiplayer worker on the test world (game-host-mp section 9): `Worker<FixtureCore>` fed with
//! `ToWorker` messages the way the gateway will, and a mirror of every seat that applies what the worker
//! sends the way the gateway and the client do. No I/O, no clock: time is a number the test passes.

use std::collections::BTreeMap;

use game_host::fixture::FixtureCore;
use game_host::{Core, Out, Worker};
use protocol::{
    AutoChoices, BasisState, Cmd, CmdId, Command, CommandOutcome, CommandResult, ContextMsg,
    CoreKind, FixtureTest, GameSettings, GameSetup, IPC_VERSION, JournalEntry, PlayerId,
    RejectCode, SeatChange, SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnInfo, TurnMode,
    TurnPhase, ViewHash, ViewUpdate, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg, WorkerReady,
    WorldView,
};

// ---------------------------------------------------------------- harness

pub fn settings(mode: TurnMode) -> GameSettings {
    GameSettings {
        name: "fixture".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "fixture.lua".into(),
        map_options: Vec::new(),
        speed: "GAMESPEED_STANDARD".into(),
        start_era: "ERA_ANCIENT".into(),
        handicap: "HANDICAP_PRINCE".into(),
        minor_civs: 0,
        turn_mode: mode,
        timer: TimerMode::Original {
            row: "TURNTIMER_FAST".into(),
        },
        game_options: Vec::new(),
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
    }
}

pub fn setup(humans: usize, ais: usize, mode: TurnMode) -> GameSetup {
    let mut slots = Vec::new();
    for i in 0..humans {
        slots.push(SlotSetup {
            player: i as i8,
            civ: protocol::CivPick::Random,
            control: SlotControl::Human {
                nick: format!("p{i}"),
            },
        });
    }
    for i in 0..ais {
        slots.push(SlotSetup {
            player: (humans + i) as i8,
            civ: protocol::CivPick::Random,
            control: SlotControl::Ai,
        });
    }
    GameSetup {
        settings: settings(mode),
        slots,
    }
}

/// What one seat has received, applied the way the gateway applies it.
pub struct Client {
    pub view: Option<WorldView>,
    pub hash: Option<ViewHash>,
    pub basis: Option<BasisState>,
    pub rev: u32,
    pub full_count: u32,
    pub delta_count: u32,
    pub journal: Vec<JournalEntry>,
}

impl Client {
    fn new() -> Self {
        Client {
            view: None,
            hash: None,
            basis: None,
            rev: 0,
            full_count: 0,
            delta_count: 0,
            journal: Vec::new(),
        }
    }
}

/// A context the worker sent: seat, the revision it is for, the message, the number of the frame it was in.
pub type SentContext = (PlayerId, u32, ContextMsg, usize);

pub struct Harness<C: Core = FixtureCore> {
    pub w: Worker<C>,
    pub now: u64,
    pub clients: BTreeMap<PlayerId, Client>,
    pub results: Vec<(PlayerId, CommandResult)>,
    pub turns: Vec<TurnInfo>,
    pub frames: Vec<WorkerFrame>,
    pub exit: Option<u8>,
    pub ready: Option<WorkerReady>,
    pub stats: Vec<protocol::StepStats>,
    pub next_id: BTreeMap<PlayerId, CmdId>,
    pub humans: Vec<PlayerId>,
    /// Every diff sent: seat, diff, basis hash.
    pub diffs: Vec<(PlayerId, protocol::ViewDiff, Option<u64>)>,
    pub contexts: Vec<SentContext>,
    pub seat_done: Vec<(PlayerId, SeatChange, Option<String>)>,
}

pub struct Opts {
    pub humans: usize,
    pub ais: usize,
    pub huge: bool,
    pub wrap_x: bool,
    pub mode: TurnMode,
    pub cfg: WorkerConfig,
}

impl Opts {
    pub fn new(humans: usize, ais: usize) -> Self {
        Opts {
            humans,
            ais,
            huge: false,
            wrap_x: false,
            mode: TurnMode::Simultaneous,
            cfg: WorkerConfig::default(),
        }
    }
}

impl Harness {
    pub fn start(o: Opts) -> Self {
        let mut core = FixtureCore::new(o.huge, o.wrap_x);
        if let Some(t) = o.cfg.fixture_test {
            core.set_test_hooks(t);
        }
        Harness::start_with(o, core)
    }
}

impl<C: Core> Harness<C> {
    pub fn start_with(o: Opts, core: C) -> Self {
        let init = WorkerInit {
            ipc: IPC_VERSION,
            game: 7,
            core: CoreKind::Fixture {
                huge: o.huge,
                wrap_x: o.wrap_x,
            },
            start: Start::New(setup(o.humans, o.ais, o.mode)),
            save_dir: "none".into(),
            seed: 0x1234_5678_9abc,
            cfg: o.cfg,
        };
        let (w, outs) = Worker::new(core, init, 0);
        let mut h = Harness {
            w,
            now: 0,
            clients: BTreeMap::new(),
            results: Vec::new(),
            turns: Vec::new(),
            frames: Vec::new(),
            exit: None,
            ready: None,
            stats: Vec::new(),
            next_id: BTreeMap::new(),
            humans: (0..o.humans).map(|i| i as i8).collect(),
            diffs: Vec::new(),
            contexts: Vec::new(),
            seat_done: Vec::new(),
        };
        for p in h.humans.clone() {
            h.clients.insert(p, Client::new());
        }
        h.feed(outs);
        h
    }

    /// Applies the outs the way the gateway does, and checks the contract of a frame.
    pub fn feed(&mut self, outs: Vec<Out>) {
        for out in outs {
            match out {
                Out::Frame(f) => {
                    self.absorb(&f);
                    self.frames.push(f);
                }
                Out::Exit(c) => self.exit = Some(c),
                // The saves have their own tests (`worker_saves.rs`).
                Out::Save(_) | Out::Replica(_) => {}
            }
        }
    }

    fn absorb(&mut self, f: &WorkerFrame) {
        let mut basis_seen: Vec<PlayerId> = Vec::new();
        let mut viewed: Vec<PlayerId> = Vec::new();
        let mut turn_seen = false;
        let mut pending_basis: BTreeMap<PlayerId, BasisState> = BTreeMap::new();
        for m in &f.msgs {
            assert!(
                !turn_seen
                    || matches!(
                        m,
                        WorkerMsg::Stats(_)
                            | WorkerMsg::Heartbeat { .. }
                            | WorkerMsg::AiTurn { .. }
                            | WorkerMsg::Trace { .. }
                            | WorkerMsg::QueryResult { .. }
                            | WorkerMsg::Fatal { .. }
                            | WorkerMsg::SeatDone { .. }
                            | WorkerMsg::SaveFailed { .. }
                    ),
                "only tail messages after Turn, got {m:?}"
            );
            match m {
                WorkerMsg::Ready(r) => self.ready = Some(r.clone()),
                WorkerMsg::Catalog { .. } | WorkerMsg::Choices(_) => {}
                WorkerMsg::Basis { players, basis } => {
                    for p in players {
                        let b = BasisState::new(basis.clone()).expect("a valid basis");
                        pending_basis.insert(*p, b);
                        basis_seen.push(*p);
                    }
                }
                WorkerMsg::View { player, update } => {
                    assert!(!viewed.contains(player), "two views for seat {player}");
                    viewed.push(*player);
                    let c = self.clients.entry(*player).or_insert_with(Client::new);
                    match update {
                        ViewUpdate::Full { rev, view, hash } => {
                            protocol::check(view).expect("a well-formed keyframe");
                            // A Full after the first one may come without a new Basis (WantFull).
                            let b = match pending_basis.remove(player) {
                                Some(b) => b,
                                None => {
                                    assert!(
                                        c.basis.is_some(),
                                        "first Full of seat {player} without a Basis before it"
                                    );
                                    c.basis.clone().expect("basis")
                                }
                            };
                            protocol::check_pair(view, b.map()).expect("map header equals basis");
                            let h = ViewHash::of(view);
                            assert_eq!(h.value(), *hash, "hash of Full of seat {player}");
                            assert!(*rev > c.rev, "revisions grow");
                            c.rev = *rev;
                            c.view = Some(view.clone());
                            c.hash = Some(h);
                            c.basis = Some(b);
                            c.full_count += 1;
                        }
                        ViewUpdate::Delta {
                            base_rev,
                            rev,
                            diff,
                            hash,
                            basis_hash,
                        } => {
                            assert!(!basis_seen.contains(player), "Basis only before a Full");
                            assert_eq!(*base_rev, c.rev, "base revision of seat {player}");
                            assert_eq!(*rev, c.rev + 1, "revision of seat {player}");
                            assert!(!diff.is_empty(), "an empty diff is not sent");
                            assert_eq!(
                                basis_hash.is_some(),
                                !diff.basis.is_empty(),
                                "basis_hash exactly with a basis diff"
                            );
                            let (v, h, b) = (
                                c.view.as_mut().expect("a view"),
                                c.hash.as_mut().expect("a hash"),
                                c.basis.as_mut().expect("a basis"),
                            );
                            protocol::apply(v, h, b, diff).expect("the diff applies");
                            assert_eq!(h.value(), *hash, "hash after the diff of seat {player}");
                            if let Some(bh) = basis_hash {
                                assert_eq!(b.hash(), *bh, "basis hash after the diff");
                            }
                            c.rev = *rev;
                            c.delta_count += 1;
                            self.diffs.push((*player, diff.clone(), *basis_hash));
                        }
                    }
                }
                WorkerMsg::Results { player, results } => {
                    for r in results {
                        self.results.push((*player, r.clone()));
                    }
                }
                WorkerMsg::Journal { player, entries } => {
                    self.clients
                        .entry(*player)
                        .or_insert_with(Client::new)
                        .journal
                        .extend(entries.iter().cloned());
                }
                WorkerMsg::Context { player, rev, ctx } => {
                    let c = self.clients.get(player).expect("a known seat");
                    assert_eq!(*rev, c.rev, "a context is for the view of the frame");
                    self.contexts
                        .push((*player, *rev, ctx.clone(), self.frames.len()));
                }
                WorkerMsg::SeatDone {
                    player,
                    change,
                    error,
                } => self
                    .seat_done
                    .push((*player, change.clone(), error.clone())),
                WorkerMsg::Turn(t) => {
                    turn_seen = true;
                    self.turns.push(t.clone());
                }
                WorkerMsg::Stats(s) => self.stats.push(s.clone()),
                WorkerMsg::Fatal { message, code } => {
                    panic!("fatal {code}: {message}")
                }
                _ => {}
            }
        }
        // Every seat that got a Basis got a Full with it.
        for p in &basis_seen {
            assert!(viewed.contains(p), "a Basis for seat {p} without a Full");
        }
    }

    /// Steps until the worker wants nothing more.
    pub fn run(&mut self) {
        for _ in 0..10_000 {
            if !self.w.wants_step() {
                return;
            }
            self.now += 1;
            let outs = self.w.step(self.now);
            self.feed(outs);
        }
        panic!("the worker never went idle");
    }

    pub fn send(&mut self, msg: ToWorker) {
        self.now += 1;
        let outs = self.w.handle(msg, self.now);
        self.feed(outs);
    }

    /// An order of `player` for the current turn; returns its number.
    pub fn cmd(&mut self, player: PlayerId, cmd: Command) -> CmdId {
        let turn = self.w.phase().0;
        self.cmd_for(player, turn, cmd)
    }

    pub fn cmd_for(&mut self, player: PlayerId, turn: i32, cmd: Command) -> CmdId {
        let n = self.next_id.entry(player).or_insert(0);
        *n += 1;
        let id = *n;
        self.send(ToWorker::Cmd {
            player,
            cmd: Cmd { id, turn, cmd },
        });
        id
    }

    pub fn outcome(&self, player: PlayerId, id: CmdId) -> Vec<CommandOutcome> {
        self.results
            .iter()
            .filter(|(p, r)| *p == player && r.id == id)
            .map(|(_, r)| r.outcome.clone())
            .collect()
    }

    pub fn last_outcome(&self, player: PlayerId, id: CmdId) -> CommandOutcome {
        self.outcome(player, id)
            .last()
            .cloned()
            .unwrap_or_else(|| panic!("no answer for order {id} of seat {player}"))
    }

    pub fn view(&self, p: PlayerId) -> &WorldView {
        self.clients[&p].view.as_ref().expect("a view")
    }

    /// The view the worker holds for the seat equals the view built from scratch, by hash and by value.
    pub fn assert_in_sync(&mut self) {
        for p in self.humans.clone() {
            let fresh = self.w.fresh_view(p).expect("a fresh view");
            let c = &self.clients[&p];
            let view = c.view.as_ref().expect("a view");
            assert_eq!(
                c.hash.as_ref().expect("hash").value(),
                ViewHash::of(&fresh).value(),
                "hash of the seat {p}: diffs against a view built anew"
            );
            assert_eq!(view, &fresh, "view of seat {p}");
            assert_eq!(self.w.view_hash(p), Some(ViewHash::of(&fresh).value()));
        }
    }

    pub fn phase(&self) -> TurnPhase {
        self.w.phase().1
    }
}

// ---------------------------------------------------------------- N2.11: start, views, basis

fn started(humans: usize, ais: usize, huge: bool) -> Harness {
    let mut h = Harness::start(Opts {
        huge,
        ..Opts::new(humans, ais)
    });
    h.run();
    h
}

#[test]
fn starts_on_two_four_and_eight_seats() {
    for (humans, ais) in [(2, 1), (4, 2), (8, 2)] {
        let mut h = started(humans, ais, false);
        // Catalog and Ready come first, in the first frame.
        let first = &h.frames[0].msgs;
        assert!(matches!(first[0], WorkerMsg::Catalog { .. }));
        assert!(matches!(first[1], WorkerMsg::Ready(_)));
        let ready = h.ready.clone().expect("Ready");
        assert_eq!((ready.width, ready.height), (40, 24));
        assert_eq!(ready.seats.len(), humans + ais);
        assert_eq!(h.phase(), TurnPhase::Humans);
        // Every seat has its Full (revision 1) and nothing else yet.
        for p in h.humans.clone() {
            let c = &h.clients[&p];
            assert_eq!((c.full_count, c.delta_count, c.rev), (1, 0, 1), "seat {p}");
            let v = h.view(p);
            assert_eq!(v.human, p);
            assert_eq!(v.map.plots.len(), 40 * 24);
        }
        // The fog differs between the seats.
        let revealed = |h: &Harness, p: PlayerId| -> Vec<bool> {
            h.view(p)
                .map
                .plots
                .iter()
                .map(|pl| pl.flags.contains(protocol::PlotFlags::REVEALED))
                .collect()
        };
        for a in 0..humans as i8 {
            for b in (a + 1)..humans as i8 {
                assert_ne!(
                    revealed(&h, a),
                    revealed(&h, b),
                    "seats {a} and {b} see alike"
                );
            }
        }
        // The last Turn names every human seat.
        let t = h.turns.last().expect("a Turn");
        assert_eq!(t.phase, TurnPhase::Humans);
        assert_eq!(t.seats.len(), humans);
        h.assert_in_sync();
    }
}

#[test]
fn the_full_hash_is_the_view_hash() {
    let h = started(3, 1, false);
    for p in h.humans.clone() {
        let c = &h.clients[&p];
        let view = c.view.as_ref().expect("view");
        assert_eq!(
            c.hash.as_ref().expect("hash").value(),
            ViewHash::of(view).value()
        );
        assert_eq!(h.w.view_hash(p), Some(ViewHash::of(view).value()));
    }
}

#[test]
fn the_basis_comes_before_the_full_and_not_in_it() {
    let h = started(4, 1, false);
    // One frame carries the seats' Basis, once, before their Views.
    let frame = h
        .frames
        .iter()
        .find(|f| f.msgs.iter().any(|m| matches!(m, WorkerMsg::View { .. })))
        .expect("a frame with views");
    let basis_at = frame
        .msgs
        .iter()
        .position(|m| matches!(m, WorkerMsg::Basis { .. }))
        .expect("a Basis");
    let first_view = frame
        .msgs
        .iter()
        .position(|m| matches!(m, WorkerMsg::View { .. }))
        .expect("a View");
    assert!(basis_at < first_view);
    let n_basis = frame
        .msgs
        .iter()
        .filter(|m| matches!(m, WorkerMsg::Basis { .. }))
        .count();
    assert_eq!(n_basis, 1, "one basis for all seats");
    if let WorkerMsg::Basis { players, basis } = &frame.msgs[basis_at] {
        assert_eq!(players.len(), 4);
        assert_eq!(basis.plots.len(), 40 * 24);
        assert_eq!(basis.basis_hash, protocol::basis_hash_of(basis));
        assert_eq!(basis.game, 7);
    }
    // The keyframe holds no basis (the type has no place for it) and the plots are the team's own.
    // The natural wonder is in the basis under the fog.
    let c = &h.clients[&0];
    let b = c.basis.as_ref().expect("basis");
    let v = c.view.as_ref().expect("view");
    let hidden_wonder = b.map().plots.iter().zip(&v.map.plots).any(|(bp, vp)| {
        bp.flags.contains(protocol::PlotFlags::NATURAL_WONDER)
            && !vp.flags.contains(protocol::PlotFlags::REVEALED)
    });
    assert!(
        hidden_wonder,
        "a natural wonder lies in the basis under the fog"
    );
}

#[test]
fn want_full_sends_the_view_again_with_or_without_the_basis() {
    let mut h = started(2, 1, false);
    let rev = h.clients[&1].rev;
    h.send(ToWorker::WantFull {
        player: 1,
        basis: false,
    });
    let c = &h.clients[&1];
    assert_eq!(c.full_count, 2);
    assert_eq!(c.rev, rev + 1);
    h.send(ToWorker::WantFull {
        player: 0,
        basis: true,
    });
    assert_eq!(h.clients[&0].full_count, 2);
    h.assert_in_sync();
}

#[test]
fn the_huge_map_starts_on_eight_seats() {
    let mut h = started(8, 2, true);
    let ready = h.ready.clone().expect("Ready");
    assert_eq!((ready.width, ready.height), (128, 80));
    for p in h.humans.clone() {
        assert_eq!(h.view(p).map.plots.len(), 128 * 80);
    }
    h.assert_in_sync();
}

#[test]
fn an_idle_worker_sends_a_heartbeat_after_a_second() {
    let mut h = started(2, 1, false);
    assert!(!h.w.wants_step());
    let wake = h.w.next_wake_ms().expect("a wake time");
    let n = h.frames.len();
    let outs = h.w.step(wake - 1);
    assert!(outs.is_empty());
    h.feed(outs);
    let outs = h.w.step(wake);
    h.feed(outs);
    assert_eq!(h.frames.len(), n + 1);
    assert!(matches!(
        h.frames.last().expect("frame").msgs[0],
        WorkerMsg::Heartbeat { .. }
    ));
}

#[test]
fn a_bad_ipc_version_is_fatal() {
    let init = WorkerInit {
        ipc: IPC_VERSION + 1,
        game: 1,
        core: CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        start: Start::New(setup(2, 1, TurnMode::Hybrid)),
        save_dir: String::new(),
        seed: 1,
        cfg: WorkerConfig {
            fixture_test: Some(FixtureTest::default()),
            ..WorkerConfig::default()
        },
    };
    let (_w, outs) = Worker::new(FixtureCore::new(false, false), init, 0);
    assert!(matches!(outs.last(), Some(Out::Exit(2))));
    let Some(Out::Frame(f)) = outs.first() else {
        panic!("a frame with Fatal");
    };
    assert!(matches!(f.msgs[0], WorkerMsg::Fatal { .. }));
}

#[test]
fn describe_answers_the_lists_and_exits() {
    let init = WorkerInit {
        ipc: IPC_VERSION,
        game: 1,
        core: CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        start: Start::Describe,
        save_dir: String::new(),
        seed: 1,
        cfg: WorkerConfig::default(),
    };
    let (_w, outs) = Worker::new(FixtureCore::new(false, false), init, 0);
    assert!(matches!(outs.last(), Some(Out::Exit(0))));
    let Some(Out::Frame(f)) = outs.first() else {
        panic!("a frame");
    };
    assert!(matches!(f.msgs[0], WorkerMsg::Choices(_)));
    assert!(matches!(f.msgs[1], WorkerMsg::Catalog { .. }));
}

#[allow(dead_code)]
fn unused(_: RejectCode) {}

// ---------------------------------------------------------------- a core whose AI phase takes long

use game_host::{
    CoreError, CoreErrorKind, CoreEvent, CorePhase, CorePump, CoreStart, CoreTurn, MapInfo,
    Refusal, RefusalCode,
};
use protocol::{
    Catalog, CityBuildView, CityView, CombatPreview, LobbyChoices, NotificationView, PathNode,
    PlayerView, PlotBasisView, PlotView, ReachNode, Shooter, TeamId, TopBar, UnitActionView,
    UnitView, ViewHeader,
};

/// `FixtureCore` that keeps the AI phase going for `extra` more pumps after the test world has finished it:
/// a stand-in for a real AI phase of many steps.
///
/// With `busy_pumps` it is also a stand-in for the real core of live game 931 (2026-10-30): after an order to a
/// unit the unit stays busy for that many pumps (`CvUnit::IsBusy`, a move the core times one slice per pump),
/// while `safe_point` keeps saying yes. Meanwhile an end of turn is refused with `PUMP_FIRST` and `save` with
/// `NotSafe`, as `gc_apply_command` and `gc_save` did there.
pub struct SlowAi {
    pub inner: FixtureCore,
    pub extra: u32,
    /// Milliseconds every pump of the AI phase takes (0: none).
    pub sleep_ms: u64,
    left: u32,
    in_ai: bool,
    pub busy_pumps: u32,
    busy_left: u32,
    pub saves_refused: u32,
    pub end_turns_refused: u32,
}

impl SlowAi {
    pub fn new(extra: u32) -> Self {
        SlowAi {
            inner: FixtureCore::new(false, false),
            extra,
            sleep_ms: 0,
            left: 0,
            in_ai: false,
            busy_pumps: 0,
            busy_left: 0,
            saves_refused: 0,
            end_turns_refused: 0,
        }
    }

    /// A core whose units stay busy for `pumps` pumps after an order.
    pub fn busy(pumps: u32) -> Self {
        SlowAi {
            busy_pumps: pumps,
            ..SlowAi::new(0)
        }
    }
}

impl Core for SlowAi {
    fn start(&mut self, s: CoreStart<'_>) -> Result<(), CoreError> {
        self.inner.start(s)
    }
    fn catalog(&mut self) -> Result<Catalog, CoreError> {
        self.inner.catalog()
    }
    fn choices(&mut self) -> Result<LobbyChoices, CoreError> {
        self.inner.choices()
    }
    fn map(&mut self) -> MapInfo {
        self.inner.map()
    }
    fn seats(&mut self) -> Result<Vec<SlotSetup>, CoreError> {
        self.inner.seats()
    }
    fn turn_status(&mut self) -> Vec<protocol::AiTurnEvent> {
        self.inner.turn_status()
    }
    fn pump(&mut self, budget: u32) -> Result<CorePump, CoreError> {
        self.busy_left = self.busy_left.saturating_sub(1);
        // The AI phase is pumped in calls of at most 4 slices (the worker's `AI_STEP_SLICES`).
        if self.sleep_ms > 0 && budget <= 4 {
            std::thread::sleep(std::time::Duration::from_millis(self.sleep_ms));
        }
        if self.left > 0 {
            self.left -= 1;
            let mut p = self.inner.pump(budget)?;
            // The inner game is already in the next turn; the worker is told the AI still moves.
            p.phase = if self.left == 0 {
                CorePhase::Humans
            } else {
                CorePhase::Ai { active: 5 }
            };
            return Ok(p);
        }
        let p = self.inner.pump(budget)?;
        match p.phase {
            CorePhase::Ai { .. } => self.in_ai = true,
            CorePhase::Humans if self.in_ai && self.extra > 0 => {
                self.left = self.extra;
                self.in_ai = false;
                return Ok(CorePump {
                    phase: CorePhase::Ai { active: 5 },
                    turn: p.turn,
                });
            }
            _ => {}
        }
        Ok(p)
    }
    fn turn(&mut self) -> Result<CoreTurn, CoreError> {
        self.inner.turn()
    }
    fn command(&mut self, p: PlayerId, c: &Command) -> Result<(), Refusal> {
        if self.busy_left > 0 && matches!(c, Command::EndTurn { .. }) {
            self.end_turns_refused += 1;
            return Err(Refusal::new(
                RefusalCode::PumpFirst,
                "END_TURN refused: a command is queued or a unit / city is busy; pump first",
            ));
        }
        self.inner.command(p, c)?;
        if self.busy_pumps > 0 && !matches!(c, Command::EndTurn { .. } | Command::CancelEndTurn) {
            self.busy_left = self.busy_pumps;
        }
        Ok(())
    }
    fn end_turn_timeout(&mut self, p: PlayerId) -> Result<(), Refusal> {
        self.inner.end_turn_timeout(p)
    }
    fn set_control(&mut self, p: PlayerId, c: &SlotControl) -> Result<(), CoreError> {
        self.inner.set_control(p, c)
    }
    fn safe_point(&mut self) -> Result<(), String> {
        self.inner.safe_point()
    }
    fn save(&mut self) -> Result<Vec<u8>, CoreError> {
        if self.busy_left > 0 {
            self.saves_refused += 1;
            return Err(CoreError::new(
                CoreErrorKind::NotSafe,
                "save: command rejected: gc_save: not at a save point: a unit or city of player 0 is busy",
            ));
        }
        self.inner.save()
    }
    fn dirty_plots(&mut self) -> Option<Vec<u32>> {
        self.inner.dirty_plots()
    }
    fn plots(
        &mut self,
        t: TeamId,
        s: Option<&[u32]>,
        out: &mut Vec<(u32, PlotView)>,
    ) -> Result<(), CoreError> {
        self.inner.plots(t, s, out)
    }
    fn basis(
        &mut self,
        s: Option<&[u32]>,
        out: &mut Vec<(u32, PlotBasisView)>,
    ) -> Result<(), CoreError> {
        self.inner.basis(s, out)
    }
    fn units(&mut self, t: TeamId) -> Result<Vec<UnitView>, CoreError> {
        self.inner.units(t)
    }
    fn cities(&mut self, t: TeamId) -> Result<Vec<CityView>, CoreError> {
        self.inner.cities(t)
    }
    fn players(&mut self, t: TeamId) -> Result<Vec<PlayerView>, CoreError> {
        self.inner.players(t)
    }
    fn header(&mut self, p: PlayerId) -> Result<ViewHeader, CoreError> {
        self.inner.header(p)
    }
    fn top_bar(&mut self, p: PlayerId) -> Result<TopBar, CoreError> {
        self.inner.top_bar(p)
    }
    fn notifications(&mut self, p: PlayerId) -> Result<Vec<NotificationView>, CoreError> {
        self.inner.notifications(p)
    }
    fn unit_actions(&mut self, p: PlayerId, u: i32) -> Result<Vec<UnitActionView>, CoreError> {
        self.inner.unit_actions(p, u)
    }
    fn reach(&mut self, p: PlayerId, u: i32, h: u8) -> Result<Vec<ReachNode>, CoreError> {
        self.inner.reach(p, u, h)
    }
    fn targets(&mut self, p: PlayerId, s: Shooter) -> Result<Vec<(i16, i16, u16)>, CoreError> {
        self.inner.targets(p, s)
    }
    fn preview(
        &mut self,
        p: PlayerId,
        a: Shooter,
        x: i16,
        y: i16,
    ) -> Result<Option<CombatPreview>, CoreError> {
        self.inner.preview(p, a, x, y)
    }
    fn city_builds(&mut self, p: PlayerId, c: i32) -> Result<Vec<CityBuildView>, CoreError> {
        self.inner.city_builds(p, c)
    }
    fn path(&mut self, p: PlayerId, u: i32, x: i16, y: i16) -> Result<Vec<PathNode>, CoreError> {
        self.inner.path(p, u, x, y)
    }
    fn can_make_peace(&mut self, a: PlayerId, b: PlayerId) -> Result<bool, CoreError> {
        self.inner.can_make_peace(a, b)
    }
    fn events(&mut self) -> Vec<CoreEvent> {
        self.inner.events()
    }
    fn digest(&mut self, t: i32) -> Result<u64, CoreError> {
        self.inner.digest(t)
    }
}

pub fn end_turn() -> Command {
    Command::EndTurn { force: false }
}

impl<C: Core> Harness<C> {
    /// Steps while the worker is in the human phase (the queue drains) and stops when the AI phase begins.
    pub fn run_batches_only(&mut self) {
        for _ in 0..1000 {
            if !self.w.wants_step() || !matches!(self.phase(), TurnPhase::Humans) {
                return;
            }
            self.now += 1;
            let outs = self.w.step(self.now);
            self.feed(outs);
        }
        panic!("the batches never ended");
    }
}

// ---------------------------------------------------------------- live game 931: a busy unit

/// Live game 931 (2026-10-30): a seat moved a unit and ended its turn; the core refused the end of turn
/// ("a unit / city is busy; pump first") and nothing pumped it again, so the turn hung, and every save was
/// refused in a hot loop until the worker died. Now the worker pumps until the unit settles: the end of turn is
/// taken, and a save waits with a growing pause and is made, with no `SaveFailed` and no fatal error.
#[test]
fn a_busy_unit_holds_neither_the_end_of_turn_nor_the_saves() {
    let mut h = Harness::start_with(Opts::new(3, 1), SlowAi::busy(4));
    h.run();
    let turn = h.w.phase().0;
    let mv = h.a_move(0).expect("a move of seat 0");
    let moved = h.cmd(0, mv);
    let ended = h.cmd(0, end_turn());
    h.run();
    assert_eq!(h.last_outcome(0, moved), CommandOutcome::Applied);
    assert_eq!(
        h.last_outcome(0, ended),
        CommandOutcome::Applied,
        "the end of turn waits for the busy unit instead of being refused"
    );
    assert!(
        h.w.core_mut().end_turns_refused > 0,
        "the core refused it at first"
    );
    assert!(h.finished(0), "seat 0 has ended its turn");
    assert_eq!(h.w.phase(), (turn, TurnPhase::Humans));

    // Seat 1 moves; a save asked for right away finds the unit busy.
    let mv = h.a_move(1).expect("a move of seat 1");
    h.cmd(1, mv);
    h.run();
    let mut outs = h.w.handle(
        ToWorker::SaveNow {
            reason: protocol::SaveReason::Admin,
        },
        h.now + 1,
    );
    h.now += 1;
    let mut saved = false;
    let mut steps = 0;
    while !saved {
        saved = outs.iter().any(|o| matches!(o, Out::Save(_)));
        let failed = outs.iter().any(|o| {
            matches!(o, Out::Frame(f) if f.msgs.iter().any(|m| matches!(m, WorkerMsg::SaveFailed { .. })))
        });
        assert!(!failed, "a save that waits for the unit is not a failure");
        h.feed(std::mem::take(&mut outs));
        if saved {
            break;
        }
        steps += 1;
        assert!(steps < 20, "the save was never made");
        // The shell sleeps until the worker's wake time: a refused save must not ask to be woken at once.
        let wake = h.w.next_wake_ms().expect("a wake time");
        assert!(
            wake > h.now,
            "no hot loop: the next try is later ({wake} > {})",
            h.now
        );
        h.now = wake;
        outs = h.w.step(h.now);
    }
    let refused = h.w.core_mut().saves_refused;
    assert!(
        (1..=4).contains(&refused),
        "the core refused the save {refused} time(s) while the unit was busy"
    );
    assert_eq!(h.exit, None);
}

// ---------------------------------------------------------------- N2.11: AI phase and basis diff

#[test]
fn no_views_are_built_while_the_ai_moves() {
    let mut h = Harness::start_with(Opts::new(2, 1), SlowAi::new(3));
    h.run();
    let turn = h.w.phase().0;
    h.cmd(0, end_turn());
    h.cmd(1, end_turn());
    h.run_batches_only();
    // The last end of turn started the AI phase.
    assert!(matches!(h.phase(), TurnPhase::Ai { .. }));
    let revs_in_ai: Vec<u32> = h.humans.iter().map(|p| h.clients[p].rev).collect();
    // The AI phase takes more pumps: no view, no journal, only the state of the turn.
    let mut ai_steps = 0;
    while matches!(h.phase(), TurnPhase::Ai { .. }) {
        let before = h.frames.len();
        h.now += 1;
        let outs = h.w.step(h.now);
        h.feed(outs);
        if matches!(h.phase(), TurnPhase::Ai { .. }) {
            ai_steps += 1;
            for f in &h.frames[before..] {
                assert!(
                    f.msgs
                        .iter()
                        .all(|m| !matches!(m, WorkerMsg::View { .. } | WorkerMsg::Journal { .. })),
                    "a view or journal in an AI step: {f:?}"
                );
            }
        }
    }
    assert!(ai_steps >= 2, "the AI phase has steps ({ai_steps})");
    // One update at the end of the phase, in the new turn.
    for (i, p) in h.humans.clone().into_iter().enumerate() {
        let c = &h.clients[&p];
        assert_eq!(
            c.rev,
            revs_in_ai[i] + 1,
            "one update at the end of the AI phase"
        );
        assert_eq!(h.view(p).turn, turn + 1);
    }
    h.assert_in_sync();
}

#[test]
fn a_basis_change_is_one_diff_shared_by_all_seats() {
    let mut h = Harness::start(Opts {
        cfg: WorkerConfig {
            fixture_test: Some(FixtureTest {
                change_basis_at_turn: Some(2),
                ..FixtureTest::default()
            }),
            ..WorkerConfig::default()
        },
        ..Opts::new(3, 1)
    });
    h.run();
    let hash0 = h.clients[&0].basis.as_ref().expect("basis").hash();
    for p in 0..3 {
        h.cmd(p, end_turn());
    }
    h.run();
    assert_eq!(h.w.phase().0, 2);
    let with_basis: Vec<_> = h
        .diffs
        .iter()
        .filter(|(_, d, _)| !d.basis.is_empty())
        .collect();
    assert_eq!(with_basis.len(), 3, "one basis diff per seat");
    let first = &with_basis[0].1.basis;
    for (_, d, bh) in &with_basis {
        assert_eq!(
            &d.basis, first,
            "the diff of the basis is the same for all seats"
        );
        assert!(bh.is_some());
    }
    for p in 0..3 {
        let b = h.clients[&p].basis.as_ref().expect("basis");
        assert_ne!(b.hash(), hash0, "the basis hash changed");
        assert_eq!(Some(b.hash()), with_basis[0].2);
    }
    h.assert_in_sync();
}

// ---------------------------------------------------------------- N2.12: orders, queue, answers, turn

use protocol::{QueryAnswer, UnitFlags};

impl<C: Core> Harness<C> {
    /// A unit of `p` that needs orders, from the view of the seat.
    pub fn unit_needing_orders(&self, p: PlayerId) -> Option<i32> {
        self.view(p)
            .units
            .iter()
            .find(|u| u.owner == p && u.flags.contains(UnitFlags::NEEDS_ORDERS))
            .map(|u| u.id)
    }

    /// A move order for a unit of `p` onto the first step of its move tree.
    pub fn a_move(&mut self, p: PlayerId) -> Option<Command> {
        let ids: Vec<i32> = self
            .view(p)
            .units
            .iter()
            .filter(|u| u.owner == p && u.moves_left > 0 && u.max_moves > 0)
            .map(|u| u.id)
            .collect();
        for unit in ids {
            let tree = self.w.core_mut().reach(p, unit, 1).ok()?;
            if let Some(n) = tree.get(1) {
                return Some(Command::UnitMoveTo {
                    unit,
                    x: n.x,
                    y: n.y,
                });
            }
        }
        None
    }

    /// Everybody ends the turn and the worker runs until the humans move again.
    pub fn next_turn(&mut self) {
        let turn = self.w.phase().0;
        for p in self.humans.clone() {
            if !self.finished(p) {
                self.cmd(p, end_turn());
            }
        }
        self.run();
        assert_eq!(self.w.phase().0, turn + 1, "the turn changed");
        assert_eq!(self.phase(), TurnPhase::Humans);
    }

    /// Frames since frame number `from` that answer orders (the background contexts are frames of their own).
    pub fn answer_frames_since(&self, from: usize) -> usize {
        self.frames[from..]
            .iter()
            .filter(|f| {
                f.msgs
                    .iter()
                    .any(|m| matches!(m, WorkerMsg::Results { .. }))
            })
            .count()
    }

    pub fn finished(&self, p: PlayerId) -> bool {
        self.turns
            .last()
            .and_then(|t| t.seats.iter().find(|s| s.player == p))
            .is_some_and(|s| s.finished)
    }

    /// The frame that carries `outcome` for an order of the seat.
    pub fn frame_of(&self, p: PlayerId, id: CmdId, outcome: &CommandOutcome) -> &WorkerFrame {
        self.frames
            .iter()
            .find(|f| {
                f.msgs.iter().any(|m| {
                    matches!(m, WorkerMsg::Results { player, results }
                    if *player == p && results.iter().any(|r| r.id == id && &r.outcome == outcome))
                })
            })
            .expect("a frame with the answer")
    }
}

fn skip(unit: i32) -> Command {
    Command::UnitSkip { unit }
}

#[test]
fn an_order_is_answered_after_the_view_that_shows_it() {
    let mut h = started(4, 2, false);
    let cmd = h.a_move(0).expect("a move");
    let Command::UnitMoveTo { unit, x, y } = cmd else {
        panic!()
    };
    let others: Vec<u32> = (1..4).map(|p| h.clients[&p].delta_count).collect();
    let id = h.cmd(0, cmd);
    // The order waits for the next step.
    assert!(h.outcome(0, id).is_empty());
    assert!(h.w.wants_step());
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    let view_at = f
        .msgs
        .iter()
        .position(|m| matches!(m, WorkerMsg::View { player: 0, .. }))
        .expect("the view of the seat in the same frame");
    let result_at = f
        .msgs
        .iter()
        .position(|m| matches!(m, WorkerMsg::Results { player: 0, .. }))
        .expect("results");
    assert!(view_at < result_at, "Applied comes after the view");
    let u = h
        .view(0)
        .units
        .iter()
        .find(|u| u.id == unit && u.owner == 0);
    assert_eq!(
        u.map(|u| (u.x, u.y)),
        Some((x, y)),
        "the view shows the move"
    );
    // Another seat gets a diff only when it sees the change.
    for p in 1..4i8 {
        let sees = h.view(p).units.iter().any(|u| u.owner == 0);
        let got = h.clients[&p].delta_count - others[usize::from(p as u8) - 1];
        if !sees {
            assert_eq!(got, 0, "seat {p} sees nothing of seat 0 and gets no diff");
        }
    }
    h.assert_in_sync();
}

#[test]
fn orders_of_several_seats_are_batched_in_arrival_order() {
    let mut h = started(4, 1, false);
    let mut ids = Vec::new();
    for p in 0..4 {
        if let Some(u) = h.unit_needing_orders(p) {
            ids.push((p, h.cmd(p, skip(u))));
        }
    }
    assert_eq!(ids.len(), 4);
    let frames = h.frames.len();
    h.run();
    // Four orders are one batch, one frame (the contexts that follow are frames of their own).
    assert_eq!(h.answer_frames_since(frames), 1);
    for (p, id) in ids {
        assert_eq!(h.last_outcome(p, id), CommandOutcome::Applied);
    }
    h.assert_in_sync();
}

#[test]
fn a_batch_is_at_most_eight_orders() {
    let mut h = Harness::start(Opts::new(2, 1));
    h.run();
    let u = h.unit_needing_orders(0).expect("a unit");
    let ids: Vec<CmdId> = (0..20).map(|_| h.cmd(0, skip(u))).collect();
    let frames = h.frames.len();
    h.run();
    assert_eq!(h.answer_frames_since(frames), 3, "20 orders: 8 + 8 + 4");
    for id in ids {
        assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    }
}

#[test]
fn an_order_of_an_earlier_turn_is_turn_changed() {
    let mut h = started(2, 1, false);
    let u = h.unit_needing_orders(0).expect("a unit");
    let turn = h.w.phase().0;
    let id = h.cmd_for(0, turn - 1, skip(u));
    h.run();
    assert!(matches!(
        h.last_outcome(0, id),
        CommandOutcome::Rejected {
            code: RejectCode::TurnChanged,
            ..
        }
    ));
}

#[test]
fn a_seat_with_no_human_is_not_seated() {
    let mut h = started(2, 1, false);
    // Player 2 is an AI, 9 does not exist.
    for p in [2, 9] {
        h.send(ToWorker::Cmd {
            player: p,
            cmd: Cmd {
                id: 1,
                turn: 1,
                cmd: end_turn(),
            },
        });
        assert!(matches!(
            h.results.last(),
            Some((q, CommandResult { id: 1, outcome: CommandOutcome::Rejected { code: RejectCode::NotSeated, .. } })) if *q == p
        ));
        h.results.clear();
    }
    assert!(!h.w.wants_step());
}

#[test]
fn orders_before_the_game_starts_have_no_game() {
    let mut h = Harness::start(Opts::new(2, 1));
    assert_eq!(h.phase(), TurnPhase::Starting);
    let id = h.cmd_for(0, 1, end_turn());
    assert!(matches!(
        h.last_outcome(0, id),
        CommandOutcome::Rejected {
            code: RejectCode::NoGame,
            ..
        }
    ));
}

#[test]
fn orders_in_the_ai_phase_wait_and_are_applied_in_the_next_human_phase() {
    let mut h = Harness::start_with(Opts::new(2, 1), SlowAi::new(3));
    h.run();
    let turn = h.w.phase().0;
    let unit = h.unit_needing_orders(0).expect("a unit");
    h.cmd(0, end_turn());
    h.cmd(1, end_turn());
    h.run_batches_only();
    assert!(matches!(h.phase(), TurnPhase::Ai { .. }));
    // One order for the coming turn, one for the turn that is ending, and too many from seat 1.
    let next = h.cmd_for(0, turn + 1, skip(unit));
    let old = h.cmd_for(0, turn, skip(unit));
    assert_eq!(h.outcome(0, next), vec![CommandOutcome::Queued]);
    assert_eq!(h.outcome(0, old), vec![CommandOutcome::Queued]);
    let bulk: Vec<CmdId> = (0..66)
        .map(|_| h.cmd_for(1, turn + 1, skip(unit)))
        .collect();
    let too_many = bulk
        .iter()
        .filter(|id| {
            matches!(
                h.outcome(1, **id).last(),
                Some(CommandOutcome::Rejected {
                    code: RejectCode::TooMany,
                    ..
                })
            )
        })
        .count();
    assert_eq!(too_many, 2, "the queue of a seat holds 64");
    // The phase ends: the queued orders are applied to the new turn, in order.
    h.run();
    assert_eq!(h.w.phase().0, turn + 1);
    assert_eq!(h.last_outcome(0, next), CommandOutcome::Applied);
    assert!(matches!(
        h.last_outcome(0, old),
        CommandOutcome::Rejected {
            code: RejectCode::TurnChanged,
            ..
        }
    ));
    // Seat 1 has no unit with that id (it is seat 0's): the core decides, every number is answered once
    // with a final outcome.
    for id in bulk {
        let all = h.outcome(1, id);
        let finals = all
            .iter()
            .filter(|o| !matches!(o, CommandOutcome::Queued))
            .count();
        assert_eq!(finals, 1, "order {id} has one final answer: {all:?}");
    }
    h.assert_in_sync();
}

#[test]
fn the_turn_passes_through_the_ai_phase() {
    let mut h = started(3, 2, false);
    let t0 = h.turns.len();
    h.next_turn();
    let seen: Vec<(i32, TurnPhase)> = h.turns[t0..].iter().map(|t| (t.turn, t.phase)).collect();
    // All three ends of turn are one batch: the phase goes from the humans' straight to the AI's.
    assert!(
        seen.iter()
            .any(|(t, p)| *t == 1 && matches!(p, TurnPhase::Ai { .. })),
        "{seen:?}"
    );
    assert_eq!(seen.last(), Some(&(2, TurnPhase::Humans)));
    // The new turn starts clean.
    let last = h.turns.last().expect("turn");
    assert!(last.seats.iter().all(|s| !s.finished && s.may_act));
    assert_eq!(last.mode, TurnMode::Simultaneous);
    assert_eq!(last.turn_len_s, 30, "the timer row of the settings");
    assert_eq!(last.timer_base_s, 30);
    h.assert_in_sync();
}

#[test]
fn hybrid_humans_at_war_take_turns() {
    let mut h = Harness::start(Opts {
        mode: TurnMode::Hybrid,
        ..Opts::new(3, 1)
    });
    h.run();
    // Seat 0 declares war on seat 1: both move in turn now, seat 2 still moves at once.
    let id = h.cmd(0, Command::DeclareWar { team: 1 });
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let t = h.turns.last().expect("turn");
    assert_eq!(t.mode, TurnMode::Hybrid);
    assert!(t.seats[0].may_act && !t.seats[0].simultaneous);
    assert!(!t.seats[1].may_act && !t.seats[1].simultaneous);
    assert!(t.seats[2].may_act && t.seats[2].simultaneous);
    // The war between two humans that moved at once is reported once, for the timer.
    let wars: Vec<_> = h.turns.iter().flat_map(|t| t.human_wars.iter()).collect();
    assert_eq!(wars.len(), 1);
    assert_eq!((wars[0].from, wars[0].to), (0, 1));
    // Seat 1 has to wait: the core says it is not its turn.
    let u1 = h.unit_needing_orders(1).expect("unit");
    let wait = h.cmd(1, skip(u1));
    h.run();
    assert!(matches!(
        h.last_outcome(1, wait),
        CommandOutcome::Rejected {
            code: RejectCode::NotYourTurn,
            ..
        }
    ));
    // Seat 0 ends its turn: seat 1 moves.
    h.cmd(0, end_turn());
    h.run();
    let t = h.turns.last().expect("turn");
    assert!(t.seats[0].finished && t.seats[1].may_act);
    let ok = h.cmd(1, skip(u1));
    h.run();
    assert_eq!(h.last_outcome(1, ok), CommandOutcome::Applied);
    // A seat that has ended its turn in turn cannot take the end back by an order: the next human moves.
    let late = h.cmd(0, skip(h.unit_needing_orders(0).expect("unit")));
    h.run();
    assert!(matches!(
        h.last_outcome(0, late),
        CommandOutcome::Rejected {
            code: RejectCode::TurnEnded,
            ..
        }
    ));
    assert!(h.finished(0), "the end of turn is not changed");
    h.assert_in_sync();
}

#[test]
fn force_end_turn_ends_the_turn_of_the_named_seats() {
    let mut h = started(3, 1, false);
    let turn = h.w.phase().0;
    h.send(ToWorker::ForceEndTurn {
        players: vec![0, 1],
    });
    h.run();
    assert!(h.finished(0) && h.finished(1) && !h.finished(2));
    assert!(
        h.results.is_empty(),
        "the timer has no number and no answer"
    );
    // A timer that fires late, for a turn that is over, does nothing.
    h.send(ToWorker::ForceEndTurn { players: vec![2] });
    // It is queued for this turn: seat 2 ends now.
    h.run();
    assert_eq!(h.w.phase().0, turn + 1);
    h.send(ToWorker::ForceEndTurn { players: vec![9] });
    assert!(!h.w.wants_step(), "a player with no seat is ignored");
}

#[test]
fn the_end_of_turn_can_be_taken_back() {
    let mut h = started(3, 1, false);
    h.cmd(0, end_turn());
    h.run();
    assert!(h.finished(0));
    let id = h.cmd(0, Command::CancelEndTurn);
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    assert!(!h.finished(0));
    // Not ended: the core refuses.
    let again = h.cmd(0, Command::CancelEndTurn);
    h.run();
    assert!(matches!(
        h.last_outcome(0, again),
        CommandOutcome::Rejected {
            code: RejectCode::Core,
            ..
        }
    ));
}

#[test]
fn an_order_after_the_own_end_of_turn_takes_the_end_back_and_runs() {
    let mut h = started(3, 1, false);
    let unit = h.unit_needing_orders(0).expect("unit");
    h.cmd(0, end_turn());
    h.run();
    assert!(h.finished(0));
    let id = h.cmd(0, skip(unit));
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    // The same frame: the view, the answer and the turn state with `finished = false`.
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    let turn = f
        .msgs
        .iter()
        .find_map(|m| match m {
            WorkerMsg::Turn(t) => Some(t),
            _ => None,
        })
        .expect("a Turn in the frame of the answer");
    assert!(!turn.seats[0].finished);
    assert!(
        f.msgs
            .iter()
            .any(|m| matches!(m, WorkerMsg::View { player: 0, .. }))
    );
    assert!(!h.finished(0));
    h.assert_in_sync();
}

#[test]
fn the_end_stays_taken_back_when_the_core_refuses_the_order() {
    let mut h = started(3, 1, false);
    h.cmd(0, end_turn());
    h.run();
    // A unit that is not there.
    let id = h.cmd(0, skip(9999));
    h.run();
    assert!(matches!(
        h.last_outcome(0, id),
        CommandOutcome::Rejected {
            code: RejectCode::Core,
            ..
        }
    ));
    assert!(
        !h.finished(0),
        "the end of turn was taken back all the same"
    );
    let f = h.frame_of(0, id, &h.outcome(0, id).last().cloned().expect("an answer"));
    assert!(f.msgs.iter().any(|m| matches!(m, WorkerMsg::Turn(_))));
}

#[test]
fn queries_are_answered_at_once() {
    let mut h = started(2, 1, false);
    let unit = h.unit_needing_orders(0).expect("unit");
    h.send(ToWorker::Query {
        player: 0,
        qid: 5,
        q: protocol::Query::Units,
    });
    h.send(ToWorker::Query {
        player: 0,
        qid: 6,
        q: protocol::Query::UnitActions { unit },
    });
    h.send(ToWorker::Query {
        player: 7,
        qid: 7,
        q: protocol::Query::Units,
    });
    let answers: Vec<(u64, QueryAnswer)> = h
        .frames
        .iter()
        .flat_map(|f| f.msgs.iter())
        .filter_map(|m| match m {
            WorkerMsg::QueryResult { qid, answer } => Some((*qid, answer.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(answers.len(), 3);
    assert!(matches!(&answers[0], (5, QueryAnswer::Units(u)) if *u == h.view(0).units));
    assert!(matches!(&answers[1], (6, QueryAnswer::UnitActions(a)) if !a.is_empty()));
    assert!(matches!(&answers[2], (7, QueryAnswer::Error { .. })));
}

#[test]
fn the_journal_goes_to_its_addressees_as_the_core_wrote_it() {
    let mut h = started(4, 1, false);
    for _ in 0..9 {
        h.next_turn();
    }
    for p in 0..4 {
        let j = &h.clients[&p].journal;
        // Whole-game lines reach every seat once.
        let begins = j
            .iter()
            .filter(|e| e.text == "Turn 5 begins (fixture)")
            .count();
        assert_eq!(begins, 1, "seat {p}");
        // A line for one player reaches that player only.
        let learned = j
            .iter()
            .filter(|e| e.text == "Agriculture researched")
            .count();
        assert_eq!(learned, 1, "seat {p}");
    }
    let total: usize = (0..4)
        .map(|p| {
            h.clients[&p]
                .journal
                .iter()
                .filter(|e| e.text == "Agriculture researched")
                .count()
        })
        .sum();
    assert_eq!(total, 4);
}

// ---------------------------------------------------------------- N2.12: random play

/// xorshift64*: the same orders in every run.
pub struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

impl<C: Core> Harness<C> {
    /// A random order for `p`: mostly legal ones, some that the core refuses.
    pub fn random_cmd(&mut self, p: PlayerId, rng: &mut Rng) -> Command {
        let units: Vec<(i32, i16, i16)> = self
            .view(p)
            .units
            .iter()
            .filter(|u| u.owner == p)
            .map(|u| (u.id, u.x, u.y))
            .collect();
        let cities: Vec<i32> = self
            .view(p)
            .cities
            .iter()
            .filter(|c| c.owner == p)
            .map(|c| c.id)
            .collect();
        let unit = units
            .get(rng.below(units.len() as u64) as usize)
            .map_or(9999, |u| u.0);
        match rng.below(100) {
            0..=39 => {
                let tree = self.w.core_mut().reach(p, unit, 1).unwrap_or_default();
                match tree.get(rng.below(tree.len() as u64) as usize) {
                    Some(n) => Command::UnitMoveTo {
                        unit,
                        x: n.x,
                        y: n.y,
                    },
                    None => skip(unit),
                }
            }
            40..=47 => Command::UnitFortify { unit },
            48..=53 => Command::UnitSleep { unit },
            54..=62 => skip(unit),
            63..=67 => Command::UnitWake { unit },
            68..=72 => Command::UnitRangeAttack {
                unit,
                x: rng.below(40) as i16,
                y: rng.below(24) as i16,
            },
            73..=79 => Command::CitySetProduction {
                city: cities
                    .get(rng.below(cities.len() as u64) as usize)
                    .copied()
                    .unwrap_or(77),
                order: rng.below(2) as i8,
                item: rng.below(4) as i16,
            },
            80..=83 => Command::CancelEndTurn,
            84 => Command::DeclareWar {
                team: rng.below(5) as i8,
            },
            85..=89 => Command::UnitMoveTo {
                unit,
                x: rng.below(40) as i16,
                y: rng.below(24) as i16,
            },
            90..=91 => Command::UnitDisband { unit },
            _ => end_turn(),
        }
    }
}

fn random_play(humans: usize, ais: usize, mode: TurnMode, orders: usize, huge: bool) -> Harness {
    let mut h = Harness::start(Opts {
        mode,
        huge,
        ..Opts::new(humans, ais)
    });
    h.run();
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut sent = 0;
    let mut sent_ids: Vec<(PlayerId, CmdId)> = Vec::new();
    while sent < orders {
        // A few orders from random seats, then the worker runs; the views are checked now and then.
        for _ in 0..1 + rng.below(12) {
            // Seats that have ended their turn are left alone most of the time, so that turns pass.
            let open: Vec<PlayerId> = h
                .humans
                .iter()
                .copied()
                .filter(|p| !h.finished(*p))
                .collect();
            let p = if !open.is_empty() && rng.below(100) < 92 {
                open[rng.below(open.len() as u64) as usize]
            } else {
                rng.below(humans as u64) as PlayerId
            };
            let cmd = h.random_cmd(p, &mut rng);
            let id = h.cmd(p, cmd);
            sent_ids.push((p, id));
            sent += 1;
        }
        h.run();
        if sent % 97 < 12 {
            h.assert_in_sync();
        }
    }
    h.assert_in_sync();
    // Every order was answered exactly once with a final outcome.
    let applied = sent_ids
        .iter()
        .filter(|(p, id)| h.last_outcome(*p, *id) == CommandOutcome::Applied)
        .count();
    let mut hist: BTreeMap<String, usize> = BTreeMap::new();
    for (p, id) in &sent_ids {
        let key = match h.last_outcome(*p, *id) {
            CommandOutcome::Rejected { code, reason } => format!("{code:?}: {reason}"),
            o => format!("{o:?}"),
        };
        *hist.entry(key).or_default() += 1;
    }
    println!(
        "{orders} orders, {applied} applied, game turn {}: {hist:#?}",
        h.w.phase().0
    );
    assert!(
        applied > orders / 10,
        "the random orders do something ({applied})"
    );
    for (p, id) in sent_ids {
        let all = h.outcome(p, id);
        let finals = all
            .iter()
            .filter(|o| !matches!(o, CommandOutcome::Queued))
            .count();
        assert_eq!(finals, 1, "order {id} of seat {p}: {all:?}");
    }
    h
}

#[test]
fn ten_thousand_random_orders_on_four_seats_keep_the_views_equal_to_a_fresh_build() {
    let h = random_play(4, 2, TurnMode::Simultaneous, 10_000, false);
    assert!(h.w.phase().0 > 5, "the game went on");
    for p in 0..4 {
        assert!(h.clients[&p].delta_count > 100);
    }
}

#[test]
fn random_play_in_hybrid_mode_with_wars_between_the_seats() {
    random_play(4, 1, TurnMode::Hybrid, 3_000, false);
}

// ---------------------------------------------------------------- N2.13: cities under fog, auditor, dirty plots

use protocol::PlotFlags;

fn audited(humans: usize, ais: usize, leak: bool) -> Harness {
    Harness::start(Opts {
        mode: TurnMode::Simultaneous,
        cfg: WorkerConfig {
            audit_fog: true,
            stats: true,
            fixture_test: Some(FixtureTest {
                leak_fog: leak,
                ..FixtureTest::default()
            }),
            ..WorkerConfig::default()
        },
        ..Opts::new(humans, ais)
    })
}

/// Plays `turns` turns: a few random orders per seat, then everybody ends the turn. Returns how many foreign
/// units and cities the seats had in view over the game (so a test knows the auditor had something to check).
fn play_turns(h: &mut Harness, turns: i32, rng: &mut Rng) -> (usize, usize) {
    h.run();
    let (mut units, mut cities) = (0, 0);
    let end = h.w.phase().0 + turns;
    while h.w.phase().0 < end {
        for p in h.humans.clone() {
            for _ in 0..4 {
                let cmd = h.random_cmd(p, rng);
                if !matches!(cmd, Command::EndTurn { .. } | Command::CancelEndTurn) {
                    h.cmd(p, cmd);
                }
            }
        }
        h.run();
        for p in h.humans.clone() {
            units += h.view(p).units.iter().filter(|u| u.owner != p).count();
            cities += h.view(p).cities.iter().filter(|c| c.owner != p).count();
        }
        let t = h.w.phase().0;
        for p in h.humans.clone() {
            h.cmd(p, end_turn());
        }
        h.run();
        assert_eq!(h.w.phase().0, t + 1);
    }
    (units, cities)
}

#[test]
fn the_auditor_finds_no_violation_in_a_hundred_turns_of_four_seats() {
    let mut h = audited(4, 2, false);
    let mut rng = Rng(77);
    let (units, _cities) = play_turns(&mut h, 100, &mut rng);
    assert!(units > 0, "the seats met foreign units, the audit had work");
    assert_eq!(h.w.fog_violations(), 0);
    let stat_sum: u32 = h.stats.iter().map(|s| s.fog_violations).sum();
    assert_eq!(stat_sum, 0, "StepStats count the same");
    assert!(!h.stats.is_empty());
    h.assert_in_sync();
}

#[test]
fn the_fog_leak_control_is_caught() {
    let mut h = audited(3, 1, true);
    let mut rng = Rng(5);
    play_turns(&mut h, 3, &mut rng);
    assert!(h.w.fog_violations() > 0, "the auditor sees the leaked unit");
    let in_stats: u32 = h.stats.iter().map(|s| s.fog_violations).sum();
    assert_eq!(u64::from(in_stats), h.w.fog_violations());
}

#[test]
fn without_the_audit_switch_nothing_is_checked() {
    let mut h = Harness::start(Opts {
        cfg: WorkerConfig {
            fixture_test: Some(FixtureTest {
                leak_fog: true,
                ..FixtureTest::default()
            }),
            ..WorkerConfig::default()
        },
        ..Opts::new(3, 1)
    });
    let mut rng = Rng(5);
    play_turns(&mut h, 3, &mut rng);
    assert_eq!(h.w.fog_violations(), 0);
}

/// Hex distance on the odd-row layout (odd rows shifted half a hex east).
fn hex_distance(a: (i16, i16), b: (i16, i16)) -> i32 {
    let cube = |(x, y): (i16, i16)| {
        let (x, y) = (i32::from(x), i32::from(y));
        let q = x - (y - (y & 1)) / 2;
        (q, y, -q - y)
    };
    let (p, q) = (cube(a), cube(b));
    ((p.0 - q.0).abs() + (p.1 - q.1).abs() + (p.2 - q.2).abs()) / 2
}

impl<C: Core> Harness<C> {
    /// Moves every mobile unit of `p` one step along its move tree, towards (`toward`) or away from `at`.
    fn walk(&mut self, p: PlayerId, at: (i16, i16), toward: bool) {
        let units: Vec<(i32, i16)> = self
            .view(p)
            .units
            .iter()
            .filter(|u| u.owner == p && u.moves_left > 0 && u.max_moves > 0)
            .map(|u| (u.id, u.moves_left))
            .collect();
        for (unit, _) in units {
            let Ok(tree) = self.w.core_mut().reach(p, unit, 1) else {
                continue;
            };
            let best = tree
                .iter()
                .filter(|n| {
                    n.flags.0 & (protocol::ReachFlags::ATTACK | protocol::ReachFlags::CITY) == 0
                })
                .min_by_key(|n| {
                    let d = hex_distance((n.x, n.y), at);
                    if toward { d } else { -d }
                });
            if let Some(n) = best {
                self.cmd(
                    p,
                    Command::UnitMoveTo {
                        unit,
                        x: n.x,
                        y: n.y,
                    },
                );
            }
        }
    }

    fn city_in_view(&self, p: PlayerId, owner: PlayerId) -> Option<(CityView, bool)> {
        let v = self.view(p);
        let c = v.cities.iter().find(|c| c.owner == owner)?;
        let idx =
            usize::try_from(c.y).ok()? * usize::from(v.map.width) + usize::try_from(c.x).ok()?;
        let visible = v.map.plots[idx].flags.contains(PlotFlags::VISIBLE);
        Some((c.clone(), visible))
    }

    fn end_all_and_run(&mut self) {
        let t = self.w.phase().0;
        for p in self.humans.clone() {
            self.cmd(p, end_turn());
        }
        self.run();
        assert_eq!(self.w.phase().0, t + 1);
    }
}

#[test]
fn a_foreign_city_under_fog_keeps_the_values_last_seen_and_the_audit_agrees() {
    let mut h = audited(2, 0, false);
    h.run();
    // The cities of both seats; seat 0's units walk to the city of seat 1 until they see it.
    let target = {
        let c = h.w.core_mut().cities(1).expect("cities of seat 1");
        let c = c.iter().find(|c| c.owner == 1).expect("a city");
        (c.x, c.y)
    };
    for _ in 0..80 {
        if matches!(h.city_in_view(0, 1), Some((_, true))) {
            break;
        }
        h.walk(0, target, true);
        h.run();
        h.end_all_and_run();
    }
    let (c, visible) = h.city_in_view(0, 1).expect("seat 0 has found the city");
    assert!(visible);
    assert!(
        c.population > 0 && c.max_hp > 0,
        "an open city has its values"
    );
    let mut last_seen = c;

    // The units go home; the city leaves the sight and the city of seat 1 goes on (growth, buildings).
    let mut hidden_turns = 0;
    for _ in 0..80 {
        let in_sight = matches!(h.city_in_view(0, 1), Some((_, true)));
        if let Some((c, true)) = h.city_in_view(0, 1) {
            last_seen = c;
        }
        if !in_sight {
            hidden_turns += 1;
        }
        if hidden_turns >= 14 {
            break;
        }
        if in_sight {
            h.walk(0, target, false);
            h.run();
        }
        h.end_all_and_run();
    }
    assert!(hidden_turns >= 14, "the city left the sight");
    let (shown, visible) = h.city_in_view(0, 1).expect("the city stays in the view");
    assert!(!visible);
    // What the core knows of the city today is more than what the seat is shown.
    let truth = h.w.core_mut().cities(1).expect("cities");
    let truth = truth.iter().find(|c| c.owner == 1).expect("the city");
    assert_ne!(
        (truth.population, &truth.buildings, truth.hp),
        (last_seen.population, &last_seen.buildings, last_seen.hp),
        "the city changed while it was out of sight"
    );
    // The seat is shown the values of its last sight.
    assert_eq!(
        (shown.population, shown.hp, shown.max_hp, &shown.buildings),
        (
            last_seen.population,
            last_seen.hp,
            last_seen.max_hp,
            &last_seen.buildings
        )
    );
    assert_eq!(shown.name, last_seen.name);
    assert_eq!(
        h.w.fog_violations(),
        0,
        "the audit accepts the last seen values"
    );
    h.assert_in_sync();
}

fn steps_equal_fresh_views(humans: usize, ais: usize, steps: usize, check_every: usize) {
    let mut h = Harness::start(Opts::new(humans, ais));
    h.run();
    let mut rng = Rng(0xfeed_beef);
    let mut done = 0;
    while done < steps {
        let n = 1 + rng.below(3);
        for _ in 0..n {
            let open: Vec<PlayerId> = h
                .humans
                .iter()
                .copied()
                .filter(|p| !h.finished(*p))
                .collect();
            let p = if open.is_empty() {
                rng.below(humans as u64) as PlayerId
            } else {
                open[rng.below(open.len() as u64) as usize]
            };
            let cmd = h.random_cmd(p, &mut rng);
            h.cmd(p, cmd);
        }
        // One step at a time; the views of the diffs must be the views built from all plots.
        while h.w.wants_step() && done < steps {
            h.now += 1;
            let outs = h.w.step(h.now);
            h.feed(outs);
            done += 1;
            if done % check_every == 0 {
                h.assert_in_sync();
            }
        }
    }
    h.assert_in_sync();
    assert!(h.w.phase().0 > 3, "turns passed");
}

#[test]
fn a_view_by_dirty_plots_equals_a_view_by_all_plots_over_ten_thousand_steps_on_four_seats() {
    steps_equal_fresh_views(4, 2, 10_000, 4);
}

#[test]
fn a_view_by_dirty_plots_equals_a_view_by_all_plots_over_ten_thousand_steps_on_eight_seats() {
    steps_equal_fresh_views(8, 2, 10_000, 10);
}

// ---------------------------------------------------------------- N3.27: focus and contexts

use protocol::{Focus, ReachFlags};

const SETTLER: i32 = 2;
const WORKER: i32 = 3;
const SCOUT: i32 = 5;

/// The unit a unit context or a `Dropped` is about.
fn ctx_unit(c: &ContextMsg) -> Option<i32> {
    match c {
        ContextMsg::Unit(u) => Some(u.unit),
        ContextMsg::Dropped { unit } => Some(*unit),
        _ => None,
    }
}

impl<C: Core> Harness<C> {
    /// The contexts of `p` that came after the `from`-th one sent so far.
    fn contexts_since(&self, p: PlayerId, from: usize) -> Vec<&SentContext> {
        self.contexts[from..].iter().filter(|c| c.0 == p).collect()
    }

    fn unit_pos(&self, p: PlayerId, id: i32) -> (i16, i16) {
        let u = self
            .view(p)
            .units
            .iter()
            .find(|u| u.owner == p && u.id == id)
            .expect("the unit");
        (u.x, u.y)
    }

    /// Moves `unit` of `p` to the farthest plot of its move tree (a walk, not an attack).
    fn go_far(&mut self, p: PlayerId, unit: i32) -> CmdId {
        let tree = self.w.core_mut().reach(p, unit, 1).expect("a tree");
        let root = (tree[0].x, tree[0].y);
        let far = tree
            .iter()
            .filter(|n| n.flags.0 & (ReachFlags::ATTACK | ReachFlags::CITY) == 0)
            .max_by_key(|n| hex_distance((n.x, n.y), root))
            .expect("a node");
        let id = self.cmd(
            p,
            Command::UnitMoveTo {
                unit,
                x: far.x,
                y: far.y,
            },
        );
        self.run();
        id
    }

    fn focus(&mut self, p: PlayerId, units: &[i32], city: Option<i32>) {
        self.send(ToWorker::Focus {
            player: p,
            focus: Focus {
                units: units.to_vec(),
                city,
            },
        });
        self.run();
    }
}

#[test]
fn the_default_focus_is_the_first_three_units_that_need_orders_computed_in_the_background() {
    let mut h = started(2, 1, false);
    for p in 0..2i8 {
        let mut want: Vec<i32> = h
            .view(p)
            .units
            .iter()
            .filter(|u| u.owner == p && u.flags.contains(UnitFlags::NEEDS_ORDERS))
            .map(|u| u.id)
            .collect();
        want.sort_unstable();
        want.truncate(3);
        assert_eq!(want, vec![SETTLER, WORKER, 4], "settler, worker, archer");
        let got: Vec<&SentContext> = h.contexts.iter().filter(|c| c.0 == p).collect();
        let ids: Vec<i32> = got.iter().filter_map(|c| ctx_unit(&c.2)).collect();
        assert_eq!(ids, want, "seat {p}: the default focus, in its order");
        for c in &got {
            let ContextMsg::Unit(u) = &c.2 else {
                panic!("a unit that moves gets a unit context, got {:?}", c.2)
            };
            assert_eq!(u.view_seq, 0, "the gateway sets the sequence");
            assert!(!u.actions.is_empty() && !u.reach.is_empty());
            let core = h.w.core_mut();
            assert_eq!(u.reach, core.reach(p, u.unit, 1).expect("reach"));
            assert_eq!(
                u.actions,
                core.unit_actions(p, u.unit).expect("actions"),
                "the actions are the core's"
            );
        }
    }
    // The background does one context per step: one per frame, none with a view.
    let frames: std::collections::BTreeSet<usize> = h.contexts.iter().map(|c| c.3).collect();
    assert_eq!(frames.len(), h.contexts.len());
    for (_, _, _, f) in &h.contexts {
        assert!(
            !h.frames[*f]
                .msgs
                .iter()
                .any(|m| matches!(m, WorkerMsg::View { .. })),
            "a background context has a frame of its own"
        );
    }
    assert!(!h.w.wants_step(), "everything wanted has been computed");
}

#[test]
fn nothing_is_computed_while_the_ai_moves_and_a_focus_sent_then_holds_for_the_next_turn() {
    let mut h = Harness::start_with(Opts::new(2, 1), SlowAi::new(3));
    h.run();
    h.cmd(0, end_turn());
    h.cmd(1, end_turn());
    h.run_batches_only();
    assert!(matches!(h.phase(), TurnPhase::Ai { .. }));
    let sent = h.contexts.len();
    h.send(ToWorker::Focus {
        player: 0,
        focus: Focus {
            units: vec![SCOUT],
            city: None,
        },
    });
    let mut steps = 0;
    while matches!(h.phase(), TurnPhase::Ai { .. }) {
        h.now += 1;
        let outs = h.w.step(h.now);
        h.feed(outs);
        steps += 1;
        assert_eq!(h.contexts.len(), sent, "no context during the AI phase");
        assert!(steps < 100);
    }
    h.run();
    let ids: Vec<i32> = h
        .contexts_since(0, sent)
        .iter()
        .filter_map(|c| ctx_unit(&c.2))
        .collect();
    assert_eq!(
        ids,
        vec![SCOUT],
        "the focus of the AI phase, not the default"
    );
    // Seat 1 sent none: its default focus.
    assert_eq!(h.contexts_since(1, sent).len(), 3);
}

#[test]
fn a_focus_replaces_the_default_and_holds_four_units_and_a_city() {
    let mut h = started(2, 1, false);
    let before = h.contexts.len();
    h.focus(0, &[SCOUT], Some(1));
    let core_builds = h.w.core_mut().city_builds(0, 1).expect("builds");
    let got = h.contexts_since(0, before);
    assert_eq!(got.len(), 2);
    assert_eq!(ctx_unit(&got[0].2), Some(SCOUT));
    let ContextMsg::City(c) = &got[1].2 else {
        panic!("a city context, got {:?}", got[1].2)
    };
    assert_eq!(c.city, 1);
    assert_eq!(c.builds, core_builds, "the build list is the core's");
    assert!(h.contexts_since(1, before).is_empty());
    // Six units in a focus: the first four count, the one the seat already has is not sent again.
    let before = h.contexts.len();
    h.focus(0, &[SETTLER, WORKER, 4, SCOUT, 1, 9], None);
    let ids: Vec<i32> = h
        .contexts_since(0, before)
        .iter()
        .filter_map(|c| ctx_unit(&c.2))
        .collect();
    assert_eq!(ids, vec![SETTLER, WORKER, 4], "the scout kept its context");
    // The same focus again changes nothing.
    let before = h.contexts.len();
    h.focus(0, &[SETTLER, WORKER, 4, SCOUT], None);
    assert_eq!(h.contexts.len(), before);
    // A focus for a seat that does not exist is ignored.
    h.send(ToWorker::Focus {
        player: 9,
        focus: Focus::default(),
    });
    assert!(!h.w.wants_step());
}

#[test]
fn the_focus_of_a_turn_is_over_with_the_turn_and_the_default_comes_back() {
    let mut h = started(2, 1, false);
    h.focus(0, &[SCOUT], None);
    let before = h.contexts.len();
    h.next_turn();
    h.run();
    let mut want: Vec<i32> = h
        .view(0)
        .units
        .iter()
        .filter(|u| u.owner == 0 && u.flags.contains(UnitFlags::NEEDS_ORDERS))
        .map(|u| u.id)
        .collect();
    want.sort_unstable();
    want.truncate(3);
    let mut ids: Vec<i32> = h
        .contexts_since(0, before)
        .iter()
        .filter_map(|c| ctx_unit(&c.2))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, want, "the default of the new turn");
}

#[test]
fn an_order_gets_the_context_of_its_first_focus_unit_in_the_frame_of_its_view() {
    let mut h = started(2, 1, false);
    h.focus(0, &[SCOUT], None);
    let tree = h.w.core_mut().reach(0, SCOUT, 1).expect("tree");
    let step = tree[1];
    let before = h.contexts.len();
    let id = h.cmd(
        0,
        Command::UnitMoveTo {
            unit: SCOUT,
            x: step.x,
            y: step.y,
        },
    );
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    let at = |pred: &dyn Fn(&WorkerMsg) -> bool| f.msgs.iter().position(pred);
    let view = at(&|m| matches!(m, WorkerMsg::View { player: 0, .. })).expect("a view");
    let results = at(&|m| matches!(m, WorkerMsg::Results { player: 0, .. })).expect("results");
    let ctx = at(&|m| matches!(m, WorkerMsg::Context { player: 0, .. })).expect("a context");
    assert!(
        view < results && results < ctx,
        "the order of a seat's messages"
    );
    let WorkerMsg::Context { rev, ctx, .. } = &f.msgs[ctx] else {
        unreachable!()
    };
    let ContextMsg::Unit(u) = ctx else {
        panic!("a unit context, got {ctx:?}")
    };
    assert_eq!(u.unit, SCOUT);
    assert_eq!(
        (u.reach[0].x, u.reach[0].y),
        (step.x, step.y),
        "the tree of the new place"
    );
    assert_eq!(*rev, h.clients[&0].rev);
    assert_eq!(
        h.contexts_since(0, before).len(),
        1,
        "and nothing in the background after it"
    );
    assert!(!h.w.wants_step());
    h.assert_in_sync();
}

#[test]
fn a_city_order_gets_the_city_context_in_its_frame() {
    let mut h = started(2, 1, false);
    h.focus(0, &[], Some(1));
    let builds = h.w.core_mut().city_builds(0, 1).expect("builds");
    let other = builds
        .iter()
        .find(|b| b.order_type == 0 && !b.flags.contains(protocol::CityBuildFlags::CURRENT))
        .expect("a build that is not the current one");
    let before = h.contexts.len();
    let id = h.cmd(
        0,
        Command::CitySetProduction {
            city: 1,
            order: other.order_type,
            item: other.item,
        },
    );
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let core_builds = h.w.core_mut().city_builds(0, 1).expect("builds");
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    let city = f.msgs.iter().find_map(|m| match m {
        WorkerMsg::Context {
            player: 0,
            ctx: ContextMsg::City(c),
            ..
        } => Some(c),
        _ => None,
    });
    let city = city.expect("the city context in the frame of the order");
    assert_eq!(city.builds, core_builds);
    assert_ne!(city.builds, builds, "the new production is in it");
    assert_eq!(h.contexts_since(0, before).len(), 1);
}

fn contexts_in(f: &WorkerFrame, p: PlayerId) -> Vec<&ContextMsg> {
    f.msgs
        .iter()
        .filter_map(|m| match m {
            WorkerMsg::Context { player, ctx, .. } if *player == p => Some(ctx),
            _ => None,
        })
        .collect()
}

#[test]
fn a_context_the_diff_did_not_touch_is_valid_in_the_same_frame() {
    let mut h = started(2, 1, false);
    // Send the scout far from home, where the worker is.
    for _ in 0..6 {
        h.go_far(0, SCOUT);
        if hex_distance(h.unit_pos(0, SCOUT), h.unit_pos(0, WORKER)) >= 8 {
            break;
        }
        h.next_turn();
    }
    assert!(hex_distance(h.unit_pos(0, SCOUT), h.unit_pos(0, WORKER)) >= 8);
    h.next_turn();
    h.focus(0, &[SCOUT], None);
    assert!(
        h.contexts
            .iter()
            .any(|c| c.0 == 0 && matches!(&c.2, ContextMsg::Unit(u) if u.unit == SCOUT))
    );
    let id = h.cmd(0, skip(WORKER));
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    assert_eq!(
        contexts_in(f, 0),
        vec![&ContextMsg::Valid {
            units: vec![SCOUT],
            city: None,
            view_seq: 0
        }],
        "the order is far from the scout: its context holds"
    );
    assert!(
        f.msgs
            .iter()
            .any(|m| matches!(m, WorkerMsg::View { player: 0, .. }))
    );
}

#[test]
fn a_touched_context_that_comes_out_the_same_is_valid_too() {
    let mut h = started(2, 1, false);
    h.focus(0, &[SCOUT], None);
    // The worker stands next to the scout: its record is in the scout's cells, but the tree does not change.
    let id = h.cmd(0, skip(WORKER));
    h.run();
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    assert_eq!(
        contexts_in(f, 0),
        vec![&ContextMsg::Valid {
            units: vec![SCOUT],
            city: None,
            view_seq: 0
        }]
    );
}

#[test]
fn a_unit_that_is_gone_or_cannot_act_is_dropped_once() {
    let mut h = started(2, 1, false);
    h.focus(0, &[SCOUT, 999], None);
    let dropped: Vec<i32> = h
        .contexts
        .iter()
        .filter(|c| c.0 == 0)
        .filter_map(|c| match c.2 {
            ContextMsg::Dropped { unit } => Some(unit),
            _ => None,
        })
        .collect();
    assert_eq!(dropped, vec![999], "a unit the seat does not have");
    // The scout spends all its moves: its context is dropped in the frame of the order.
    let mut id = h.go_far(0, SCOUT);
    for _ in 0..4 {
        let left = h
            .view(0)
            .units
            .iter()
            .find(|u| u.id == SCOUT && u.owner == 0);
        if left.is_none_or(|u| u.moves_left == 0) {
            break;
        }
        id = h.go_far(0, SCOUT);
    }
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    assert_eq!(
        h.view(0)
            .units
            .iter()
            .find(|u| u.id == SCOUT && u.owner == 0)
            .map(|u| u.moves_left),
        Some(0)
    );
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    assert!(f.msgs.iter().any(|m| matches!(
        m,
        WorkerMsg::Context {
            player: 0,
            ctx: ContextMsg::Dropped { unit: SCOUT },
            ..
        }
    )));
    // A seat that has been told is not told again by a later round.
    let count = h.contexts.len();
    let id = h.cmd(0, skip(WORKER));
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    assert!(
        h.contexts_since(0, count)
            .iter()
            .all(|c| ctx_unit(&c.2) != Some(SCOUT))
    );
}

#[test]
fn a_whole_view_on_request_keeps_the_contexts_valid() {
    let mut h = started(2, 1, false);
    let before = h.contexts.len();
    h.send(ToWorker::WantFull {
        player: 0,
        basis: false,
    });
    let valid: Vec<&SentContext> = h.contexts_since(0, before);
    assert_eq!(valid.len(), 1);
    assert_eq!(
        valid[0].2,
        ContextMsg::Valid {
            units: vec![SETTLER, WORKER, 4],
            city: None,
            view_seq: 0
        }
    );
    assert_eq!(
        valid[0].1, h.clients[&0].rev,
        "for the revision of the new Full"
    );
}

#[test]
fn the_seat_of_the_last_order_gets_its_background_contexts_first() {
    let mut h = started(3, 1, false);
    let before = h.contexts.len();
    h.next_turn();
    // `next_turn` ended the turn of seat 0, 1 and 2 in this order: seat 2 gave the last order.
    h.run();
    // The three orders sent a `Valid` each in their own frames; then nine background steps of the new
    // turn (the contexts come out the same as before, so they are `Valid` too).
    let seats: Vec<PlayerId> = h.contexts[before..].iter().map(|c| c.0).collect();
    assert_eq!(seats.len(), 12);
    assert_eq!(seats[3..], [2, 2, 2, 0, 1, 0, 1, 0, 1]);
}

// ---------------------------------------------------------------- N3.28: slots, peace, timer

use protocol::PeaceOffer;

fn kick(h: &mut Harness<impl Core>, player: PlayerId) {
    h.send(ToWorker::Seat {
        player,
        change: SeatChange::Kick,
    });
}

fn claim(h: &mut Harness<impl Core>, player: PlayerId, nick: &str) {
    h.send(ToWorker::Seat {
        player,
        change: SeatChange::Claim { nick: nick.into() },
    });
}

impl<C: Core> Harness<C> {
    /// The frame that carries the `SeatDone` number `n` (0 based).
    fn seat_done_frame(&self, n: usize) -> &WorkerFrame {
        self.frames
            .iter()
            .filter(|f| {
                f.msgs
                    .iter()
                    .any(|m| matches!(m, WorkerMsg::SeatDone { .. }))
            })
            .nth(n)
            .expect("a frame with SeatDone")
    }

    fn journal_has(&self, p: PlayerId, kind: JournalKind, text: &str) -> bool {
        self.clients[&p]
            .journal
            .iter()
            .any(|e| e.kind == kind && e.text.contains(text))
    }
}

use protocol::JournalKind;

#[test]
fn a_kicked_seat_is_played_by_the_ai_and_gets_no_more_views() {
    let mut h = started(3, 1, false);
    let rev = h.clients[&1].rev;
    kick(&mut h, 1);
    assert!(h.w.wants_step(), "the change waits for a step");
    h.run();
    assert_eq!(h.seat_done, vec![(1, SeatChange::Kick, None)]);
    let f = h.seat_done_frame(0);
    // The others hear it in the same frame: a `Session` line and the new turn state without the seat.
    assert!(h.journal_has(0, JournalKind::Session, "p1 left the game"));
    assert!(h.journal_has(2, JournalKind::Session, "p1 left the game"));
    assert!(!h.journal_has(1, JournalKind::Session, "left"));
    let turn = f
        .msgs
        .iter()
        .find_map(|m| match m {
            WorkerMsg::Turn(t) => Some(t),
            _ => None,
        })
        .expect("a Turn");
    assert_eq!(
        turn.seats.iter().map(|s| s.player).collect::<Vec<_>>(),
        vec![0, 2]
    );
    assert!(
        !f.msgs
            .iter()
            .any(|m| matches!(m, WorkerMsg::View { player: 1, .. }))
    );
    h.humans.retain(|p| *p != 1);
    h.assert_in_sync();
    // The game goes on without it: the turn ends when the two others end theirs.
    h.next_turn();
    assert_eq!(h.clients[&1].rev, rev, "no more views for the seat");
    h.assert_in_sync();
    // It has no seat: its orders are not taken.
    let id = h.cmd(1, skip(SCOUT));
    h.run();
    assert!(matches!(
        h.last_outcome(1, id),
        CommandOutcome::Rejected {
            code: RejectCode::NotSeated,
            ..
        }
    ));
}

#[test]
fn a_seat_change_that_cannot_be_done_is_refused_in_the_answer() {
    let mut h = started(2, 2, false);
    kick(&mut h, 3);
    claim(&mut h, 0, "thief");
    claim(&mut h, 9, "ghost");
    h.run();
    let errors: Vec<(PlayerId, bool)> = h
        .seat_done
        .iter()
        .map(|(p, _, e)| (*p, e.is_some()))
        .collect();
    assert_eq!(errors, vec![(3, true), (0, true), (9, true)]);
    assert!(
        h.clients[&0]
            .journal
            .iter()
            .all(|e| e.kind != JournalKind::Session)
    );
    h.assert_in_sync();
}

#[test]
fn a_seat_change_waits_for_the_human_phase() {
    let mut h = Harness::start_with(Opts::new(2, 1), SlowAi::new(3));
    h.run();
    h.cmd(0, end_turn());
    h.cmd(1, end_turn());
    h.run_batches_only();
    assert!(matches!(h.phase(), TurnPhase::Ai { .. }));
    kick(&mut h, 1);
    for _ in 0..20 {
        if !matches!(h.phase(), TurnPhase::Ai { .. }) {
            break;
        }
        h.now += 1;
        let outs = h.w.step(h.now);
        h.feed(outs);
        assert!(h.seat_done.is_empty(), "no seat change in the AI phase");
    }
    h.run();
    assert_eq!(h.seat_done, vec![(1, SeatChange::Kick, None)]);
    assert_eq!(h.phase(), TurnPhase::Humans);
}

#[test]
fn a_slot_is_claimed_with_a_full_view_and_the_default_contexts_and_the_others_hear_it() {
    let mut h = started(2, 2, false);
    let others = (h.clients[&0].rev, h.clients[&1].rev);
    claim(&mut h, 2, "dave");
    h.run();
    assert_eq!(
        h.seat_done,
        vec![(
            2,
            SeatChange::Claim {
                nick: "dave".into()
            },
            None
        )]
    );
    let f = h.seat_done_frame(0);
    let at = |pred: &dyn Fn(&WorkerMsg) -> bool| f.msgs.iter().position(pred);
    let basis = at(&|m| matches!(m, WorkerMsg::Basis { players, .. } if players == &vec![2]))
        .expect("a Basis");
    let full = at(&|m| {
        matches!(
            m,
            WorkerMsg::View {
                player: 2,
                update: ViewUpdate::Full { .. }
            }
        )
    })
    .expect("a Full");
    assert!(basis < full);
    let ctx: Vec<i32> = contexts_in(f, 2)
        .iter()
        .filter_map(|c| ctx_unit(c))
        .collect();
    assert_eq!(
        ctx,
        vec![SETTLER, WORKER, 4],
        "its default contexts in the same frame"
    );
    let turn = f.msgs.iter().find_map(|m| match m {
        WorkerMsg::Turn(t) => Some(t),
        _ => None,
    });
    assert_eq!(turn.map(|t| t.seats.len()), Some(3));
    for p in [0, 1] {
        assert!(h.journal_has(p, JournalKind::Session, "dave took over"));
    }
    assert!(h.journal_has(2, JournalKind::Session, "dave took over"));
    assert!(h.clients[&0].rev >= others.0 && h.clients[&1].rev >= others.1);
    // The player of the new seat is a human in the views of the others.
    h.humans.push(2);
    h.assert_in_sync();
    let id = h.cmd(2, skip(SCOUT));
    h.run();
    assert_eq!(h.last_outcome(2, id), CommandOutcome::Applied);
    // A seat that took a slot is a human of the game: the turn waits for it.
    h.cmd(0, end_turn());
    h.cmd(1, end_turn());
    h.run();
    assert_eq!(h.phase(), TurnPhase::Humans);
    h.cmd(2, end_turn());
    h.run();
    assert_eq!(h.phase(), TurnPhase::Humans);
    assert_eq!(h.w.phase().0, 2);
}

#[test]
fn a_seat_that_leaves_and_comes_back_gets_a_new_basis_and_a_full_view_that_goes_on_in_revisions() {
    let mut h = started(3, 1, false);
    kick(&mut h, 1);
    h.run();
    h.humans.retain(|p| *p != 1);
    h.next_turn();
    let (rev, fulls) = (h.clients[&1].rev, h.clients[&1].full_count);
    claim(&mut h, 1, "p1b");
    h.run();
    assert_eq!(h.seat_done.last().map(|d| d.2.clone()), Some(None));
    h.humans.push(1);
    assert!(h.clients[&1].rev > rev);
    assert_eq!(h.clients[&1].full_count, fulls + 1);
    // Its view is what a fresh build says (the data of its team was read again).
    h.assert_in_sync();
}

#[test]
fn a_kick_and_a_claim_on_the_huge_map_take_under_a_second() {
    let mut h = started(7, 2, true);
    let t = std::time::Instant::now();
    kick(&mut h, 3);
    h.run();
    claim(&mut h, 3, "late");
    h.run();
    let spent = t.elapsed();
    assert!(h.seat_done.iter().all(|d| d.2.is_none()));
    assert!(spent < std::time::Duration::from_secs(1), "took {spent:?}");
}

#[test]
fn a_forced_end_of_turn_is_taken_back_by_an_order_too() {
    let mut h = started(3, 1, false);
    h.send(ToWorker::ForceEndTurn { players: vec![0] });
    h.run();
    assert!(h.finished(0));
    let unit = h.unit_needing_orders(0).expect("unit");
    let id = h.cmd(0, skip(unit));
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    let turn = f.msgs.iter().find_map(|m| match m {
        WorkerMsg::Turn(t) => Some(t),
        _ => None,
    });
    assert_eq!(turn.map(|t| t.seats[0].finished), Some(false));
}

// peace

fn offers(h: &Harness, p: PlayerId) -> Vec<PeaceOffer> {
    h.view(p).peace_offers.clone()
}

fn at_war(h: &mut Harness, a: PlayerId, b: PlayerId) {
    let id = h.cmd(a, Command::DeclareWar { team: b });
    h.run();
    assert_eq!(h.last_outcome(a, id), CommandOutcome::Applied);
}

fn rejected_with(h: &Harness, p: PlayerId, id: CmdId, want: RejectCode) {
    match h.last_outcome(p, id) {
        CommandOutcome::Rejected { code, .. } => assert_eq!(code, want),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_peace_offer_is_in_the_views_of_both_sides_and_in_nobody_else_s() {
    let mut h = started(3, 1, false);
    at_war(&mut h, 0, 1);
    let turn = h.w.phase().0;
    let id = h.cmd(0, Command::ProposePeace { to: 1 });
    h.run();
    assert_eq!(h.last_outcome(0, id), CommandOutcome::Applied);
    let want = vec![PeaceOffer {
        from: 0,
        to: 1,
        turn,
    }];
    assert_eq!(offers(&h, 0), want);
    assert_eq!(offers(&h, 1), want);
    assert!(offers(&h, 2).is_empty());
    assert!(h.journal_has(0, JournalKind::Diplomacy, "You offered peace to p1"));
    assert!(h.journal_has(1, JournalKind::Diplomacy, "p0 offers peace"));
    assert!(!h.journal_has(2, JournalKind::Diplomacy, "peace"));
    // The answer comes after the view that shows the offer.
    let f = h.frame_of(0, id, &CommandOutcome::Applied);
    assert!(
        f.msgs
            .iter()
            .any(|m| matches!(m, WorkerMsg::View { player: 1, .. }))
    );
    h.assert_in_sync();
}

#[test]
fn the_offer_checks_refuse_what_the_spec_says() {
    let mut h = started(3, 1, false);
    // No war with seat 2: the core says peace is not possible.
    let id = h.cmd(0, Command::ProposePeace { to: 2 });
    // The AI, oneself.
    let ai = h.cmd(0, Command::ProposePeace { to: 3 });
    let me = h.cmd(0, Command::ProposePeace { to: 0 });
    // An answer to nothing.
    let none = h.cmd(
        1,
        Command::AnswerPeace {
            from: 0,
            accept: true,
        },
    );
    h.run();
    rejected_with(&h, 0, id, RejectCode::Core);
    rejected_with(&h, 0, ai, RejectCode::Invalid);
    rejected_with(&h, 0, me, RejectCode::Invalid);
    rejected_with(&h, 1, none, RejectCode::Invalid);
    // An offer between the two exists: neither side makes another.
    at_war(&mut h, 0, 1);
    h.cmd(0, Command::ProposePeace { to: 1 });
    let again = h.cmd(0, Command::ProposePeace { to: 1 });
    let back = h.cmd(1, Command::ProposePeace { to: 0 });
    h.run();
    rejected_with(&h, 0, again, RejectCode::Invalid);
    rejected_with(&h, 1, back, RejectCode::Invalid);
    assert_eq!(offers(&h, 0).len(), 1);
    // Only the one it is made to answers it.
    let wrong = h.cmd(
        0,
        Command::AnswerPeace {
            from: 1,
            accept: true,
        },
    );
    h.run();
    rejected_with(&h, 0, wrong, RejectCode::Invalid);
}

#[test]
fn an_accepted_offer_makes_peace_in_the_core_and_goes_away() {
    let mut h = started(3, 1, false);
    at_war(&mut h, 0, 1);
    h.cmd(0, Command::ProposePeace { to: 1 });
    h.run();
    let id = h.cmd(
        1,
        Command::AnswerPeace {
            from: 0,
            accept: true,
        },
    );
    h.run();
    assert_eq!(h.last_outcome(1, id), CommandOutcome::Applied);
    assert!(offers(&h, 0).is_empty() && offers(&h, 1).is_empty());
    assert!(!h.w.core_mut().can_make_peace(0, 1).expect("asked"));
    // The core journaled the peace for both.
    assert!(h.journal_has(0, JournalKind::Diplomacy, "made peace"));
    h.assert_in_sync();
}

#[test]
fn a_declined_offer_goes_away_and_the_war_goes_on() {
    let mut h = started(3, 1, false);
    at_war(&mut h, 0, 1);
    h.cmd(0, Command::ProposePeace { to: 1 });
    h.run();
    let id = h.cmd(
        1,
        Command::AnswerPeace {
            from: 0,
            accept: false,
        },
    );
    h.run();
    assert_eq!(h.last_outcome(1, id), CommandOutcome::Applied);
    assert!(offers(&h, 0).is_empty() && offers(&h, 1).is_empty());
    assert!(
        h.w.core_mut().can_make_peace(0, 1).expect("asked"),
        "still at war"
    );
    assert!(h.journal_has(0, JournalKind::Diplomacy, "p1 declined your peace offer"));
    assert!(h.journal_has(
        1,
        JournalKind::Diplomacy,
        "You declined the peace offer of p0"
    ));
    // A new offer may be made.
    let again = h.cmd(0, Command::ProposePeace { to: 1 });
    h.run();
    assert_eq!(h.last_outcome(0, again), CommandOutcome::Applied);
    h.assert_in_sync();
}

#[test]
fn an_offer_expires_at_the_start_of_the_next_human_phase() {
    let mut h = started(3, 1, false);
    at_war(&mut h, 0, 1);
    h.cmd(0, Command::ProposePeace { to: 1 });
    h.run();
    assert_eq!(offers(&h, 1).len(), 1);
    h.next_turn();
    assert!(offers(&h, 0).is_empty() && offers(&h, 1).is_empty());
    for p in [0, 1] {
        assert!(
            h.clients[&p]
                .journal
                .iter()
                .any(|e| e.kind == JournalKind::System
                    && e.text == "The peace offer of p0 to p1 expired"),
            "seat {p} hears that it expired"
        );
    }
    assert!(
        !h.clients[&2]
            .journal
            .iter()
            .any(|e| e.kind == JournalKind::System)
    );
    h.assert_in_sync();
}

#[test]
fn the_offers_of_a_seat_that_is_kicked_are_void() {
    let mut h = started(3, 1, false);
    at_war(&mut h, 0, 1);
    h.cmd(0, Command::ProposePeace { to: 1 });
    h.run();
    kick(&mut h, 1);
    h.run();
    h.humans.retain(|p| *p != 1);
    assert!(offers(&h, 0).is_empty());
    assert!(h.journal_has(0, JournalKind::Diplomacy, "is void"));
    h.assert_in_sync();
}

// ---------------------------------------------------------------- contexts and seat changes over random play

/// What the client holds of the unit contexts: `Unit` replaces, `Dropped` removes, `Valid` changes nothing.
type Held = BTreeMap<(PlayerId, i32), Option<protocol::UnitContext>>;

fn absorb_contexts(h: &Harness, cursor: &mut usize, held: &mut Held) {
    for (p, _, ctx, _) in &h.contexts[*cursor..] {
        match ctx {
            ContextMsg::Unit(u) => {
                held.insert((*p, u.unit), Some(u.clone()));
            }
            ContextMsg::Dropped { unit } => {
                held.insert((*p, *unit), None);
            }
            ContextMsg::Valid { .. } | ContextMsg::City(_) => {}
        }
    }
    *cursor = h.contexts.len();
}

/// The first three units of `p` that need orders, by id.
fn default_focus(h: &Harness, p: PlayerId) -> Vec<i32> {
    let mut ids: Vec<i32> = h
        .view(p)
        .units
        .iter()
        .filter(|u| u.owner == p && u.flags.contains(UnitFlags::NEEDS_ORDERS))
        .map(|u| u.id)
        .collect();
    ids.sort_unstable();
    ids.truncate(3);
    ids
}

/// Every context the client holds for the wanted units says what the core says now. Returns how many were
/// compared.
fn contexts_agree_with_the_core(
    h: &mut Harness,
    wanted: &BTreeMap<PlayerId, Vec<i32>>,
    held: &Held,
) -> usize {
    let mut compared = 0;
    for (&p, ids) in wanted {
        for &id in ids {
            let ctx = held
                .get(&(p, id))
                .unwrap_or_else(|| panic!("no context was ever sent for unit {id} of seat {p}"));
            match ctx {
                Some(c) => {
                    let core = h.w.core_mut();
                    assert_eq!(
                        c.reach,
                        core.reach(p, id, 1).expect("reach"),
                        "tree of {p}/{id}"
                    );
                    assert_eq!(
                        c.actions,
                        core.unit_actions(p, id).expect("actions"),
                        "actions of {p}/{id}"
                    );
                    let shots: Vec<(i16, i16)> = core
                        .targets(p, protocol::Shooter::Unit(id))
                        .expect("targets")
                        .into_iter()
                        .map(|(x, y, _)| (x, y))
                        .collect();
                    let held_shots: Vec<(i16, i16)> = c
                        .targets
                        .iter()
                        .filter(|t| t.kind == protocol::TargetKind::Ranged)
                        .map(|t| (t.x, t.y))
                        .collect();
                    assert_eq!(held_shots, shots, "targets of {p}/{id}");
                    compared += 1;
                }
                None => {
                    let unit = h.view(p).units.iter().find(|u| u.owner == p && u.id == id);
                    assert!(
                        unit.is_none_or(|u| !u.flags.contains(UnitFlags::CAN_MOVE)),
                        "unit {p}/{id} was dropped but can act"
                    );
                }
            }
        }
    }
    compared
}

#[test]
fn the_contexts_a_client_holds_are_what_the_core_says_over_random_play_with_seat_changes() {
    let mut h = audited(4, 2, false);
    h.run();
    let mut rng = Rng(2024);
    let (mut held, mut cursor, mut compared) = (Held::new(), 0, 0);
    let mut away: Option<PlayerId> = None;
    for turn_no in 0..40 {
        // Seat changes happen at the start of a turn, before the orders.
        if turn_no % 6 == 2 {
            let p = h.humans[rng.below(h.humans.len() as u64) as usize];
            kick(&mut h, p);
            h.run();
            h.humans.retain(|x| *x != p);
            away = Some(p);
        } else if turn_no % 6 == 5
            && let Some(p) = away.take()
        {
            claim(&mut h, p, "back");
            h.run();
            h.humans.push(p);
            h.humans.sort_unstable();
        }
        // The default focus is taken once a turn: the same units all turn long.
        let defaults: BTreeMap<PlayerId, Vec<i32>> = h
            .humans
            .iter()
            .map(|&p| (p, default_focus(&h, p)))
            .collect();
        for _ in 0..4 {
            absorb_contexts(&h, &mut cursor, &mut held);
            compared += contexts_agree_with_the_core(&mut h, &defaults, &held);
            for p in h.humans.clone() {
                for _ in 0..3 {
                    let cmd = h.random_cmd(p, &mut rng);
                    if !matches!(cmd, Command::EndTurn { .. } | Command::CancelEndTurn) {
                        h.cmd(p, cmd);
                    }
                }
            }
            h.run();
        }
        absorb_contexts(&h, &mut cursor, &mut held);
        compared += contexts_agree_with_the_core(&mut h, &defaults, &held);
        let t = h.w.phase().0;
        for p in h.humans.clone() {
            h.cmd(p, end_turn());
        }
        h.run();
        assert_eq!(h.w.phase().0, t + 1);
    }
    assert!(compared > 200, "compared {compared}");
    assert_eq!(h.w.fog_violations(), 0);
    h.assert_in_sync();
}

// ---------------------------------------------------------------- the timeline of the end of a turn, the AI progress

#[test]
fn the_end_of_a_turn_leaves_its_timeline() {
    use protocol::{Codec, Postcard};
    let mut h = Harness::start_with(Opts::new(2, 2), SlowAi::new(3));
    h.run();
    h.next_turn();
    let traces = h.w.closed_traces();
    let t = traces
        .iter()
        .find(|t| t.turn == 1)
        .expect("the trace of turn 1");
    let at = |name: &str| {
        t.recs
            .iter()
            .position(|r| r.span == name)
            .unwrap_or(usize::MAX)
    };
    for name in ["w.end_turn", "w.apply", "w.ai", "w.turn_change"] {
        assert!(
            at(name) != usize::MAX,
            "{name} is in the trace: {:?}",
            t.recs
        );
    }
    // In the order the worker makes them: the order, the AI, the turn change.
    assert!(at("w.end_turn") < at("w.ai") && at("w.ai") < at("w.turn_change"));
    // Two humans: two ends of turn, each with the number of its order.
    let ids: Vec<i64> = t.all("w.end_turn").filter_map(|r| r.arg("cmd")).collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    let ai = t.find("w.ai").expect("w.ai");
    assert!(
        ai.arg("calls").is_some_and(|c| c >= 2),
        "the AI phase took several pumps: {ai:?}"
    );
    // The same trace went to the gateway.
    let sent: Vec<&Vec<u8>> = h
        .frames
        .iter()
        .flat_map(|f| f.msgs.iter())
        .filter_map(|m| match m {
            WorkerMsg::Trace { turn: 1, bytes } => Some(bytes),
            _ => None,
        })
        .collect();
    assert_eq!(sent.len(), 1);
    let back: turntrace::TurnTrace = Postcard.decode(sent[0]).expect("decodes");
    assert_eq!(&back, t);
    // The summary adds up.
    let s = turntrace::summarize(&back);
    assert_eq!(
        s.wait_others_us + s.order_us + s.server_ai_us + s.server_post_us + s.other_us,
        s.total_us
    );
}

#[test]
fn the_ai_phase_in_the_order_step_is_booked_as_ai_time() {
    let mut core = SlowAi::new(3);
    core.sleep_ms = 4;
    let mut h = Harness::start_with(
        Opts {
            cfg: WorkerConfig {
                stats: true,
                ..WorkerConfig::default()
            },
            ..Opts::new(2, 2)
        },
        core,
    );
    h.run();
    h.stats.clear();
    h.next_turn();
    // The step of the last END_TURN pumps the AI: it is an AI step with the AI's time (it was an order step with 0).
    let first_ai = h
        .stats
        .iter()
        .position(|s| s.kind == protocol::StepKind::AiPhase)
        .expect("an AI step");
    assert!(h.stats[first_ai].ai_us >= 3_000, "{:?}", h.stats[first_ai]);
    assert!(
        h.stats[..first_ai].iter().all(|s| s.ai_us == 0),
        "no AI time before it"
    );
    let total: u64 = h.stats.iter().map(|s| u64::from(s.ai_us)).sum();
    assert!(total >= 12_000, "four pumps of 4 ms: {total}");
    let change = h
        .stats
        .iter()
        .position(|s| s.kind == protocol::StepKind::TurnChange)
        .expect("the step that gives the turn back closes the phase");
    assert!(change > first_ai);
    assert!(
        h.stats[first_ai..change]
            .iter()
            .all(|s| s.kind == protocol::StepKind::AiPhase),
        "AI steps in between"
    );
    // The heartbeat of a long order step says AI, not Humans.
    assert!(matches!(h.w.watch_phase(), TurnPhase::Humans));
}

#[test]
fn the_ai_turn_queue_starts_every_ai_at_once_and_ends_them_one_by_one() {
    // SIMP-exe-820: a burst of Started for every non-human civilization when the AI phase begins, then an Ended per
    // civilization (ascending, the barbarians last), each in the frame of the pump call that did it.
    let mut h = Harness::start_with(Opts::new(2, 3), SlowAi::new(6));
    h.run();
    h.next_turn();
    let events: Vec<(usize, i32, protocol::AiTurnEvent)> = h
        .frames
        .iter()
        .enumerate()
        .flat_map(|(i, f)| {
            f.msgs.iter().filter_map(move |m| match m {
                WorkerMsg::AiTurn { turn, events } => Some((i, *turn, events.clone())),
                _ => None,
            })
        })
        .flat_map(|(i, t, v)| v.into_iter().map(move |e| (i, t, e)))
        .collect();
    assert!(events.iter().all(|e| e.1 == 1), "{events:?}");
    let started: Vec<_> = events.iter().filter(|e| !e.2.ended).collect();
    let ended: Vec<_> = events.iter().filter(|e| e.2.ended).collect();
    assert_eq!(
        started.len(),
        4,
        "3 AI majors and the barbarians: {events:?}"
    );
    assert_eq!(ended.len(), 4, "{events:?}");
    // Everything is started before the first one ends.
    let last_start = events.iter().rposition(|e| !e.2.ended).unwrap();
    let first_end = events.iter().position(|e| e.2.ended).unwrap();
    assert!(last_start < first_end, "{events:?}");
    // Humans are not part of it; the ends are ascending by id, the barbarians (63) last.
    let ids: Vec<i8> = ended.iter().map(|e| e.2.player).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "ascending");
    assert_eq!(ids.last(), Some(&63));
    assert!(ids.iter().all(|p| !h.humans.contains(p)));
    let mut s_ids: Vec<i8> = started.iter().map(|e| e.2.player).collect();
    s_ids.sort_unstable();
    assert_eq!(
        s_ids, sorted,
        "the same civilizations are started and ended"
    );
    // The Ended of the last AI is in an earlier frame than the one that gave the turn back (the humans' Turn message).
    let turn_back = h
        .frames
        .iter()
        .position(|f| {
            f.msgs.iter().any(
                |m| matches!(m, WorkerMsg::Turn(t) if t.turn == 2 && t.phase == TurnPhase::Humans),
            )
        })
        .expect("the turn comes back to the humans");
    assert!(ended.last().unwrap().0 <= turn_back);
}
