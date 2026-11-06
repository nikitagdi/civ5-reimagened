//! Saves and restarts of the multiplayer worker on the test world (game-host-mp 4.11, 4.12, section 9
//! "сейвы" and "перезапуск"): `Worker<FixtureCore>` and the jobs it hands to the shell, and a second worker
//! that continues from the file. No I/O, no clock: time is a number the test passes; the "shell" here
//! packs the job with `saves::encode` the way the save thread of the server does.

use std::collections::BTreeMap;

use game_host::fixture::FixtureCore;
use game_host::saves::{self, SaveError};
use game_host::{Out, SaveDone, SaveJob, Worker};
use protocol::{
    AutoChoices, Cmd, Command, CoreKind, GameSettings, GameSetup, IPC_VERSION, JournalKind,
    PlayerId, SaveKind, SaveReason, SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnMode,
    TurnPhase, ViewUpdate, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg,
};

const MIN: u64 = 60_000;

fn settings() -> GameSettings {
    GameSettings {
        name: "saves".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "fixture.lua".into(),
        map_options: Vec::new(),
        speed: "GAMESPEED_STANDARD".into(),
        start_era: "ERA_ANCIENT".into(),
        handicap: "HANDICAP_PRINCE".into(),
        minor_civs: 0,
        turn_mode: TurnMode::Simultaneous,
        timer: TimerMode::Off,
        game_options: Vec::new(),
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
        mods: Vec::new(),
    }
}

fn slots(humans: usize, ais: usize) -> Vec<SlotSetup> {
    let mut v = Vec::new();
    for i in 0..humans {
        v.push(SlotSetup {
            player: i as i8,
            civ: protocol::CivPick::Random,
            control: SlotControl::Human {
                nick: format!("p{i}"),
            },
        });
    }
    for i in 0..ais {
        v.push(SlotSetup {
            player: (humans + i) as i8,
            civ: protocol::CivPick::Random,
            control: SlotControl::Ai,
        });
    }
    v
}

fn init(start: Start, save_dir: &str, cfg: WorkerConfig) -> WorkerInit {
    WorkerInit {
        ipc: IPC_VERSION,
        game: 7,
        core: CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        start,
        save_dir: save_dir.into(),
        seed: 0x1234_5678_9abc,
        cfg,
    }
}

/// The worker and what came out of it.
struct H {
    w: Worker<FixtureCore>,
    now: u64,
    jobs: Vec<SaveJob>,
    frames: Vec<WorkerFrame>,
    exit: Option<u8>,
    next_id: BTreeMap<PlayerId, u32>,
}

impl H {
    fn new(start: Start, save_dir: &str, cfg: WorkerConfig) -> Self {
        let (w, outs) = Worker::new(
            FixtureCore::new(false, false),
            init(start, save_dir, cfg),
            0,
        );
        Self::with(w, outs)
    }

    fn with(w: Worker<FixtureCore>, outs: Vec<Out>) -> Self {
        let mut h = H {
            w,
            now: 0,
            jobs: Vec::new(),
            frames: Vec::new(),
            exit: None,
            next_id: BTreeMap::new(),
        };
        h.feed(outs);
        h
    }

    fn game(humans: usize, ais: usize) -> Self {
        let setup = GameSetup {
            settings: settings(),
            slots: slots(humans, ais),
        };
        let mut h = H::new(Start::New(setup), "saves", WorkerConfig::default());
        h.run();
        h
    }

