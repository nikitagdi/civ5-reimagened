//! UI v2 source: the view-model roots built from the replica's game and the client session.
//!
//! [`UiSource::frame`] runs once per frame. It compares the change signals (the replica's state, generation and
//! last applied batch, the pump's steps; the session's turn state, seats and timer; the UI's selection and open
//! window) with the previous frame's and rebuilds only the roots a change asks for; with no change it reads
//! nothing and writes nothing. Roots are written whole through the store's [`ModelWriter`]; the store diffs them.
//!
//! What a frame reads from the game is kept until the replica changes ([`Cache`]): a root that a session or UI
//! change rebuilds reads only what no earlier root of the same game state read.
//!
//! Read only by construction: the builders see the game as `&mut dyn replica::GameRead` (typed queries, no
//! setters, no UI operations) and the session as `&dyn SessionRead` (getters only). [`ReplicaView::of`] is the
//! one place that touches a [`Replica`], and it only takes its read-only signals and its reader.

pub mod build;
pub mod catalog;
pub mod cost;
pub mod pending;
pub mod profile;
pub mod session;
pub mod static_source;
pub mod trigger;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use replica::{
    CatalogEntry, CatalogTable, City, CityBuild, CityOrder, GameInfo, GameRead, GameSide,
    Notification, Player, PlayerResource, PolicyBranchState, ReadError, Replica, ReplicaStep,
    State, TechState, TopBar, Unit, UnitAction,
};
use ui2_model::r#gen::{Meta, Turn, Ui};
use ui2_model::{Key, ModelWriter, Root, Schema};

pub use crate::cost::DEFAULT_FRAME_BUDGET;
pub use crate::profile::SourceProfile;
pub use crate::session::SessionRead;
pub use crate::static_source::StaticSource;

use crate::catalog::Catalog;
use crate::cost::{ReadKind, UNLIMITED};
use crate::profile::{READ_SLOTS, ReadCost, RootCost};
use crate::trigger::{Roots, Seen, SessionSeen, UiSeen};

/// A monotonic clock in ms the host may give the source for its profile (library code reads no time on wasm).
pub type SourceClock = Box<dyn Fn() -> f64>;

/// The clock as a frame borrows it.
type Clock<'a> = Option<&'a dyn Fn() -> f64>;

/// The time `f` takes, µs (0 without a clock).
fn timed<T>(clock: Clock<'_>, f: impl FnOnce() -> T) -> (T, f64) {
    match clock {
        Some(c) => {
            let t0 = c();
            let r = f();
            (r, (c() - t0) * 1000.0)
        }
        None => (f(), 0.0),
    }
}

/// The replica as one frame of the source sees it: its change signals and, while it can be read, its reader.
pub struct ReplicaView<'a> {
    pub state: State,
    /// The replica still has journal batches to apply (or no base yet): what it shows is the past.
    pub catching_up: bool,
    /// Grows on every snapshot load and resync.
    pub generation: u64,
    /// The last journal batch applied.
    pub applied_jseq: u64,
    /// The game turn the replica's journal has reached (`Replica::turn`): a batch that ends the turn says so
    /// before the next turn's snapshot is loaded, so it can be ahead of the game the reader shows. `None`: not
    /// known.
    pub turn: Option<i32>,
    /// The game, read only; `None` when the replica is not live or its engine has no core.
    pub read: Option<&'a mut dyn GameRead>,
}

impl<'a> ReplicaView<'a> {
    /// The view of a replica: its signals and [`Replica::read`]. Nothing here changes the replica.
    pub fn of(r: &'a mut Replica) -> ReplicaView<'a> {
        let state = r.state();
        let catching_up = GameSide::catching_up(r);
        let generation = r.generation();
        let applied_jseq = r.applied_jseq();
        let turn = Some(r.turn());
        ReplicaView {
            state,
            catching_up,
            generation,
            applied_jseq,
            turn,
            read: r.read(),
        }
    }

    fn seen(&self) -> Seen {
        Seen {
            state: self.state,
            catching_up: self.catching_up,
            generation: self.generation,
            applied_jseq: self.applied_jseq,
            turn: self.turn,
        }
    }
}

/// The input of one frame, made by the host after it pumped the replica and applied the UI's intents. `'r` is the
/// replica's borrow, `'s` the borrow of everything else.
pub struct SourceFrame<'r, 's> {
    /// `None`: there is no replica (lobby, another UI).
    pub replica: Option<ReplicaView<'r>>,
    /// What the replica's pump did in this frame.
    pub steps: &'s [ReplicaStep],
    /// The client session (in the app: the `ClientCore`).
    pub session: &'s dyn SessionRead,
    /// The notifications the player dismissed here (in the app: `UiActions::dismissed`); `notes` leaves them out.
    pub dismissed: &'s BTreeSet<Key>,
    /// The host's clock, ms: the turn timer is read at this time.
    pub now_ms: u64,
}

/// What one frame cost, for the debug overlay and the performance report.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SourceStats {
    /// Roots written to the store.
    pub roots_built: u32,
    /// Reads of the core.
    pub getter_calls: u32,
    /// Time spent, µs. Always 0 here: the library has no clock (wasm); the host times the call.
    pub spent_us: u32,
    /// The cost units of the reads made ([`cost`]: about µs of the browser build); what the frame budget counts.
    pub spent_units: u32,
    /// Roots that wait for a later frame (the replica cannot be read yet, the frame's budget is spent, or the
    /// production window still fills its comparison).
    pub deferred: u32,
}

