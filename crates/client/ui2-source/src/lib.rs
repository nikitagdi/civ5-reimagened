//! UI v2 source: the view-model roots built from the replica's game.
//!
//! [`UiSource::frame`] runs once per frame. It compares the replica's change signals (state, generation, last
//! applied batch, the pump's steps) with the previous frame's and rebuilds only the roots a change asks for; with
//! no change it reads nothing and writes nothing. Roots are written whole through the store's
//! [`ModelWriter`]; the store diffs them.
//!
//! Read only by construction: the builders see the game as `&mut dyn replica::GameRead` (typed queries, no
//! setters, no UI operations) and nothing else of the replica. [`ReplicaView::of`] is the one place that touches
//! a [`Replica`], and it only takes its read-only signals and its reader.

pub mod build;
pub mod catalog;
pub mod trigger;

use std::sync::Arc;

use replica::{
    CatalogEntry, CatalogTable, GameInfo, GameRead, GameSide, Player, PlayerResource,
    PolicyBranchState, ReadError, Replica, ReplicaStep, State, TechState, TopBar,
};
use ui2_model::{ModelWriter, Root, Schema};

use crate::catalog::Catalog;
use crate::trigger::{Roots, Seen};

/// The replica as one frame of the source sees it: its change signals and, while it can be read, its reader.
pub struct ReplicaView<'a> {
    pub state: State,
    /// The replica still has journal batches to apply (or no base yet): what it shows is the past.
    pub catching_up: bool,
    /// Grows on every snapshot load and resync.
    pub generation: u64,
    /// The last journal batch applied.
    pub applied_jseq: u64,
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
        ReplicaView {
            state,
            catching_up,
            generation,
            applied_jseq,
            read: r.read(),
        }
    }

    fn seen(&self) -> Seen {
        Seen {
            state: self.state,
            catching_up: self.catching_up,
            generation: self.generation,
            applied_jseq: self.applied_jseq,
        }
    }
}

/// The input of one frame, made by the host after it pumped the replica.
pub struct SourceFrame<'a> {
    /// `None`: there is no replica (lobby, another UI).
    pub replica: Option<ReplicaView<'a>>,
    /// What the replica's pump did in this frame.
    pub steps: &'a [ReplicaStep],
    /// The host's clock, ms (the turn timer root uses it).
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
    /// Roots that wait for a later frame (the replica cannot be read yet).
    pub deferred: u32,
}

/// The replica adapter: rebuilds the game-data roots on change triggers.
pub struct UiSource {
    /// The schema version, as `meta.schema` carries it.
    schema_text: Arc<str>,
    /// The replica's signals as of the last frame that had a replica.
    seen: Option<Seen>,
    /// Roots asked for and not built yet.
    pending: Roots,
    /// The last `meta` written, so an unchanged one is not written again.
    meta: Option<ui2_model::r#gen::Meta>,
    /// Read once per core; `None` until a rebuild needs it (or after a failed read).
    catalog: Option<Catalog>,
    /// The UI v2 queries the core does not serve: not asked again until [`UiSource::reset`].
    missing: Missing,
}

/// The UI v2 queries a core answered `ReadError::Unsupported` to (the stub, a core older than the queries). Their
/// fields stay at the schema's defaults; asking again on every trigger would only cost reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Missing {
    techs: bool,
    resources: bool,
    branches: bool,
}

impl UiSource {
    #[must_use]
    pub fn new(schema: &'static Schema) -> UiSource {
        UiSource {
            schema_text: Arc::from(schema.version().to_string()),
            seen: None,
            pending: Roots::NONE,
            meta: None,
            catalog: None,
            missing: Missing::default(),
        }
    }

    /// Forgets everything (a new game or a new core): the next frame rebuilds every root, reads the catalog and
    /// asks the UI v2 queries again.
    pub fn reset(&mut self) {
        self.seen = None;
        self.pending = Roots::NONE;
        self.meta = None;
        self.catalog = None;
        self.missing = Missing::default();
    }

    /// One frame: rebuild what the replica's signals ask for, write it, report the cost.
    pub fn frame(&mut self, f: SourceFrame<'_>, w: &mut ModelWriter<'_>) -> SourceStats {
        let mut stats = SourceStats::default();
        let SourceFrame { replica, steps, .. } = f;

        let now = replica.as_ref().map(ReplicaView::seen);
        let meta = build::game::meta(&self.schema_text, now.as_ref());
        if self.meta.as_ref() != Some(&meta) {
            self.write(w, meta.clone(), &mut stats);
            self.meta = Some(meta);
        }

        let Some(mut view) = replica else {
            return stats;
        };
        let now = view.seen();
        self.pending = self
            .pending
            .union(trigger::changes(self.seen.as_ref(), &now, steps));
        self.seen = Some(now);
        if self.pending.is_empty() {
            return stats;
        }
        let read = match view.read.as_deref_mut() {
            Some(read) if now.readable() => read,
            _ => {
                stats.deferred = self.pending.len();
                return stats;
            }
        };

        let want = std::mem::take(&mut self.pending);
        let mut read = CountingRead::new(read, self.missing);
        // Every root of this source shows catalog names (eras, technologies, resources, policy branches).
        if !want.is_empty() && self.catalog.is_none() {
            match Catalog::load(&mut read) {
                Ok(c) => self.catalog = Some(c),
                Err(e) => tracing::debug!(target: "ui2_source", "catalog: {e}"),
            }
        }
        let empty = Catalog::default();
        let catalog = self.catalog.as_ref().unwrap_or(&empty);

        // The order of the budget queue: the top bar first, then research, then the game root.
        if want.contains(Roots::TOP) {
            let r = build::top::top(&mut read, catalog);
            self.write_built(w, r, &mut stats);
        }
        if want.contains(Roots::RESEARCH) {
            let r = build::top::research(&mut read, catalog);
            self.write_built(w, r, &mut stats);
        }
        if want.contains(Roots::GAME) {
            let r = build::game::game(&mut read, catalog);
            self.write_built(w, r, &mut stats);
        }
        stats.getter_calls = read.calls;
        self.missing = read.missing;
        stats
    }