    fn feed(&mut self, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Frame(f) => {
                    for m in &f.msgs {
                        if let WorkerMsg::Fatal { message, code } = m {
                            panic!("fatal {code}: {message}");
                        }
                    }
                    self.frames.push(f);
                }
                Out::Save(j) => self.jobs.push(*j),
                Out::Replica(_) => {}
                Out::Exit(c) => self.exit = Some(c),
            }
        }
    }

    fn run(&mut self) {
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

    fn send(&mut self, msg: ToWorker) {
        self.now += 1;
        let outs = self.w.handle(msg, self.now);
        self.feed(outs);
    }

    /// A step at the given time (the clock jumped).
    fn step_at(&mut self, at: u64) {
        self.now = at;
        let outs = self.w.step(at);
        self.feed(outs);
    }

    fn cmd(&mut self, player: PlayerId, cmd: Command) {
        let turn = self.w.phase().0;
        let n = self.next_id.entry(player).or_insert(0);
        *n += 1;
        let id = *n;
        self.send(ToWorker::Cmd {
            player,
            cmd: Cmd { id, turn, cmd },
        });
    }

    fn end_turn(&mut self, player: PlayerId) {
        self.cmd(player, Command::EndTurn { force: false });
    }

    /// Everything the human seats do to end the turn, then the AI and the new turn.
    fn next_turn(&mut self, humans: usize) {
        let turn = self.w.phase().0;
        for p in 0..humans {
            self.end_turn(p as i8);
        }
        self.run();
        assert_eq!(self.w.phase(), (turn + 1, TurnPhase::Humans));
    }

    fn fortify_first_unit(&mut self, player: PlayerId) {
        let v = self.w.fresh_view(player).unwrap();
        let unit = v
            .units
            .iter()
            .find(|u| u.owner == player)
            .expect("an own unit")
            .id;
        self.cmd(player, Command::UnitFortify { unit });
    }

    /// The job the shell would write, packed and read back.
    fn written(&self, job: &SaveJob, at: u64) -> Vec<u8> {
        let mut header = job.header.clone();
        header.saved_at_ms = at;
        saves::encode(&header, &job.gcs1, &job.sections)
    }

    fn done(&mut self, job: &SaveJob, size: u64) {
        let d = SaveDone {
            file: job.file,
            saved_at_ms: 1_000 + job.file.seq,
            result: Ok(size),
        };
        self.now += 1;
        let outs = self.w.save_done(d, self.now);
        self.feed(outs);
    }

    fn msgs(&self) -> impl Iterator<Item = &WorkerMsg> {
        self.frames.iter().flat_map(|f| f.msgs.iter())
    }
}

// ---------------------------------------------------------------- when

#[test]
fn a_save_is_made_at_the_start_of_the_human_phase_after_the_views() {
    let h = H::game(2, 1);
    assert_eq!(h.jobs.len(), 1, "one save at the start of the game");
    let j = &h.jobs[0];
    assert_eq!(
        (j.file.seq, j.file.turn, j.file.kind),
        (1, 1, SaveKind::Ring)
    );
    assert_eq!(j.file.rel_path(), "ring/00000001-t0001.c5sv");
    assert_eq!(j.header.build_id, "fixture");
    assert_eq!(j.header.game, 7);
    assert_eq!(j.ring, 10);
    assert!(j.gcs1.starts_with(b"FIXT"));
    assert_eq!(j.sections.seats.len(), 3);
    assert_eq!(j.sections.settings.name, "saves");
    // The job goes out after the frame that holds the first views.
    assert!(h.frames.iter().flat_map(|f| &f.msgs).any(|m| matches!(
        m,
        WorkerMsg::View {
            update: ViewUpdate::Full { .. },
            ..
        }
    )));
}

#[test]
fn every_turn_saves_and_every_tenth_goes_to_keep() {
    let mut h = H::game(2, 1);
    for _ in 0..10 {
        h.next_turn(2);
    }
    let files: Vec<(u64, i32, SaveKind)> = h
        .jobs
        .iter()
        .map(|j| (j.file.seq, j.file.turn, j.file.kind))
        .collect();
    assert_eq!(files.len(), 11);
    for (i, (seq, turn, kind)) in files.iter().enumerate() {
        assert_eq!((*seq, *turn), (i as u64 + 1, i as i32 + 1));
        let want = if turn % 10 == 0 {
            SaveKind::Keep
        } else {
            SaveKind::Ring
        };
        assert_eq!(*kind, want, "turn {turn}");
    }
    assert_eq!(h.jobs[9].file.rel_path(), "keep/00000010-t0010.c5sv");
}