/// The replica adapter: rebuilds the game-data roots on change triggers.
pub struct UiSource {
    /// The schema version, as `meta.schema` carries it.
    schema_text: Arc<str>,
    /// The replica's signals as of the last frame that had a replica.
    seen: Option<Seen>,
    /// The session's signals as of the last frame.
    session: Option<SessionSeen>,
    /// Roots asked for and not built yet.
    pending: Roots,
    /// The last `meta` written, so an unchanged one is not written again.
    meta: Option<Meta>,
    /// The last `turn` written: the timer patch starts from it.
    turn: Option<Turn>,
    /// Read once per core, table by table over the first frames that read the game.
    catalog: Catalog,
    /// The game reads of the current replica state.
    cache: Cache,
    /// The UI v2 queries the core does not serve: not asked again until [`UiSource::reset`].
    missing: Missing,
    /// Cost units per frame, see [`UiSource::set_read_budget`].
    budget: u32,
    /// The dismissed notifications as of the last frame.
    dismissed: BTreeSet<Key>,
    /// The ids of the notifications the core had at the last `notes` build.
    note_ids: Option<BTreeSet<Key>>,
    /// The UI root's parts the roots depend on, as of the last frame.
    ui: UiSeen,
    /// The turn each chat line came in.
    chat: build::chat::ChatLog,
    /// Frames each game root (by bit) has waited while the game could be read: the longest waiting goes first,
    /// and one that waited [`STALE_FRAMES`] gets all its reads, so a stream of journal batches starves none.
    waited: [u8; 16],
    /// The host's clock, for the profile only (the budget counts cost units).
    clock: Option<SourceClock>,
    /// What the last frame or warm-up spent.
    profile: SourceProfile,
}

/// The UI v2 queries a core answered `ReadError::Unsupported` to (the stub, a core older than the queries). Their
/// fields stay at the schema's defaults; asking again on every trigger would only cost reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Missing {
    techs: bool,
    resources: bool,
    branches: bool,
    promotions: bool,
    queue: bool,
}

impl UiSource {
    #[must_use]
    pub fn new(schema: &'static Schema) -> UiSource {
        UiSource::blank(
            Arc::from(schema.version().to_string()),
            DEFAULT_FRAME_BUDGET,
        )
    }

    fn blank(schema_text: Arc<str>, budget: u32) -> UiSource {
        UiSource {
            schema_text,
            seen: None,
            session: None,
            pending: Roots::NONE,
            meta: None,
            turn: None,
            catalog: Catalog::default(),
            cache: Cache::default(),
            missing: Missing::default(),
            budget,
            dismissed: BTreeSet::new(),
            note_ids: None,
            ui: UiSeen::default(),
            chat: build::chat::ChatLog::default(),
            waited: [0; 16],
            clock: None,
            profile: SourceProfile::default(),
        }
    }

    /// Gives the source a clock (ms): the [`profile`](Self::profile) then has the time of each root and read.
    /// Nothing else depends on it.
    pub fn set_profile_clock(&mut self, clock: SourceClock) {
        self.clock = Some(clock);
    }

    /// What the last [`frame`](Self::frame) (or [`warm_up`](Self::warm_up)) spent, per root and per kind of read.
    #[must_use]
    pub fn profile(&self) -> &SourceProfile {
        &self.profile
    }

