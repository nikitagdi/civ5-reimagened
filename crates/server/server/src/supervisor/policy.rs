//! What the gateway does when a worker fails: how an exit is classified, which
//! save a restart loads, how long to wait, when a game is `Faulted`, and which orders were lost.
//! Pure state, no processes and no timers: times are `Clock` milliseconds, so tests drive them.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::time::Duration;

use protocol::{CmdId, CommandOutcome, PlayerId, SaveKind, SaveRecord, WorkerFrame, WorkerMsg};

/// Why a worker is gone, as the restart rules see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// No frame for `hang_s`; the gateway killed the process.
    Hang,
    /// The pipe carried something that is not a frame; the gateway killed the process.
    Garbage,
    /// Exit 1 or 3, a signal, an exit 0 nobody asked for, or stdout closed early.
    Crash,
    /// Exit 2: the worker could not read the gateway's frames.
    IpcError,
    /// Exit 4: the save did not load.
    LoadFailed,
    /// Exit 5: the game did not start.
    NotStarted,
}

/// What an exit means ("Коды выхода").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    /// Exit 0 after the gateway asked for it.
    Clean,
    Failed(Failure),
}

/// Classifies how the process ended. `shutdown_asked`: the gateway sent `Shutdown` (or closed the
/// pipe on purpose); `killed_for_hang`/`killed_for_garbage`: the gateway killed it for that reason.
pub fn classify_exit(
    code: Option<i32>,
    shutdown_asked: bool,
    killed_for_hang: bool,
    killed_for_garbage: bool,
) -> ExitKind {
    if killed_for_hang {
        return ExitKind::Failed(Failure::Hang);
    }
    if killed_for_garbage {
        return ExitKind::Failed(Failure::Garbage);
    }
    match code {
        Some(0) if shutdown_asked => ExitKind::Clean,
        Some(2) => ExitKind::Failed(Failure::IpcError),
        Some(4) => ExitKind::Failed(Failure::LoadFailed),
        Some(5) => ExitKind::Failed(Failure::NotStarted),
        // 1: other; 3: the core is stuck; 0 unasked; a signal; anything else: all crashes.
        _ => ExitKind::Failed(Failure::Crash),
    }
}

/// Timing of the restart rules. `Default` is the spec: 0, 2 and 10 s; three failures in ten
/// minutes make a game `Faulted`; a second failure within 60 s of a restart loads the save before.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Wait before restart number 1, 2, 3 (by the number of failures in the window).
    pub delays: [Duration; 3],
    pub faulted_after: usize,
    pub window: Duration,
    pub quick: Duration,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            delays: [
                Duration::ZERO,
                Duration::from_secs(2),
                Duration::from_secs(10),
            ],
            faulted_after: 3,
            window: Duration::from_secs(10 * 60),
            quick: Duration::from_secs(60),
        }
    }
}

/// What to do about a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Start a worker after `delay` from the save that is `skip` places behind the newest one
    /// (0: the newest).
    Restart { delay: Duration, skip: usize },
    /// No restart: the game is `Faulted`. The text is for the log and the admin.
    Faulted(String),
    /// A new game did not start (exit 5): refuse the start of the room.
    RefuseStart(String),
}

/// Restart rules of one game. One value per game, living as long as its supervision.
#[derive(Debug)]
pub struct RestartPolicy {
    cfg: PolicyConfig,
    /// Times of the failures inside the window.
    failures: VecDeque<u64>,
    skip: usize,
    /// When the worker of the last restart came up.
    restarted_at: Option<u64>,
    /// A restart was ordered and its worker is not up yet.
    restarting: bool,
    ipc_in_a_row: u32,
}

impl RestartPolicy {
    pub fn new(cfg: PolicyConfig) -> Self {
        Self {
            cfg,
            failures: VecDeque::new(),
            skip: 0,
            restarted_at: None,
            restarting: false,
            ipc_in_a_row: 0,
        }
    }