#[test]
fn the_save_of_the_turn_start_comes_before_the_queued_orders_are_applied() {
    let mut h = H::game(2, 1);
    let turn = h.w.phase().0;
    // Both seats end the turn; seat 0 sends its next-turn order while the AI moves.
    h.end_turn(0);
    h.end_turn(1);
    // One step: the orders are applied, the AI phase begins.
    h.now += 1;
    let outs = h.w.step(h.now);
    h.feed(outs);
    assert!(
        matches!(h.w.phase().1, TurnPhase::Ai { .. }),
        "the AI moves"
    );
    let before = h.jobs.len();
    h.cmd_for_turn(0, turn + 1);
    h.run();
    // The turn-start save is there, and the order of the new turn came after it.
    assert_eq!(h.jobs.len(), before + 1);
    let j = h.jobs.last().unwrap();
    assert_eq!(j.file.turn, turn + 1);
    let applied_in_save = j
        .sections
        .cmds
        .iter()
        .find(|(p, _)| *p == 0)
        .map(|(_, id)| *id);
    assert_eq!(
        applied_in_save,
        Some(1),
        "only the end of turn of the old turn is in the save"
    );
}

impl H {
    fn cmd_for_turn(&mut self, player: PlayerId, turn: i32) {
        let n = self.next_id.entry(player).or_insert(0);
        *n += 1;
        let id = *n;
        let unit = self
            .w
            .fresh_view(player)
            .ok()
            .and_then(|v| v.units.iter().find(|u| u.owner == player).map(|u| u.id))
            .unwrap_or(1);
        self.send(ToWorker::Cmd {
            player,
            cmd: Cmd {
                id,
                turn,
                cmd: Command::UnitFortify { unit },
            },
        });
    }
}

#[test]
fn a_save_every_sixty_seconds_when_an_order_was_applied() {
    let mut h = H::game(2, 1);
    let n = h.jobs.len();
    // No order: no save, however long it takes.
    h.step_at(10 * MIN);
    assert_eq!(h.jobs.len(), n);
    // A save on request, then an order: the time since the last save is short.
    h.now = 10 * MIN;
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let n = n + 1;
    assert_eq!(h.jobs.len(), n);
    let saved_at = h.now;
    h.fortify_first_unit(0);
    h.run();
    h.step_at(saved_at + 30_000);
    assert_eq!(h.jobs.len(), n, "the minute is not over");
    // The worker asks to be woken when it is.
    let wake = h.w.next_wake_ms().unwrap();
    assert!(wake <= saved_at + MIN, "wake at {wake}");
    h.step_at(saved_at + MIN + 1);
    assert_eq!(h.jobs.len(), n + 1);
    let j = h.jobs.last().unwrap();
    assert_eq!(j.file.kind, SaveKind::Ring);
    assert_eq!(j.file.turn, 1);
    assert!(j.sections.cmds.iter().any(|(p, id)| *p == 0 && *id == 1));
    // Nothing new, nothing saved.
    h.step_at(30 * MIN);
    assert_eq!(h.jobs.len(), n + 1);
}

#[test]
fn save_now_saves_in_the_safe_point_and_the_record_is_told_when_it_is_written() {
    let mut h = H::game(2, 1);
    h.fortify_first_unit(1);
    h.run();
    let n = h.jobs.len();
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    assert_eq!(h.jobs.len(), n + 1);
    let job = h.jobs.last().unwrap().clone();
    assert_eq!(job.file.seq, 2);
    h.done(&job, 1234);
    let rec = h
        .msgs()
        .find_map(|m| match m {
            WorkerMsg::Saved(r) => Some(r.clone()),
            _ => None,
        })
        .expect("Saved");
    // The first save of the game (the start of the turn) was not announced: nobody told the worker.
    assert_eq!(rec.meta.seq, 2);
    assert_eq!(rec.meta.turn, 1);
    assert_eq!(rec.meta.saved_at_ms, 1_002);
    assert_eq!(rec.file, "ring/00000002-t0001.c5sv");
    assert_eq!(rec.kind, SaveKind::Ring);
    assert_eq!(rec.applied_upto, vec![(1, 1)]);
}