    /// The frame budget, in cost units ([`cost`]: one unit is about one µs of reads in the browser build; the
    /// default is [`DEFAULT_FRAME_BUDGET`], 1.5 ms); `u32::MAX` is no limit. Each read of the game costs its
    /// weight from the [`cost`] table, a read answered from the cache costs nothing.
    ///
    /// The rules of a frame:
    /// - the roots always on screen ([`Roots::PRIORITY`]: `turn`, `game`, `top`, the selected `unit`) are built
    ///   first and get all their reads, so a change shows in the frame the replica has it; they cost about a
    ///   tenth of the budget, and what they spent counts against it;
    /// - then the other roots, the longest waiting first, each written whole; a root whose next read does not
    ///   fit in what is left of the budget is not written, waits for the next frame and stops this one (what it
    ///   read so far is kept, so the next frame does not read it again);
    /// - the first piece of work of a frame that reads the game (a root, or a catalog table) always gets all its
    ///   reads, and so does a root that has waited [`STALE_FRAMES`] frames: no root starves under a stream of
    ///   journal batches, and a frame spends more than the budget only when such a piece alone costs more;
    /// - the catalog (about 1.9 ms in all) is read table by table over the first frames, the tables that fit;
    ///   a root that names things from a table not read yet waits for it, the other roots go on;
    /// - an open production window is built only when nothing was read before it in the frame (its city's
    ///   options alone cost more than the default budget) or once it is stale; it reads the options of at
    ///   most one city per frame while the budget is limited, so its comparison across the seat's cities fills
    ///   over frames, the window's own city first, the root rewritten whole as each city arrives.
    pub fn set_read_budget(&mut self, units: u32) {
        self.budget = units;
    }

    /// The ids of the notifications the core has (those it did not dismiss itself), as of the last `notes`
    /// build; `None` before the first. The `notes` root leaves out what the player dismissed here, so this is
    /// what the actions module judges its dismissed set by (`UiActions::reconcile`).
    #[must_use]
    pub fn notification_ids(&self) -> Option<&BTreeSet<Key>> {
        self.note_ids.as_ref()
    }

    /// Forgets everything (a new game or a new core): the next frame rebuilds every root, reads the catalog and
    /// asks the UI v2 queries again. The read budget and the clock stay.
    pub fn reset(&mut self) {
        let clock = self.clock.take();
        *self = UiSource::blank(self.schema_text.clone(), self.budget);
        self.clock = clock;
    }

    /// Takes the core's slow first calls out of the frames. The first call of some reads after a core loads a
    /// game costs far more than the frame budget assumes (the core fills its own caches: the top bar about 11 ms,
    /// the game record 1.4 ms, a unit's actions 1.6 ms, a city's options 4.6 ms). This makes one call of each of
    /// those (for the seat's first unit and first city, if any) and reads the catalog tables not read yet, with
    /// no budget, and keeps only the catalog: the frames then start from warm caches and a ready catalog.
    ///
    /// For the host: call it once when a game is joined or loaded (the replica live, after
    /// [`reset`](Self::reset) for a new game), at a moment that may take a few ms more, such as the load
    /// screen; it is never called from [`frame`](Self::frame). It reads only; skipping it changes nothing but the
    /// cost of the first frames. The stats count its reads and their cost units (the table's medians, not the
    /// real cold time).
    pub fn warm_up(&mut self, read: &mut dyn GameRead) -> SourceStats {
        // What it reads of the game is not kept: the next frame's signals decide whether the game changed.
        let mut cache = Cache::default();
        let clock = self.clock.as_deref();
        let mut read = CountingRead::new(read, &mut cache, self.missing, UNLIMITED, clock);
        read.begin(true);
        let seat = read.seat();
        let _ = read.game_info();
        let _ = read.top_bar();
        if let Ok(units) = read.units()
            && let Some(u) = units.iter().find(|u| u.owner.cast_unsigned() == seat)
        {
            let _ = read.unit_actions(u.id);
        }
        if let Ok(cities) = read.cities()
            && let Some(c) = cities.iter().find(|c| c.owner.cast_unsigned() == seat)
        {
            let _ = read.city_builds(c.id);
        }
        if !self.catalog.complete() {
            self.catalog.load(&mut read);
        }
        self.missing = read.missing;
        self.profile = SourceProfile {
            reads: read.meter.reads,
            ..SourceProfile::default()
        };
        SourceStats {
            getter_calls: read.meter.calls,
            spent_units: read.meter.spent,
            ..SourceStats::default()
        }
    }