    /// A worker failed at `now_ms`. `new_game_start`: it was the first start of a new game, no
    /// worker of this game ever got ready.
    pub fn on_failure(&mut self, failure: Failure, new_game_start: bool, now_ms: u64) -> Decision {
        let window = ms(self.cfg.window);
        while self
            .failures
            .front()
            .is_some_and(|t| now_ms.saturating_sub(*t) >= window)
        {
            self.failures.pop_front();
        }
        self.failures.push_back(now_ms);

        // A failure soon after a restart (or before the restarted worker was up) is the second
        // in a row.
        let quick = self.restarting
            || self
                .restarted_at
                .is_some_and(|t| now_ms.saturating_sub(t) < ms(self.cfg.quick));
        self.restarting = false;

        if failure == Failure::NotStarted {
            return if new_game_start {
                Decision::RefuseStart("the worker could not start the game (exit 5)".into())
            } else {
                Decision::Faulted(
                    "the worker could not start the game from the save (exit 5)".into(),
                )
            };
        }
        if failure == Failure::IpcError {
            self.ipc_in_a_row = if quick { self.ipc_in_a_row + 1 } else { 1 };
            if self.ipc_in_a_row >= 2 {
                return Decision::Faulted("IPC error twice in a row (exit 2)".into());
            }
        } else {
            self.ipc_in_a_row = 0;
        }
        if self.failures.len() >= self.cfg.faulted_after {
            return Decision::Faulted(format!(
                "{} failures within {} s",
                self.failures.len(),
                self.cfg.window.as_secs()
            ));
        }

        // Which save: exit 4 and a quick second failure step one save back, an IPC error tries the
        // same save once more, anything else starts from the newest.
        match failure {
            Failure::LoadFailed => self.skip += 1,
            Failure::IpcError => {}
            _ if quick => self.skip += 1,
            _ => self.skip = 0,
        }
        self.restarting = true;
        let at = (self.failures.len() - 1).min(self.cfg.delays.len() - 1);
        Decision::Restart {
            delay: self.cfg.delays[at],
            skip: self.skip,
        }
    }

    /// The restarted worker is up (`Ready`).
    pub fn on_restarted(&mut self, now_ms: u64) {
        self.restarting = false;
        self.restarted_at = Some(now_ms);
    }

    /// An administrator restarts the game: history is forgotten, the newest save is loaded.
    pub fn reset(&mut self) {
        self.failures.clear();
        self.skip = 0;
        self.restarted_at = None;
        self.restarting = false;
        self.ipc_in_a_row = 0;
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

// ---------------------------------------------------------------- saves

/// One save the gateway knows of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveEntry {
    pub seq: u64,
    /// Path relative to the game's save directory (`ring/…` or `keep/…`), as in `Init::Load`.
    pub file: String,
    pub turn: i32,
    pub kind: SaveKind,
}

/// The saves of one game, newest first. Fed by `Saved` messages; at gateway start by [`scan`].
///
/// [`scan`]: SaveBook::scan
#[derive(Debug, Default, Clone)]
pub struct SaveBook {
    entries: Vec<SaveEntry>,
}

impl SaveBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a save the worker reported.
    pub fn record(&mut self, rec: &SaveRecord) {
        self.insert(SaveEntry {
            seq: rec.meta.seq,
            file: rec.file.clone(),
            turn: rec.meta.turn,
            kind: rec.kind,
        });
    }

    /// The save a game was loaded from, as the oldest entry: a restart before the game's own first
    /// save starts from it again. `file` may be absolute (a save of another game).
    pub fn seed(&mut self, file: String, turn: i32) {
        self.insert(SaveEntry {
            seq: 0,
            file,
            turn,
            kind: SaveKind::Keep,
        });
    }

    fn insert(&mut self, entry: SaveEntry) {
        self.entries.retain(|e| e.seq != entry.seq);
        self.entries.push(entry);
        self.entries.sort_by_key(|e| std::cmp::Reverse(e.seq));
    }

    /// A book of known entries (the saves whose headers checked out, see `game::restore::read_saves`).
    pub fn from_entries(entries: impl IntoIterator<Item = SaveEntry>) -> Self {
        let mut book = Self::new();
        for e in entries {
            book.insert(e);
        }
        book
    }