#[test]
fn a_save_that_is_not_written_is_a_failure_and_a_line_for_every_seat() {
    let mut h = H::game(2, 1);
    let job = h.jobs[0].clone();
    let before = h.frames.len();
    let d = SaveDone {
        file: job.file,
        saved_at_ms: 5,
        result: Err("disk full".into()),
    };
    let outs = h.w.save_done(d, 1);
    h.feed(outs);
    let new: Vec<&WorkerMsg> = h.frames[before..].iter().flat_map(|f| &f.msgs).collect();
    assert!(
        new.iter()
            .any(|m| matches!(m, WorkerMsg::SaveFailed { reason } if reason.contains("disk full")))
    );
    for p in 0..2i8 {
        assert!(
            new.iter().any(|m| matches!(m,
                WorkerMsg::Journal { player, entries }
                    if *player == p && entries.iter().any(|e| e.kind == JournalKind::System && e.text.contains("disk full")))),
            "a System line for seat {p}"
        );
    }
    assert_eq!(h.exit, None, "the game goes on");
    assert!(!new.iter().any(|m| matches!(m, WorkerMsg::Saved(_))));
}

// ---------------------------------------------------------------- shutdown

#[test]
fn shutdown_with_save_saves_then_exits_with_zero_once_the_file_is_written() {
    let mut h = H::game(2, 1);
    let n = h.jobs.len();
    h.send(ToWorker::Shutdown { save: true });
    assert_eq!(h.jobs.len(), n + 1);
    assert_eq!(h.exit, None, "the file is not written yet");
    let job = h.jobs.last().unwrap().clone();
    h.done(&job, 10);
    assert_eq!(h.exit, Some(0));
    assert!(
        h.msgs()
            .any(|m| matches!(m, WorkerMsg::Saved(r) if r.meta.seq == job.file.seq))
    );
}

#[test]
fn shutdown_without_save_exits_at_once() {
    let mut h = H::game(2, 1);
    let n = h.jobs.len();
    h.send(ToWorker::Shutdown { save: false });
    assert_eq!((h.exit, h.jobs.len()), (Some(0), n));
}

#[test]
fn shutdown_in_the_ai_phase_waits_for_the_next_safe_point() {
    let mut h = H::game(2, 1);
    h.end_turn(0);
    h.end_turn(1);
    h.now += 1;
    let outs = h.w.step(h.now);
    h.feed(outs);
    assert!(matches!(h.w.phase().1, TurnPhase::Ai { .. }));
    let n = h.jobs.len();
    h.send(ToWorker::Shutdown { save: true });
    assert_eq!(h.jobs.len(), n, "no safe point in the AI phase");
    assert!(h.w.wants_step());
    h.run();
    // The save of the new turn's start is the final one too: it is written, then the worker ends.
    assert_eq!(h.jobs.len(), n + 1);
    assert_eq!(h.jobs.last().unwrap().file.turn, 2);
    assert_eq!(h.exit, None);
    let job = h.jobs.last().unwrap().clone();
    h.done(&job, 10);
    assert_eq!(h.exit, Some(0));
}

#[test]
fn a_shutdown_that_finds_no_safe_point_in_five_seconds_ends_without_a_save() {
    let mut h = H::game(2, 1);
    h.end_turn(0);
    h.end_turn(1);
    h.now += 1;
    let outs = h.w.step(h.now);
    h.feed(outs);
    assert!(matches!(h.w.phase().1, TurnPhase::Ai { .. }));
    let n = h.jobs.len();
    h.send(ToWorker::Shutdown { save: true });
    let at = h.now + 5_001;
    h.step_at(at);
    assert_eq!((h.exit, h.jobs.len()), (Some(0), n));
}

#[test]
fn an_empty_save_dir_turns_the_saves_off() {
    let setup = GameSetup {
        settings: settings(),
        slots: slots(2, 1),
    };
    let mut h = H::new(Start::New(setup), "", WorkerConfig::default());
    h.run();
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    h.step_at(100 * MIN);
    assert!(h.jobs.is_empty());
    assert!(h.msgs().any(|m| matches!(m, WorkerMsg::SaveFailed { .. })));
    h.send(ToWorker::Shutdown { save: true });
    assert_eq!(h.exit, Some(0));
}