    /// Takes the core's slow first calls after a snapshot load out of the frames. Every new turn loads a snapshot,
    /// and the first `top_bar` after a load costs the core 3 to 5.6 ms on a small game (it fills its own caches;
    /// measured natively and in the browser), more than three frame budgets; the first reads of a unit's actions
    /// and of a city's options are slow the same way. This makes the first call of each read the next frame makes
    /// of the always-shown roots (the game record, the top bar with resources and policies, the selected unit's
    /// actions and promotions in `ui`, the open production window's city), with no budget, and keeps nothing.
    ///
    /// For the host: call it in the frame whose pump loaded a snapshot (`ReplicaStep::Loaded`/`TurnSwitched`), right
    /// after the pump and before [`frame`](Self::frame), as part of the load; never from inside a frame. It reads
    /// only; skipping it changes nothing but the cost of that frame. The [`profile`](Self::profile) has its reads.
    pub fn warm_turn(&mut self, read: &mut dyn GameRead, ui: &Ui) -> SourceStats {
        let mut cache = Cache::default();
        let clock = self.clock.as_deref();
        let mut read = CountingRead::new(read, &mut cache, self.missing, UNLIMITED, clock);
        read.begin(true);
        let seat = read.seat();
        let _ = read.game_info();
        let _ = read.top_bar();
        let _ = read.player_resources();
        let _ = read.policy_branches();
        if let Some(key) = ui.selection.unit
            && let Ok(units) = read.units()
            && let Some(u) =
                build::unit::find(&units, key).filter(|u| u.owner.cast_unsigned() == seat)
        {
            let _ = read.unit_actions(u.id);
            let _ = read.unit_promotions(u.id);
        }
        let window = &ui.windows.production;
        if window.open
            && let Some(key) = window.city
            && let Ok(cities) = read.cities()
            && let Some(c) = cities.iter().find(|c| {
                c.owner.cast_unsigned() == seat && build::entity_key(c.owner, c.id) == key
            })
        {
            let _ = read.city_builds(c.id);
        }
        self.missing = read.missing;
        self.profile = SourceProfile {
            reads: read.meter.reads,
            ..SourceProfile::default()
        };
        SourceStats {
            getter_calls: read.meter.calls,
            spent_units: read.meter.spent,
            ..SourceStats::default()
        }
    }

    /// One frame: rebuild what the signals ask for, write it, report the cost. `ui` is the UI's own root (owned by
    /// the actions module): the selected unit and the open production window come from it.
    pub fn frame(
        &mut self,
        f: SourceFrame<'_, '_>,
        w: &mut ModelWriter<'_>,
        ui: &Ui,
    ) -> SourceStats {
        self.profile = SourceProfile {
            session_turn: f.session.turn_state().map(|t| t.turn),
            ..SourceProfile::default()
        };
        let stats = self.run(f, w, ui);
        self.profile.pending = self.pending;
        self.profile.game_turn = self.cache.turn();
        stats
    }