    fn write_built<R: Root>(
        &self,
        w: &mut ModelWriter<'_>,
        r: Result<R, ReadError>,
        stats: &mut SourceStats,
    ) {
        match r {
            Ok(root) => self.write(w, root, stats),
            // The root keeps its last value; the next trigger tries again (never every frame).
            Err(e) => tracing::debug!(target: "ui2_source", root = R::NAME, "not rebuilt: {e}"),
        }
    }

    fn write<R: Root>(&self, w: &mut ModelWriter<'_>, root: R, stats: &mut SourceStats) {
        match w.put(root) {
            Ok(()) => stats.roots_built += 1,
            Err(e) => tracing::warn!(target: "ui2_source", "model write refused: {e}"),
        }
    }
}

/// The reader of one frame: counts the reads and keeps the two records several roots share, so a frame asks the
/// core for each at most once.
pub(crate) struct CountingRead<'a, 'r> {
    read: &'a mut (dyn GameRead + 'r),
    calls: u32,
    info: Option<Result<GameInfo, ReadError>>,
    top: Option<Result<TopBar, ReadError>>,
    /// The UI v2 queries known to be missing; the frame hands back what it learned.
    missing: Missing,
}

impl<'a, 'r> CountingRead<'a, 'r> {
    fn new(read: &'a mut (dyn GameRead + 'r), missing: Missing) -> Self {
        CountingRead {
            read,
            calls: 0,
            info: None,
            top: None,
            missing,
        }
    }

    /// One UI v2 read: not made when the core is known not to serve it; an `Unsupported` answer is remembered.
    fn ui2<T>(
        &mut self,
        flag: fn(&mut Missing) -> &mut bool,
        name: &'static str,
        f: impl FnOnce(&mut (dyn GameRead + 'r)) -> Result<T, ReadError>,
    ) -> Result<T, ReadError> {
        if *flag(&mut self.missing) {
            return Err(ReadError::Unsupported);
        }
        self.calls += 1;
        let r = f(&mut *self.read);
        if matches!(r, Err(ReadError::Unsupported)) {
            *flag(&mut self.missing) = true;
            tracing::debug!(target: "ui2_source", query = name, "the core does not serve it: the fields stay empty");
        }
        r
    }

    pub(crate) fn techs(&mut self) -> Result<Vec<TechState>, ReadError> {
        self.ui2(|m| &mut m.techs, "techs", |r| r.techs())
    }

    pub(crate) fn player_resources(&mut self) -> Result<Vec<PlayerResource>, ReadError> {
        self.ui2(
            |m| &mut m.resources,
            "player_resources",
            |r| r.player_resources(),
        )
    }

    pub(crate) fn policy_branches(&mut self) -> Result<Vec<PolicyBranchState>, ReadError> {
        self.ui2(
            |m| &mut m.branches,
            "policy_branches",
            |r| r.policy_branches(),
        )
    }

    pub(crate) fn seat(&self) -> u8 {
        self.read.seat()
    }

    pub(crate) fn game_info(&mut self) -> Result<&GameInfo, ReadError> {
        if self.info.is_none() {
            self.calls += 1;
            self.info = Some(self.read.game_info());
        }
        match &self.info {
            Some(Ok(i)) => Ok(i),
            Some(Err(e)) => Err(e.clone()),
            None => Err(ReadError::Core("unreachable: no game info".into())),
        }
    }

    pub(crate) fn top_bar(&mut self) -> Result<&TopBar, ReadError> {
        if self.top.is_none() {
            self.calls += 1;
            self.top = Some(self.read.top_bar());
        }
        match &self.top {
            Some(Ok(t)) => Ok(t),
            Some(Err(e)) => Err(e.clone()),
            None => Err(ReadError::Core("unreachable: no top bar".into())),
        }
    }

    pub(crate) fn players(&mut self) -> Result<Vec<Player>, ReadError> {
        self.calls += 1;
        self.read.players()
    }

    pub(crate) fn catalog(&mut self, t: CatalogTable) -> Result<Vec<CatalogEntry>, ReadError> {
        self.calls += 1;
        self.read.catalog(t)
    }
}