#[test]
fn the_seq_goes_on_above_the_files_the_shell_found() {
    let setup = GameSetup {
        settings: settings(),
        slots: slots(2, 1),
    };
    let (mut w, outs) = Worker::new(
        FixtureCore::new(false, false),
        init(Start::New(setup), "saves", WorkerConfig::default()),
        0,
    );
    w.set_seq_floor(41);
    let mut h = H::with(w, outs);
    h.run();
    assert_eq!(h.jobs[0].file.seq, 42);
}

// ---------------------------------------------------------------- restart

fn load_init(file: &str, seats: Vec<SlotSetup>) -> WorkerInit {
    init(
        Start::Load {
            file: file.into(),
            seats,
        },
        "saves",
        WorkerConfig::default(),
    )
}

/// A worker that continues from the job, as the shell does: pack, read, `resume`.
fn restarted(h: &H, job: &SaveJob, seats: Vec<SlotSetup>) -> H {
    let bytes = h.written(job, 99);
    let save = saves::decode(&bytes).expect("the file reads");
    let (w, outs) = Worker::resume(
        FixtureCore::new(false, false),
        load_init("ring/x.c5sv", seats),
        save,
        500,
    );
    let mut r = H::with(w, outs);
    r.now = 500;
    r.run();
    r
}

#[test]
fn a_restart_gives_every_seat_the_view_hash_it_had_at_the_save() {
    let mut h = H::game(2, 1);
    for _ in 0..3 {
        h.next_turn(2);
    }
    // Mid-turn: seat 0 has ended its turn, seat 1 has given an order.
    h.fortify_first_unit(1);
    h.end_turn(0);
    h.run();
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Shutdown,
    });
    let job = h.jobs.last().unwrap().clone();
    let hashes: Vec<Option<u64>> = (0..2).map(|p| h.w.view_hash(p)).collect();
    assert!(hashes.iter().all(Option::is_some));

    let mut r = restarted(&h, &job, slots(2, 1));
    assert_eq!(r.w.phase(), h.w.phase());
    for p in 0..2i8 {
        assert_eq!(r.w.view_hash(p), hashes[p as usize], "seat {p}");
        let fresh = r.w.fresh_view(p).unwrap();
        assert_eq!(
            protocol::ViewHash::of(&fresh).value(),
            hashes[p as usize].unwrap()
        );
    }
    // The first frames: Ready with the seats, a Basis and a Full for each seat, the journals with them.
    let ready = r
        .msgs()
        .find_map(|m| match m {
            WorkerMsg::Ready(r) => Some(r.clone()),
            _ => None,
        })
        .expect("Ready");
    assert_eq!(ready.turn, 4);
    assert_eq!(ready.seats.len(), 3);
    for p in 0..2i8 {
        assert!(r.msgs().any(|m| matches!(m, WorkerMsg::View { player, update: ViewUpdate::Full { .. } } if *player == p)));
    }
    assert!(
        r.msgs()
            .any(|m| matches!(m, WorkerMsg::Basis { players, .. } if players.len() == 2))
    );
    // No save at the start of a loaded game; the numbers go on above the loaded one.
    assert!(r.jobs.is_empty(), "the loaded save is the start save");
    r.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    assert_eq!(r.jobs.len(), 1);
    assert_eq!(r.jobs[0].file.seq, job.file.seq + 1);
    // The orders already applied are in the next save too (`CMDS`), the settings and the seats as well.
    assert_eq!(r.jobs[0].sections.cmds, job.sections.cmds);
    assert_eq!(r.jobs[0].sections.settings, job.sections.settings);
    assert_eq!(r.jobs[0].sections.seats, job.sections.seats);
    // The state of the core is the same: the next save of it is the same bytes.
    assert_eq!(r.jobs[0].gcs1, job.gcs1);
}