    fn run(&mut self, f: SourceFrame<'_, '_>, w: &mut ModelWriter<'_>, ui: &Ui) -> SourceStats {
        let mut stats = SourceStats::default();
        let SourceFrame {
            replica,
            steps,
            session,
            dismissed,
            now_ms,
        } = f;

        let seen = replica.as_ref().map(ReplicaView::seen);
        let meta = build::game::meta(&self.schema_text, seen.as_ref(), session.connected());
        if self.meta.as_ref() != Some(&meta) {
            write(w, meta.clone(), &mut stats);
            self.meta = Some(meta);
        }

        if self.session.is_none() {
            // A first look at the session: the session's roots are written once even if it holds nothing yet.
            self.pending = self.pending.union(Roots::CHAT);
        }
        let sess = self.session.get_or_insert_with(SessionSeen::default);
        self.pending = self.pending.union(sess.update(session, now_ms));
        if &self.dismissed != dismissed {
            self.dismissed.clone_from(dismissed);
            self.pending = self.pending.union(Roots::NOTES);
        }
        self.pending = self.pending.union(self.ui.update(ui));

        let mut reader = None;
        if let Some(view) = replica {
            let now = view.seen();
            let changed = trigger::changes(self.seen.as_ref(), &now, steps);
            if !changed.is_empty() {
                // The game changed: what was read of it is stale.
                self.cache = Cache::default();
                self.catalog.retry_failed();
            }
            self.pending = self.pending.union(changed);
            self.seen = Some(now);
            if now.readable() {
                reader = view.read;
            }
        }
        if self.pending.is_empty() {
            return stats;
        }

        // The timer patch reads nothing: it keeps the clock going while the replica cannot be read. When `turn`
        // is rebuilt in this frame anyway, the rebuild carries the new second.
        if self.pending.contains(Roots::TIMER) {
            self.pending = self.pending.without(Roots::TIMER);
            if !(self.pending.contains(Roots::TURN) && reader.is_some()) {
                self.patch_timer(w, session, now_ms, &mut stats);
            }
        }
        // The chat reads only the session.
        if self.pending.contains(Roots::CHAT) {
            self.pending = self.pending.without(Roots::CHAT);
            let turn = session
                .turn_state()
                .map(|t| t.turn)
                .or_else(|| self.cache.turn());
            let chat = self.chat.rows(session, turn.unwrap_or(0));
            write(w, chat, &mut stats);
        }
        if self.pending.is_empty() {
            return stats;
        }
        let Some(read) = reader else {
            stats.deferred = self.pending.len();
            return stats;
        };
        self.profile.readable = true;

        let clock = self.clock.as_deref();
        let now = || clock.map_or(0.0, |c| c());
        let mut read = CountingRead::new(read, &mut self.cache, self.missing, self.budget, clock);
        // The roots of this source show catalog names; the tables are read over the first frames, and a root
        // waits for the tables it needs.
        if !self.catalog.complete() {
            self.catalog.load(&mut read);
        }
        let catalog = &self.catalog;
        let cx = build::Cx {
            session,
            now_ms,
            catalog,
            journal_turn: self.seen.and_then(|s| s.turn),
        };

        // The order of the frame (see `set_read_budget`): the roots always on screen first, each with all its reads;
        // then the others, the longest waiting first, against what is left of the budget. A root is written only
        // when none of its reads was refused; a refused root stays pending and ends the frame (its admitted reads
        // are kept in the cache).
        let window = ui.windows.production.open;
        let limited = self.budget != UNLIMITED;
        let order = queue_order(&self.waited);
        let mut sweeping = false;
        for root in order {
            if !self.pending.contains(root) || !catalog.ready(catalog::tables_of(root)) {
                // Not asked for, or its names are not loaded yet.
                continue;
            }
            let priority = Roots::PRIORITY.contains(&root);
            let stale = self.waited[root.index()] >= STALE_FRAMES;
            if root == Roots::PRODUCTION && window && limited && !read.nothing_read() && !stale {
                // The options of a city cost more than a frame's budget: the window waits for a frame of its own,
                // or until it is stale.
                continue;
            }
            read.begin(priority || stale || read.nothing_read());
            let (t0, u0, n0) = (now(), read.meter.spent, stats.roots_built);
            if root == Roots::TOP {
                let r = build::top::top(&mut read, catalog);
                if let Some(r) = read.settle(r) {
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::RESEARCH {
                let r = build::top::research(&mut read, catalog);
                if let Some(r) = read.settle(r) {
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::GAME {
                let r = build::game::game(&mut read, &cx);
                if let Some(r) = read.settle(r) {
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::TURN {
                let r = build::turn::turn(&mut read, &cx);
                match read.settle(r) {
                    Some(Ok(t)) => {
                        self.turn = Some(t.clone());
                        write(w, t, &mut stats);
                    }
                    Some(Err(e)) => not_built(Turn::NAME, &e),
                    None => {}
                }
            } else if root == Roots::PLAYERS {
                let r = build::players::players(&mut read);
                if let Some(r) = read.settle(r) {
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::UNIT {
                let r = build::unit::unit(&mut read, catalog, ui.selection.unit, &ui.pending);
                if let Some(r) = read.settle(r) {
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::PRODUCTION {
                let r = build::production::production(
                    &mut read,
                    catalog,
                    &ui.windows.production,
                    ui.production.focus,
                    &ui.pending,
                );
                if let Some(r) = read.settle(r) {
                    let r = r.map(|(root, complete)| {
                        sweeping = !complete;
                        root
                    });
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::NOTES {
                let r = build::notes::notes(&mut read, dismissed);
                if let Some(r) = read.settle(r) {
                    let r = r.map(|(rows, ids)| {
                        self.note_ids = Some(ids);
                        rows
                    });
                    write_built(w, r, &mut stats);
                }
            } else if root == Roots::MAP {
                let r = build::map::map(&mut read, catalog);
                if let Some(r) = read.settle(r) {
                    write_built(w, r, &mut stats);
                }
            }
            self.profile.roots.push(RootCost {
                root,
                units: read.meter.spent - u0,
                us: (now() - t0) * 1000.0,
                written: stats.roots_built > n0,
            });
            if read.refused() {
                break;
            }
            self.pending = self.pending.without(root);
        }
        for root in Roots::QUEUE {
            let w = &mut self.waited[root.index()];
            *w = if self.pending.contains(root) {
                w.saturating_add(1)
            } else {
                0
            };
        }
        if sweeping {
            // The comparison still misses cities: the next frame reads one more. Only this source's own state
            // keeps it going; its writes to the model trigger nothing.
            self.pending = self.pending.union(Roots::PRODUCTION);
        }
        stats.deferred = self.pending.len();
        stats.getter_calls = read.meter.calls;
        stats.spent_units = read.meter.spent;
        self.missing = read.missing;
        self.profile.reads = read.meter.reads;
        self.cache.forget_errors();
        stats
    }

    /// Writes the last `turn` with the timer of now.
    fn patch_timer(
        &mut self,
        w: &mut ModelWriter<'_>,
        session: &dyn SessionRead,
        now_ms: u64,
        stats: &mut SourceStats,
    ) {
        let Some(turn) = self.turn.as_mut() else {
            return;
        };
        let t = session::timer(session, now_ms);
        if turn.timer != t {
            turn.timer = t;
            write(w, turn.clone(), stats);
        }
    }
}

/// Frames a root may wait for the budget (frames in which the game could be read): then it gets all its reads,
/// so every root asked for is written at most `STALE_FRAMES` frames after the first frame that could build it.
pub const STALE_FRAMES: u8 = 2;

/// The roots of the game in the order of one frame: the [`Roots::PRIORITY`] roots, then the others the longest
/// waiting first (ties in [`Roots::QUEUE`] order).
fn queue_order(waited: &[u8; 16]) -> [Roots; 9] {
    let mut order = Roots::QUEUE;
    let first = Roots::PRIORITY.len();
    // A stable sort: equal waits keep the queue order.
    order[first..].sort_by_key(|r| std::cmp::Reverse(waited[r.index()]));
    order
}

fn not_built(root: &str, e: &ReadError) {
    // The root keeps its last value; the next trigger tries again (never every frame).
    tracing::debug!(target: "ui2_source", root, "not rebuilt: {e}");
}

fn write_built<R: Root>(w: &mut ModelWriter<'_>, r: Result<R, ReadError>, stats: &mut SourceStats) {
    match r {
        Ok(root) => write(w, root, stats),
        Err(e) => not_built(R::NAME, &e),
    }
}

fn write<R: Root>(w: &mut ModelWriter<'_>, root: R, stats: &mut SourceStats) {
    match w.put(root) {
        Ok(()) => stats.roots_built += 1,
        Err(e) => tracing::warn!(target: "ui2_source", "model write refused: {e}"),
    }
}

/// A read kept for the current state of the game: `None` not read yet, `Some(Err)` failed in this frame.
type Slot<T> = Option<Result<Arc<T>, ReadError>>;

/// What was read of the game since it last changed. Several roots share a record (the game, the top bar, the
/// units...), and a root rebuilt for a session or UI change finds here what an earlier rebuild read. A failed read
/// is kept only until the end of its frame, so the next trigger asks again.
#[derive(Default)]
pub(crate) struct Cache {
    info: Slot<GameInfo>,
    top: Slot<TopBar>,
    players: Slot<Vec<Player>>,
    units: Slot<Vec<Unit>>,
    cities: Slot<Vec<City>>,
    notes: Slot<Vec<Notification>>,
    /// `Q_UNIT_ACTIONS` by unit id.
    actions: BTreeMap<i32, Slot<Vec<UnitAction>>>,
    /// `Q_UNIT_PROMOTIONS` by unit id.
    promotions: BTreeMap<i32, Slot<Vec<i16>>>,
    /// `Q_CITY_BUILDS` by city id.
    builds: BTreeMap<i32, Slot<Vec<CityBuild>>>,
    /// `Q_CITY_QUEUE` by city id.
    queues: BTreeMap<i32, Slot<Vec<CityOrder>>>,
}

impl Cache {
    fn forget_errors(&mut self) {
        fn clear<T>(s: &mut Slot<T>) {
            if matches!(s, Some(Err(_))) {
                *s = None;
            }
        }
        clear(&mut self.info);
        clear(&mut self.top);
        clear(&mut self.players);
        clear(&mut self.units);
        clear(&mut self.cities);
        clear(&mut self.notes);
        self.actions.retain(|_, s| matches!(s, Some(Ok(_))));
        self.promotions.retain(|_, s| matches!(s, Some(Ok(_))));
        self.builds.retain(|_, s| matches!(s, Some(Ok(_))));
        self.queues.retain(|_, s| matches!(s, Some(Ok(_))));
    }

    /// The game turn, if the game record was read since the game last changed.
    fn turn(&self) -> Option<i32> {
        match &self.info {
            Some(Ok(i)) => Some(i.turn),
            _ => None,
        }
    }
}

/// One read through the cache: the core is asked only when the slot is empty and the meter admits the call.
fn cached<T>(
    slot: &mut Slot<T>,
    meter: &mut Meter,
    kind: ReadKind,
    f: impl FnOnce() -> Result<T, ReadError>,
) -> Result<Arc<T>, ReadError> {
    if slot.is_none() {
        let r = meter.call(kind, f)?;
        *slot = Some(r.map(Arc::new));
    }
    match slot {
        Some(Ok(v)) => Ok(v.clone()),
        Some(Err(e)) => Err(e.clone()),
        None => Err(ReadError::Core("unreachable: empty slot".into())),
    }
}

/// The answer of a read the frame budget refused. Never kept in the cache and never written: the root that got it
/// is dropped by [`CountingRead::settle`] and built again on a later frame.
fn over_budget() -> ReadError {
    ReadError::Core("the frame budget is spent".into())
}

/// The frame's count of core calls and their cost against the budget.
#[derive(Clone, Copy)]
pub(crate) struct Meter<'c> {
    /// The host's clock, for the profile.
    clock: Clock<'c>,
    /// The reads made, by kind, for the profile.
    reads: [ReadCost; READ_SLOTS],
    calls: u32,
    /// Cost units spent in this frame.
    spent: u32,
    /// Cost units the frame may spend; [`UNLIMITED`]: no limit.
    budget: u32,
    /// Every read of the current piece of work is made, whatever the budget (the first root of a frame).
    grace: bool,
    /// A read of the current piece of work was refused: its result must not be written.
    refused: bool,
    /// Calls of `city_builds` in this frame.
    builds: u32,
}

impl<'c> Meter<'c> {
    fn new(budget: u32, clock: Clock<'c>) -> Meter<'c> {
        Meter {
            clock,
            reads: [ReadCost::default(); READ_SLOTS],
            calls: 0,
            spent: 0,
            budget,
            grace: false,
            refused: false,
            builds: 0,
        }
    }

    /// `kind` fits in what is left of the budget (grace not counted).
    fn affords(&self, kind: ReadKind) -> bool {
        self.budget == UNLIMITED || self.spent.saturating_add(kind.cost()) <= self.budget
    }

    /// Admits one call of the core and counts it, or refuses it: after one refusal the rest of the current piece
    /// of work is refused too.
    fn admit(&mut self, kind: ReadKind) -> Result<(), ReadError> {
        if self.refused || !(self.grace || self.affords(kind)) {
            self.refused = true;
            return Err(over_budget());
        }
        self.spent = self.spent.saturating_add(kind.cost());
        self.calls += 1;
        if kind == ReadKind::CityBuilds {
            self.builds += 1;
        }
        Ok(())
    }

    /// Admits one call of the core ([`admit`](Self::admit)), makes it and counts its time for the profile.
    fn call<T>(&mut self, kind: ReadKind, f: impl FnOnce() -> T) -> Result<T, ReadError> {
        self.admit(kind)?;
        let (r, us) = timed(self.clock, f);
        let slot = &mut self.reads[kind.slot()];
        slot.calls += 1;
        slot.units = slot.units.saturating_add(kind.cost());
        slot.us += us;
        Ok(r)
    }
}

/// The reader of one frame: counts the core's reads and their cost ([`Meter`]), keeps them in the [`Cache`], and
/// refuses the reads that do not fit in the frame budget.
pub(crate) struct CountingRead<'a, 'r> {
    read: &'a mut (dyn GameRead + 'r),
    cache: &'a mut Cache,
    meter: Meter<'a>,
    /// The UI v2 queries known to be missing; the frame hands back what it learned.
    missing: Missing,
}

impl<'a, 'r> CountingRead<'a, 'r> {
    fn new(
        read: &'a mut (dyn GameRead + 'r),
        cache: &'a mut Cache,
        missing: Missing,
        budget: u32,
        clock: Clock<'a>,
    ) -> Self {
        CountingRead {
            read,
            cache,
            meter: Meter::new(budget, clock),
            missing,
        }
    }

    /// Starts a piece of work (a root, a catalog table): `grace` admits all its reads.
    fn begin(&mut self, grace: bool) {
        self.meter.grace = grace;
        self.meter.refused = false;
    }

    /// No call of the core was made in this frame yet.
    fn nothing_read(&self) -> bool {
        self.meter.calls == 0
    }

    /// A read of the current piece of work was refused.
    fn refused(&self) -> bool {
        self.meter.refused
    }

    /// The result of the current piece of work, unless one of its reads was refused (a builder may have taken the
    /// refusal for a missing value: such a result is incomplete and must not reach the model).
    fn settle<T>(&self, r: T) -> Option<T> {
        (!self.meter.refused).then_some(r)
    }

    /// The options of `city` were read since the game last changed (reading them again costs nothing).
    pub(crate) fn builds_known(&self, city: i32) -> bool {
        matches!(self.cache.builds.get(&city), Some(Some(_)))
    }

    /// One more city's options may be read in this frame: none was read yet, or the budget still has room for
    /// them. At the default budget that is one city a frame.
    pub(crate) fn may_read_builds(&self) -> bool {
        self.meter.builds == 0 || self.meter.affords(ReadKind::CityBuilds)
    }

    /// One UI v2 read: not made when the core is known not to serve it; an `Unsupported` answer is remembered.
    fn ui2<T>(
        &mut self,
        kind: ReadKind,
        flag: fn(&mut Missing) -> &mut bool,
        name: &'static str,
        f: impl FnOnce(&mut (dyn GameRead + 'r)) -> Result<T, ReadError>,
    ) -> Result<T, ReadError> {
        if *flag(&mut self.missing) {
            return Err(ReadError::Unsupported);
        }
        let read = &mut *self.read;
        let r = self.meter.call(kind, || f(read))?;
        if matches!(r, Err(ReadError::Unsupported)) {
            *flag(&mut self.missing) = true;
            tracing::debug!(target: "ui2_source", query = name, "the core does not serve it: the fields stay empty");
        }
        r
    }

    pub(crate) fn techs(&mut self) -> Result<Vec<TechState>, ReadError> {
        self.ui2(ReadKind::Techs, |m| &mut m.techs, "techs", |r| r.techs())
    }

    pub(crate) fn player_resources(&mut self) -> Result<Vec<PlayerResource>, ReadError> {
        self.ui2(
            ReadKind::PlayerResources,
            |m| &mut m.resources,
            "player_resources",
            |r| r.player_resources(),
        )
    }

    pub(crate) fn policy_branches(&mut self) -> Result<Vec<PolicyBranchState>, ReadError> {
        self.ui2(
            ReadKind::PolicyBranches,
            |m| &mut m.branches,
            "policy_branches",
            |r| r.policy_branches(),
        )
    }

    pub(crate) fn seat(&self) -> u8 {
        self.read.seat()
    }

    pub(crate) fn game_info(&mut self) -> Result<Arc<GameInfo>, ReadError> {
        let read = &mut *self.read;
        cached(
            &mut self.cache.info,
            &mut self.meter,
            ReadKind::GameInfo,
            || read.game_info(),
        )
    }

    pub(crate) fn top_bar(&mut self) -> Result<Arc<TopBar>, ReadError> {
        let read = &mut *self.read;
        cached(
            &mut self.cache.top,
            &mut self.meter,
            ReadKind::TopBar,
            || read.top_bar(),
        )
    }

    pub(crate) fn players(&mut self) -> Result<Arc<Vec<Player>>, ReadError> {
        let read = &mut *self.read;
        cached(
            &mut self.cache.players,
            &mut self.meter,
            ReadKind::Players,
            || read.players(),
        )
    }

    pub(crate) fn units(&mut self) -> Result<Arc<Vec<Unit>>, ReadError> {
        let read = &mut *self.read;
        cached(
            &mut self.cache.units,
            &mut self.meter,
            ReadKind::Units,
            || read.units(),
        )
    }

    pub(crate) fn cities(&mut self) -> Result<Arc<Vec<City>>, ReadError> {
        let read = &mut *self.read;
        cached(
            &mut self.cache.cities,
            &mut self.meter,
            ReadKind::Cities,
            || read.cities(),
        )
    }

    pub(crate) fn notifications(&mut self) -> Result<Arc<Vec<Notification>>, ReadError> {
        let read = &mut *self.read;
        cached(
            &mut self.cache.notes,
            &mut self.meter,
            ReadKind::Notifications,
            || read.notifications(),
        )
    }

    pub(crate) fn unit_actions(&mut self, unit: i32) -> Result<Arc<Vec<UnitAction>>, ReadError> {
        let read = &mut *self.read;
        let slot = self.cache.actions.entry(unit).or_default();
        cached(slot, &mut self.meter, ReadKind::UnitActions, || {
            read.unit_actions(unit)
        })
    }

    /// `Q_UNIT_PROMOTIONS`, a UI v2 query: kept like the others, and not asked again once unsupported.
    pub(crate) fn unit_promotions(&mut self, unit: i32) -> Result<Arc<Vec<i16>>, ReadError> {
        if self.missing.promotions {
            return Err(ReadError::Unsupported);
        }
        let read = &mut *self.read;
        let slot = self.cache.promotions.entry(unit).or_default();
        let r = cached(slot, &mut self.meter, ReadKind::UnitPromotions, || {
            read.unit_promotions(unit)
        });
        if matches!(r, Err(ReadError::Unsupported)) {
            self.missing.promotions = true;
        }
        r
    }

    pub(crate) fn city_builds(&mut self, city: i32) -> Result<Arc<Vec<CityBuild>>, ReadError> {
        let read = &mut *self.read;
        let slot = self.cache.builds.entry(city).or_default();
        cached(slot, &mut self.meter, ReadKind::CityBuilds, || {
            read.city_builds(city)
        })
    }

    /// `Q_CITY_QUEUE`, a UI v2 query: kept like the others, and not asked again once unsupported.
    pub(crate) fn city_queue(&mut self, city: i32) -> Result<Arc<Vec<CityOrder>>, ReadError> {
        if self.missing.queue {
            return Err(ReadError::Unsupported);
        }
        let read = &mut *self.read;
        let slot = self.cache.queues.entry(city).or_default();
        let r = cached(slot, &mut self.meter, ReadKind::CityQueue, || {
            read.city_queue(city)
        });
        if matches!(r, Err(ReadError::Unsupported)) {
            self.missing.queue = true;
        }
        r
    }

    pub(crate) fn catalog(&mut self, t: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError> {
        let read = &mut *self.read;
        self.meter.call(ReadKind::Catalog(t), || read.catalog(t))?
    }
}