    /// Reads the saves from `<dir>/ring` and `<dir>/keep` by file name (`<seq:08>-t<turn:04>.c5sv`),
    /// without opening them: a file that does not load ends with exit 4 and the next older one is
    /// tried. A game back from the disk checks the headers (`game::restore::read_saves`).
    pub fn scan(dir: &Path) -> std::io::Result<Self> {
        let mut book = Self::new();
        for (sub, kind) in [("ring", SaveKind::Ring), ("keep", SaveKind::Keep)] {
            let Ok(read) = std::fs::read_dir(dir.join(sub)) else {
                continue;
            };
            for item in read {
                let name = item?.file_name().to_string_lossy().into_owned();
                if let Some((seq, turn)) = parse_save_name(&name) {
                    book.insert(SaveEntry {
                        seq,
                        file: format!("{sub}/{name}"),
                        turn,
                        kind,
                    });
                }
            }
        }
        Ok(book)
    }

    /// The save `skip` places behind the newest one; fewer saves than that: the oldest.
    pub fn pick(&self, skip: usize) -> Option<&SaveEntry> {
        self.entries
            .get(skip.min(self.entries.len().saturating_sub(1)))
    }

    /// Every entry, newest first.
    pub fn all(&self) -> &[SaveEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `00000007-t0012.c5sv` -> `(7, 12)`.
pub fn parse_save_name(name: &str) -> Option<(u64, i32)> {
    let stem = name.strip_suffix(".c5sv")?;
    let (seq, turn) = stem.split_once("-t")?;
    if seq.len() != 8 || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((seq.parse().ok()?, turn.parse().ok()?))
}

// ---------------------------------------------------------------- orders

/// Which orders of the seats have no answer yet, and which were applied since the last save.
/// When the worker dies, the first get `Lost` and the second are what the restart takes back.
#[derive(Debug, Default)]
pub struct OrderLedger {
    seats: BTreeMap<PlayerId, SeatOrders>,
}

#[derive(Debug, Default)]
struct SeatOrders {
    /// Sent to the worker, no final answer yet.
    pending: Vec<CmdId>,
    /// Applied, and not in the newest save.
    since_save: Vec<CmdId>,
}

/// What a failure cost.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Casualties {
    /// Orders without a final answer, per seat: each gets `Lost`.
    pub lost: Vec<(PlayerId, Vec<CmdId>)>,
    /// Orders that were applied but are not in the save the restart loads, per seat.
    pub rolled_back: Vec<(PlayerId, u32)>,
}

impl OrderLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// An order goes to the worker.
    pub fn sent(&mut self, player: PlayerId, id: CmdId) {
        self.seats.entry(player).or_default().pending.push(id);
    }

    /// Reads a worker frame: `Results` answer orders, `Saved` moves the line of what is safe.
    pub fn observe(&mut self, frame: &WorkerFrame) {
        for msg in &frame.msgs {
            match msg {
                WorkerMsg::Results { player, results } => {
                    let seat = self.seats.entry(*player).or_default();
                    for r in results {
                        // `Queued` is not final.
                        if r.outcome == CommandOutcome::Queued {
                            continue;
                        }
                        seat.pending.retain(|id| *id != r.id);
                        if r.outcome == CommandOutcome::Applied {
                            seat.since_save.push(r.id);
                        }
                    }
                }
                WorkerMsg::Saved(rec) => {
                    for (player, seat) in &mut self.seats {
                        let upto = rec
                            .applied_upto
                            .iter()
                            .find(|(p, _)| p == player)
                            .map(|(_, id)| *id);
                        // Order numbers grow per seat; everything up to `upto` is in the save.
                        seat.since_save.retain(|id| upto.is_none_or(|u| *id > u));
                    }
                }
                _ => {}
            }
        }
    }

    /// The worker is gone. Empties the ledger: the restarted worker starts from its save.
    pub fn worker_lost(&mut self) -> Casualties {
        let mut out = Casualties::default();
        for (player, seat) in &mut self.seats {
            if !seat.pending.is_empty() {
                out.lost.push((*player, std::mem::take(&mut seat.pending)));
            }
            let rolled = std::mem::take(&mut seat.since_save);
            if !rolled.is_empty() {
                out.rolled_back.push((*player, rolled.len() as u32));
            }
        }
        out
    }
}