#[test]
fn the_journals_of_the_seats_come_back_with_a_line_for_the_restart() {
    let mut h = H::game(2, 1);
    for _ in 0..2 {
        h.next_turn(2);
    }
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    let r = restarted(&h, &job, slots(2, 1));
    for p in 0..2i8 {
        let mut got: Vec<protocol::JournalEntry> = Vec::new();
        for m in r.msgs() {
            if let WorkerMsg::Journal { player, entries } = m
                && *player == p
            {
                got.extend(entries.iter().cloned());
            }
        }
        let mine = &job.sections.journals.iter().find(|(q, _)| *q == p);
        let saved = mine.map(|(_, e)| e.clone()).unwrap_or_default();
        assert_eq!(
            &got[..saved.len()],
            &saved[..],
            "seat {p}: the saved lines come first"
        );
        assert!(got.iter().any(|e| e.kind == JournalKind::System
            && e.text.contains("continues from the save of turn 3")));
    }
}

#[test]
fn the_gateways_seats_are_the_truth_after_a_load() {
    let mut h = H::game(2, 1);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    // The gateway says: seat 1 is the AI now, the AI slot 2 is held by "zed".
    let mut seats = slots(1, 2);
    seats[2].control = SlotControl::Human { nick: "zed".into() };
    let mut r = restarted(&h, &job, seats);
    assert!(r.w.view_hash(0).is_some());
    assert!(
        r.w.view_hash(1).is_none(),
        "seat 1 has no view: it is the AI's"
    );
    assert!(r.w.view_hash(2).is_some(), "zed has one");
    r.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let sections = &r.jobs[0].sections;
    let control = |p: i8| {
        sections
            .seats
            .iter()
            .find(|s| s.player == p)
            .unwrap()
            .control
            .clone()
    };
    assert_eq!(control(1), SlotControl::Ai);
    assert_eq!(control(2), SlotControl::Human { nick: "zed".into() });
}

#[test]
fn humans_who_are_off_line_only_because_of_the_load_are_not_connected() {
    // Decision 2026-10-27 (gc-abi-v2 4.9): a load does not CONNECT the humans the save already holds; they
    // wait for their login. The turn waits for both as before (nobody plays for them).
    let mut h = H::game(2, 1);
    h.next_turn(2);
    h.end_turn(0);
    h.run();
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    let mut r = restarted(&h, &job, slots(2, 1));
    assert!(
        r.w.core_mut().controls_set().is_empty(),
        "no CONNECT for a human the save holds: {:?}",
        r.w.core_mut().controls_set()
    );
    // Seat 0 had ended its turn, seat 1 had not; both seats are up and the turn still waits for seat 1.
    assert!(r.w.view_hash(0).is_some() && r.w.view_hash(1).is_some());
    assert_eq!(r.w.phase(), h.w.phase());
    r.run();
    assert_eq!(
        r.w.phase(),
        h.w.phase(),
        "seat 1 has not played: the turn waits"
    );
    r.end_turn(1);
    r.run();
    assert!(r.w.phase().0 > h.w.phase().0, "both ended: the next turn");
}

#[test]
fn a_slot_that_changes_owner_with_the_load_is_still_handed_over() {
    let mut h = H::game(2, 1);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    // The gateway: seat 0 as saved, seat 1 the AI now, AI slot 2 taken by "zed".
    let mut seats = slots(1, 2);
    seats[2].control = SlotControl::Human { nick: "zed".into() };
    let mut r = restarted(&h, &job, seats);
    let calls = r.w.core_mut().controls_set().to_vec();
    // The connect first: a kick leaves the core unsafe until the AI has played (the live load of a save
    // whose human slot went to the AI while another slot was taken died on "not at a safe point").
    assert_eq!(
        calls,
        vec![
            (2, SlotControl::Human { nick: "zed".into() }),
            (1, SlotControl::Ai)
        ]
    );
    assert!(r.w.view_hash(2).is_some() && r.w.view_hash(1).is_none());
}

#[test]
fn several_kicks_after_a_load_wait_for_the_ai_between_them() {
    let mut h = H::game(2, 1);
    h.next_turn(2);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    // The loader took the third slot; both humans of the save become AI (as in the live game 25, where the
    // player took the other civilization): two kicks and a connect.
    let mut seats = slots(0, 3);
    seats[2].control = SlotControl::Human { nick: "zed".into() };
    let mut r = restarted(&h, &job, seats);
    assert!(
        !r.msgs().any(|m| matches!(m, WorkerMsg::Fatal { .. })),
        "the load did not fail"
    );
    let calls = r.w.core_mut().controls_set().to_vec();
    assert_eq!(
        calls,
        vec![
            (2, SlotControl::Human { nick: "zed".into() }),
            (0, SlotControl::Ai),
            (1, SlotControl::Ai)
        ]
    );
    assert_eq!(r.w.phase().1, TurnPhase::Humans);
    assert!(r.w.view_hash(2).is_some(), "zed has a view");
    assert!(r.w.view_hash(0).is_none() && r.w.view_hash(1).is_none());
}

#[test]
fn a_load_waits_for_a_safe_point_before_it_hands_a_slot_over() {
    let mut h = H::game(2, 1);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    let save = saves::decode(&h.written(&job, 99)).expect("the file reads");
    // The core is not at a safe point after the load (a human of the save is in auto moves): seat 1 is given
    // to the AI all the same, after a pump, and the worker does not die on the first refusal.
    let (w, outs) = Worker::resume(
        FixtureCore::new(false, false).unsafe_after_load(),
        load_init("ring/x.c5sv", slots(1, 2)),
        save,
        500,
    );
    let mut r = H::with(w, outs);
    r.now = 500;
    r.run();
    assert!(
        !r.msgs().any(|m| matches!(m, WorkerMsg::Fatal { .. })),
        "the load did not fail"
    );
    assert_eq!(r.w.core_mut().controls_set(), [(1, SlotControl::Ai)]);
    assert!(r.w.view_hash(0).is_some());
}

#[test]
fn a_save_of_another_core_build_or_a_broken_core_part_is_code_4() {
    let mut h = H::game(2, 1);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    let try_load = |job: &SaveJob| -> Option<u8> {
        let save = saves::decode(&h.written(job, 1)).unwrap();
        let (_, outs) = Worker::resume(
            FixtureCore::new(false, false),
            load_init("x", slots(2, 1)),
            save,
            0,
        );
        outs.iter().find_map(|o| match o {
            Out::Exit(c) => Some(*c),
            _ => None,
        })
    };
    assert_eq!(try_load(&job), None);
    let mut other = job.clone();
    other.header.build_id = "other-core".into();
    assert_eq!(try_load(&other), Some(4));
    let mut broken = job;
    broken.gcs1 = b"FIXT garbage".to_vec();
    assert_eq!(try_load(&broken), Some(4));
}

#[test]
fn load_without_the_file_read_is_code_4() {
    let (_, outs) = Worker::new(
        FixtureCore::new(false, false),
        load_init("x", slots(2, 1)),
        0,
    );
    assert!(outs.contains(&Out::Exit(4)));
}

#[test]
fn a_damaged_file_does_not_decode() {
    let mut h = H::game(2, 1);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    let mut bytes = h.written(&job, 1);
    assert!(saves::decode(&bytes).is_ok());
    let at = bytes.len() / 2;
    bytes[at] ^= 0xff;
    assert!(matches!(saves::decode(&bytes), Err(SaveError::Damaged(_))));
    assert!(saves::read_header(&bytes[..20]).is_err());
}

#[test]
fn the_header_read_gives_what_the_lobby_lists() {
    let mut h = H::game(2, 1);
    h.send(ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let job = h.jobs.last().unwrap().clone();
    let info = saves::read_header(&h.written(&job, 77)).unwrap();
    assert_eq!(info.header.build_id, "fixture");
    assert_eq!(
        (info.header.seq, info.header.turn, info.header.game),
        (2, 1, 7)
    );
    assert_eq!(info.header.saved_at_ms, 77);
    assert_eq!(info.seats, job.sections.seats);
    assert_eq!(info.settings.name, "saves");
}