/// The journal line of a seat after a restart that cost orders (spec 4.10 step 5, AC-N1.5: the loss is not
/// silent). The worker writes the line "the game continues from the save of turn N" itself on every load, so
/// this one reports only the lost orders and is sent only when there are some.
// The text is shown to players.
pub fn restored_line(lost: u32) -> String {
    format!("Потеряно ваших приказов при восстановлении: {lost}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{CommandResult, SaveMeta};

    fn policy() -> RestartPolicy {
        RestartPolicy::new(PolicyConfig::default())
    }

    const MIN: u64 = 60_000;

    fn restart(skip: usize, delay_s: u64) -> Decision {
        Decision::Restart {
            delay: Duration::from_secs(delay_s),
            skip,
        }
    }

    #[test]
    fn exit_codes_map_to_failures() {
        use ExitKind::{Clean, Failed};
        let c = |code, asked| classify_exit(code, asked, false, false);
        assert_eq!(c(Some(0), true), Clean);
        // An exit 0 nobody asked for is a crash.
        assert_eq!(c(Some(0), false), Failed(Failure::Crash));
        assert_eq!(c(Some(1), false), Failed(Failure::Crash));
        assert_eq!(c(Some(2), false), Failed(Failure::IpcError));
        assert_eq!(c(Some(3), false), Failed(Failure::Crash));
        assert_eq!(c(Some(4), false), Failed(Failure::LoadFailed));
        assert_eq!(c(Some(5), false), Failed(Failure::NotStarted));
        // A signal, or a code outside the table.
        assert_eq!(c(None, false), Failed(Failure::Crash));
        assert_eq!(c(Some(139), false), Failed(Failure::Crash));
        // Our own kills win over whatever the dying process returned.
        assert_eq!(
            classify_exit(Some(1), false, true, false),
            Failed(Failure::Hang)
        );
        assert_eq!(
            classify_exit(Some(0), true, false, true),
            Failed(Failure::Garbage)
        );
    }

    #[test]
    fn a_crash_restarts_with_the_newest_save_and_no_delay() {
        let mut p = policy();
        assert_eq!(p.on_failure(Failure::Crash, false, 0), restart(0, 0));
    }

    #[test]
    fn delays_are_zero_two_and_ten_seconds() {
        // Spec 4.10 lists three delays but also makes the third failure in ten minutes
        // `Faulted`; with the default `faulted_after = 3` the 10 s step is reached only when the
        // window is configured for more failures.
        let mut p = RestartPolicy::new(PolicyConfig {
            faulted_after: 4,
            ..PolicyConfig::default()
        });
        let delays: Vec<_> = (0..3)
            .map(|i| match p.on_failure(Failure::Crash, false, i * 6_000) {
                Decision::Restart { delay, .. } => delay.as_secs(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(delays, [0, 2, 10]);
        assert!(matches!(
            p.on_failure(Failure::Crash, false, 20_000),
            Decision::Faulted(_)
        ));
    }

    #[test]
    fn three_failures_in_ten_minutes_fault_the_game() {
        let mut p = policy();
        assert!(matches!(
            p.on_failure(Failure::Crash, false, 0),
            Decision::Restart { .. }
        ));
        p.on_restarted(1_000);
        assert!(matches!(
            p.on_failure(Failure::Hang, false, 2 * MIN),
            Decision::Restart { .. }
        ));
        p.on_restarted(2 * MIN + 1_000);
        assert!(matches!(
            p.on_failure(Failure::Crash, false, 9 * MIN),
            Decision::Faulted(_)
        ));
    }

    #[test]
    fn failures_older_than_ten_minutes_do_not_count() {
        let mut p = policy();
        for i in 0..6 {
            let at = i * 11 * MIN;
            assert_eq!(
                p.on_failure(Failure::Crash, false, at),
                restart(0, 0),
                "failure {i}"
            );
            p.on_restarted(at + 5_000);
        }
    }

    #[test]
    fn exit_4_loads_the_save_before() {
        let mut p = policy();
        assert_eq!(p.on_failure(Failure::LoadFailed, false, 0), restart(1, 0));
        // The previous save did not load either: one more step back.
        assert_eq!(p.on_failure(Failure::LoadFailed, false, 100), restart(2, 2));
    }

    #[test]
    fn a_second_failure_within_a_minute_of_a_restart_steps_back() {
        let mut p = policy();
        assert_eq!(p.on_failure(Failure::Crash, false, 0), restart(0, 0));
        p.on_restarted(3_000);
        // 30 s after the restart: the newest save seems to be the cause.
        assert_eq!(p.on_failure(Failure::Crash, false, 33_000), restart(1, 2));
        p.on_restarted(36_000);
        // The worker ran for ten minutes: this is a new, unrelated failure.
        assert_eq!(
            p.on_failure(Failure::Crash, false, 36_000 + 10 * MIN),
            restart(0, 0)
        );
    }

    #[test]
    fn a_worker_that_never_came_up_counts_as_a_quick_failure() {
        let mut p = policy();
        assert_eq!(p.on_failure(Failure::Crash, false, 0), restart(0, 0));
        // The restart itself failed before `Ready`, long after: still the second in a row.
        assert_eq!(p.on_failure(Failure::Crash, false, 5 * MIN), restart(1, 2));
    }

    #[test]
    fn exit_2_gets_one_more_try_with_the_same_save() {
        let mut p = policy();
        assert_eq!(p.on_failure(Failure::IpcError, false, 0), restart(0, 0));
        p.on_restarted(1_000);
        assert!(matches!(
            p.on_failure(Failure::IpcError, false, 2_000),
            Decision::Faulted(_)
        ));
        // After a long quiet run the budget is back.
        let mut p = policy();
        assert_eq!(p.on_failure(Failure::IpcError, false, 0), restart(0, 0));
        p.on_restarted(1_000);
        assert_eq!(
            p.on_failure(Failure::IpcError, false, 5 * MIN),
            restart(0, 2)
        );
    }

    #[test]
    fn exit_5_refuses_a_new_game_and_faults_a_loaded_one() {
        let mut p = policy();
        assert!(matches!(
            p.on_failure(Failure::NotStarted, true, 0),
            Decision::RefuseStart(_)
        ));
        let mut p = policy();
        assert!(matches!(
            p.on_failure(Failure::NotStarted, false, 0),
            Decision::Faulted(_)
        ));
    }

    #[test]
    fn an_admin_restart_forgets_the_history() {
        let mut p = policy();
        p.on_failure(Failure::Crash, false, 0);
        p.on_failure(Failure::Crash, false, 1_000);
        p.reset();
        assert_eq!(p.on_failure(Failure::Crash, false, 2_000), restart(0, 0));
    }

    // ---- saves

    fn rec(seq: u64, turn: i32, upto: &[(PlayerId, CmdId)]) -> SaveRecord {
        SaveRecord {
            meta: SaveMeta {
                seq,
                build_id: "b".into(),
                turn,
                civ_name: "c".into(),
                saved_at_ms: 0,
            },
            file: format!("ring/{seq:08}-t{turn:04}.c5sv"),
            kind: SaveKind::Ring,
            applied_upto: upto.to_vec(),
        }
    }

    #[test]
    fn the_save_book_picks_newest_then_older() {
        let mut book = SaveBook::new();
        assert!(book.pick(0).is_none());
        for seq in [3, 1, 2] {
            book.record(&rec(seq, seq as i32 * 10, &[]));
        }
        assert_eq!(book.pick(0).unwrap().seq, 3);
        assert_eq!(book.pick(1).unwrap().seq, 2);
        assert_eq!(book.pick(2).unwrap().seq, 1);
        // Past the oldest: the oldest.
        assert_eq!(book.pick(9).unwrap().seq, 1);
        // The same sequence number replaces the entry.
        book.record(&rec(3, 99, &[]));
        assert_eq!((book.len(), book.pick(0).unwrap().turn), (3, 99));
    }

    #[test]
    fn save_names_are_parsed_strictly() {
        assert_eq!(parse_save_name("00000007-t0012.c5sv"), Some((7, 12)));
        assert_eq!(parse_save_name("7-t12.c5sv"), None);
        assert_eq!(parse_save_name("00000007-t0012.c5sv.tmp"), None);
        assert_eq!(parse_save_name("0000000x-t0012.c5sv"), None);
        assert_eq!(parse_save_name("notes.txt"), None);
    }

    #[test]
    fn the_save_book_scans_ring_and_keep() {
        let dir = std::env::temp_dir().join(format!("civ5-savebook-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ring")).unwrap();
        std::fs::create_dir_all(dir.join("keep")).unwrap();
        for f in [
            "ring/00000002-t0002.c5sv",
            "ring/00000004-t0004.c5sv",
            "keep/00000003-t0003.c5sv",
            "ring/junk",
        ] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let book = SaveBook::scan(&dir).unwrap();
        assert_eq!(book.len(), 3);
        assert_eq!(book.pick(0).unwrap().file, "ring/00000004-t0004.c5sv");
        assert_eq!(book.pick(1).unwrap().kind, SaveKind::Keep);
        // A directory that does not exist is an empty book.
        assert!(SaveBook::scan(&dir.join("none")).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- orders

    fn results(player: PlayerId, rs: &[(CmdId, CommandOutcome)]) -> WorkerFrame {
        WorkerFrame {
            msgs: vec![WorkerMsg::Results {
                player,
                results: rs
                    .iter()
                    .map(|(id, outcome)| CommandResult {
                        id: *id,
                        outcome: outcome.clone(),
                    })
                    .collect(),
            }],
        }
    }

    fn saved(upto: &[(PlayerId, CmdId)]) -> WorkerFrame {
        WorkerFrame {
            msgs: vec![WorkerMsg::Saved(rec(1, 1, upto))],
        }
    }

    #[test]
    fn orders_without_an_answer_are_lost() {
        let mut l = OrderLedger::new();
        for id in 1..=3 {
            l.sent(0, id);
        }
        l.sent(1, 7);
        l.observe(&results(0, &[(1, CommandOutcome::Applied)]));
        l.observe(&results(
            0,
            &[(
                2,
                CommandOutcome::Rejected {
                    code: protocol::RejectCode::Core,
                    reason: "no".into(),
                },
            )],
        ));
        // `Queued` is not an answer.
        l.observe(&results(1, &[(7, CommandOutcome::Queued)]));
        let c = l.worker_lost();
        assert_eq!(c.lost, vec![(0, vec![3]), (1, vec![7])]);
        assert_eq!(c.rolled_back, vec![(0, 1)]);
        // The ledger starts clean for the next worker.
        assert_eq!(l.worker_lost(), Casualties::default());
    }

    #[test]
    fn a_save_takes_applied_orders_out_of_the_rolled_back() {
        let mut l = OrderLedger::new();
        for id in 1..=4 {
            l.sent(0, id);
            l.observe(&results(0, &[(id, CommandOutcome::Applied)]));
            if id == 2 {
                // The save holds orders up to 2.
                l.observe(&saved(&[(0, 2)]));
            }
        }
        let c = l.worker_lost();
        assert!(c.lost.is_empty());
        assert_eq!(c.rolled_back, vec![(0, 2)]);
    }

    #[test]
    fn a_save_without_a_seat_entry_holds_none_of_its_orders() {
        let mut l = OrderLedger::new();
        l.sent(0, 1);
        l.observe(&results(0, &[(1, CommandOutcome::Applied)]));
        // This save knows no order of seat 0 yet... but `upto` absent means "none", so nothing
        // is cleared.
        l.observe(&saved(&[(1, 5)]));
        assert_eq!(l.worker_lost().rolled_back, vec![(0, 1)]);
    }

    #[test]
    fn the_journal_line_names_the_count() {
        assert_eq!(
            restored_line(3),
            "Потеряно ваших приказов при восстановлении: 3"
        );
    }
}
