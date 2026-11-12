//! The game session of the client: what every game UI reads and none of them owns.
//!
//! One [`GameSession`] lives in the app for as long as a game UI that reads the replica is registered, whichever
//! UI is on the screen. It owns:
//! - the game data of the replica's core (`gc:replica`: the group table, the VFS index first, then the databases
//!   and the Lua of the add-ins), fetched through the asset loader ([`DataBoot`]);
//! - the game's texts (`ui:text:<lang>` and the set's `ui:tables`) and the localizer made of them ([`TextBoot`]);
//! - the replica: made once the data is in and the seat of this client is known, fed by the network (replica
//!   messages, bodies) and pumped each frame within a budget; the UI's messages to the server go out here;
//! - the drain of the core's UI events while no game UI dispatches them (the queue is destructive and would grow);
//! - the classic UI set (`ui:index`, the boot groups, then the textures asked for in the background: [`UiBoot`])
//!   and the textures the renderer asks of it after each frame ([`GameSession::ui_textures`]): the classic layer
//!   reads its files, and UI v2 its icon atlases, whichever UI is on the screen.
//!
//! [`GameSession::frame`] runs before the game UI's frame and reports what the pump did ([`SessionStep`]).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use draw_list::TextureId;
use platform::Clock;
use replica::{
    GameDataHost, GameFiles, GameSide, Outgoing, Replica, ReplicaMsg, ReplicaStep,
    State as ReplicaState, UiEvent,
};
use ui_client::{BootPlan, BootState, ReadyTexture, UiBoot, UiStore};

use crate::net::NetHandle;

/// Frame budget of one `Replica::pump` step (ms).
const PUMP_BUDGET_MS: u32 = 8;
/// At most this many pump steps a frame (a turn change is one step of seconds on a big game).
const PUMP_STEPS: usize = 16;
/// The language of the game's texts. SIMPLIFIED(SIMP-268): one language per session (as the classic layer's),
/// chosen at start; only a skin's own texts change language on the fly.
pub const TEXT_LANG: &str = "en_US";

/// `Box<dyn Clock>` over the app's clock.
struct SharedClock(Rc<dyn Clock>);

impl Clock for SharedClock {
    fn now_ms(&self) -> u64 {
        self.0.now_ms()
    }

    fn now_ms_f64(&self) -> f64 {
        self.0.now_ms_f64()
    }
}

enum DataStage {
    Table(AssetHandle),
    Files {
        queue: Vec<LogicalId>,
        pending: Vec<(LogicalId, AssetHandle)>,
        total: usize,
        done: usize,
    },
    Ready,
    Failed(String),
}

/// Fetches the `gc:replica` members (the group table, the index first, the databases, the Lua of the add-ins).
struct DataBoot {
    stage: Option<DataStage>,
    files: Arc<Mutex<GameFiles>>,
    have_index: bool,
    /// The files of the core, filled when the index is in.
    started_ms: Option<u64>,
}

impl DataBoot {
    fn new() -> DataBoot {
        DataBoot {
            stage: None,
            files: Arc::new(Mutex::new(GameFiles::default())),
            have_index: false,
            started_ms: None,
        }
    }

    fn ready(&self) -> bool {
        matches!(self.stage, Some(DataStage::Ready))
    }

    fn failed(&self) -> Option<&str> {
        match &self.stage {
            Some(DataStage::Failed(m)) => Some(m),
            _ => None,
        }
    }

    fn progress(&self) -> (usize, usize) {
        match &self.stage {
            Some(DataStage::Files { done, total, .. }) => (*done, *total),
            Some(DataStage::Ready) => (1, 1),
            _ => (0, 0),
        }
    }

    fn update(&mut self, loader: &mut AssetLoader, now_ms: u64) {
        let profile = ui_client::ui_profile();
        let stage = match self.stage.take() {
            None => {
                self.started_ms = Some(now_ms);
                DataStage::Table(loader.request(LogicalId::new(assets::GROUP_TABLE_ID), &profile))
            }
            Some(s) => s,
        };
        self.stage = Some(match stage {
            DataStage::Table(h) => match loader.poll(h) {
                AssetPoll::Pending => DataStage::Table(h),
                AssetPoll::Failed(e) => DataStage::Failed(format!("group table: {e}")),
                AssetPoll::Ready(bytes) => match replica::group_members(&bytes) {
                    Ok(ids) => {
                        let queue: Vec<LogicalId> =
                            ids.into_iter().rev().map(LogicalId::new).collect();
                        let total = queue.len();
                        DataStage::Files {
                            queue,
                            pending: Vec::new(),
                            total,
                            done: 0,
                        }
                    }
                    Err(e) => DataStage::Failed(e.to_string()),
                },
            },
            DataStage::Files {
                mut queue,
                mut pending,
                total,
                mut done,
            } => {
                let mut failed = None;
                let files = Arc::clone(&self.files);
                let have_index = &mut self.have_index;
                // The index first: nothing else can be put before `GameFiles::from_index`.
                pending.retain(|(id, h)| match loader.poll(*h) {
                    AssetPoll::Pending => true,
                    AssetPoll::Ready(bytes) => {
                        done += 1;
                        let Ok(mut g) = files.lock() else {
                            return false;
                        };
                        if id.as_str() == replica::VFS_INDEX_ID {
                            match GameFiles::from_index(bytes) {
                                Ok(f) => {
                                    *g = f;
                                    *have_index = true;
                                }
                                Err(e) => {
                                    failed.get_or_insert(format!("vfs-index: {e}"));
                                }
                            }
                        } else {
                            g.put(id.as_str(), bytes);
                        }
                        false
                    }
                    AssetPoll::Failed(e) => {
                        failed.get_or_insert(format!("{}: {e}", id.as_str()));
                        false
                    }
                });
                if let Some(m) = failed {
                    DataStage::Failed(m)
                } else {
                    // Until the index is in, only the index is in flight.
                    let limit = if *have_index { 24 } else { 1 };
                    while pending.len() < limit {
                        let Some(id) = queue.pop() else { break };
                        let h = loader.request(id.clone(), &profile);
                        pending.push((id, h));
                    }
                    if pending.is_empty() && queue.is_empty() {
                        tracing::info!(files = total, "the replica game data is loaded");
                        DataStage::Ready
                    } else {
                        DataStage::Files {
                            queue,
                            pending,
                            total,
                            done,
                        }
                    }
                }
            }
            s @ (DataStage::Ready | DataStage::Failed(_)) => s,
        });
    }
}

/// The texts of one language over the `C5LT` table of the set.
struct TextRows(render::asset_format::LocaleTable);

impl loc::TextTable for TextRows {
    fn get(&self, tag: &str) -> Option<loc::TextEntry<'_>> {
        let text = self.0.get(tag)?;
        let (gender, plurality) = self.0.grammar_of(tag);
        Some(loc::TextEntry {
            text,
            gender,
            plurality,
        })
    }
}

/// The localizer of `lang` from its text table and the set's tables (the plural rule), as the classic layer
/// makes it from its store.
pub(crate) fn make_localizer(
    text: &[u8],
    tables: Option<&[u8]>,
    lang: &str,
) -> Result<loc::Localizer, String> {
    let texts = assets::decode_texts(text).map_err(|e| e.to_string())?;
    let tables = tables
        .and_then(|b| render::asset_format::UiTables::decode(b).ok())
        .unwrap_or_default();
    let info = match tables.languages.iter().find(|l| l.lang == lang) {
        Some(l) => loc::LanguageInfo::new(lang, lang, l.plural_rule),
        // SIMPLIFIED(SIMP-uicl-4): the game data gives only en_US its `PluralRule`; another language composes by rule 2.
        None => loc::LanguageInfo::new(lang, lang, 2),
    };
    Ok(loc::Localizer::new(info, Box::new(TextRows(texts))))
}

/// Fetches the game's texts of [`TEXT_LANG`] and the set's tables, then makes the localizer once.
struct TextBoot {
    text: Option<AssetHandle>,
    tables: Option<AssetHandle>,
    text_bytes: Option<Vec<u8>>,
    tables_bytes: Option<Option<Vec<u8>>>,
    started: bool,
    settled: bool,
    localizer: Option<Rc<loc::Localizer>>,
}

impl TextBoot {
    fn new() -> TextBoot {
        TextBoot {
            text: None,
            tables: None,
            text_bytes: None,
            tables_bytes: None,
            started: false,
            settled: false,
            localizer: None,
        }
    }

    fn update(&mut self, loader: &mut AssetLoader) {
        if self.settled {
            return;
        }
        let profile = ui_client::ui_profile();
        if !self.started {
            self.started = true;
            self.text = Some(loader.request(assets::UiFiles::text_id(TEXT_LANG), &profile));
            self.tables =
                Some(loader.request(LogicalId::new(render::asset_format::UI_TABLES_ID), &profile));
        }
        if let Some(h) = self.text {
            match loader.poll(h) {
                AssetPoll::Pending => {}
                AssetPoll::Ready(bytes) => {
                    self.text = None;
                    self.text_bytes = Some(bytes.to_vec());
                }
                AssetPoll::Failed(e) => {
                    self.text = None;
                    self.settle(Err(format!(
                        "text table {} is not loaded: {e}",
                        assets::UiFiles::text_id(TEXT_LANG).as_str()
                    )));
                    return;
                }
            }
        }
        if let Some(h) = self.tables {
            match loader.poll(h) {
                AssetPoll::Pending => {}
                AssetPoll::Ready(bytes) => {
                    self.tables = None;
                    self.tables_bytes = Some(Some(bytes.to_vec()));
                }
                // Without the tables the plural rule is the default one (as a store without them).
                AssetPoll::Failed(_) => {
                    self.tables = None;
                    self.tables_bytes = Some(None);
                }
            }
        }
        if let (Some(text), Some(tables)) = (&self.text_bytes, &self.tables_bytes) {
            let made = make_localizer(text, tables.as_deref(), TEXT_LANG);
            self.text_bytes = None;
            self.tables_bytes = None;
            self.settle(made.map(Rc::new));
        }
    }

    fn settle(&mut self, made: Result<Rc<loc::Localizer>, String>) {
        self.settled = true;
        match made {
            Ok(l) => self.localizer = Some(l),
            Err(e) => tracing::warn!("the texts for Locale are not loaded: {e}"),
        }
    }
}

/// What the app gives a session frame.
pub struct SessionEnv<'a> {
    pub now_ms: u64,
    pub loader: Option<&'a mut AssetLoader>,
    pub net: Option<&'a NetHandle>,
    pub clock: &'a Rc<dyn Clock>,
    /// This client's seat in the game (0 until the attach names it); the replica's active player.
    pub seat: i32,
    /// The attach came: the seat is known, so the core may be made.
    pub seat_known: bool,
    /// No game UI dispatches the core's UI events this frame: the session drops them.
    pub drain_events: bool,
}

/// What one session frame did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionStep {
    /// The replica is `Live` (a game is loaded and in step with the server).
    pub live: bool,
    /// The pump's steps of this frame, in order (the model source's change signal).
    pub steps: Vec<ReplicaStep>,
    /// The `EndTurn` orders of the UI that went out this frame (the classic UI follows their answers).
    pub end_turns: Vec<protocol::CmdId>,
    /// The replica's turn when the frame's messages went out.
    pub turn: i32,
}

/// The game session: the replica, its data and its network, and the game's localizer (see the module docs).
pub struct GameSession {
    data: DataBoot,
    text: TextBoot,
    replica: Option<Rc<RefCell<Replica>>>,
    backlog: Vec<ReplicaMsg>,
    blob_wanted: BTreeSet<u64>,
    resync_flush: Vec<Outgoing>,
    seat: i32,
    failed: Option<String>,
    live: bool,
    /// Time spent in `gc_create` (ms), for the report.
    create_ms: f64,
    /// The core's UI events dropped while no game UI dispatched them.
    drained: u64,
    events: Vec<UiEvent>,
    /// The classic UI set and where its boot is.
    ui_set: UiBoot,
    ui_state: BootState,
    /// Textures the renderer asked for that are on their way (answered by a later call), and those the set did not
    /// have (logged once each).
    tex_waiting: BTreeSet<TextureId>,
    tex_unresolved: BTreeSet<u64>,
}

impl Default for GameSession {
    fn default() -> Self {
        GameSession::new()
    }
}

impl GameSession {
    pub fn new() -> GameSession {
        GameSession {
            data: DataBoot::new(),
            text: TextBoot::new(),
            replica: None,
            backlog: Vec::new(),
            blob_wanted: BTreeSet::new(),
            resync_flush: Vec::new(),
            seat: 0,
            failed: None,
            live: false,
            create_ms: 0.0,
            drained: 0,
            events: Vec::new(),
            ui_set: UiBoot::new(BootPlan::m2(exe_api::m2_contexts())),
            ui_state: BootState::Loading { done: 0, total: 0 },
            tex_waiting: BTreeSet::new(),
            tex_unresolved: BTreeSet::new(),
        }
    }

    /// The classic UI set, once it is loaded.
    pub fn ui_store(&self) -> Option<Rc<UiStore>> {
        self.ui_set.store()
    }

    /// Where the boot of the classic UI set is (as of the last frame).
    pub fn ui_state(&self) -> &BootState {
        &self.ui_state
    }

    /// The variants of the textures the device takes (`Renderer::platform_profile`), for the set's fetches from now
    /// on: a phone gets their `Astc` variants.
    pub fn set_ui_profile(&mut self, profile: &assets::PlatformProfile) {
        if self.ui_set.profile() != profile {
            self.ui_set.set_profile(profile.clone());
        }
    }

    /// After the renderer drew: the textures of the set it asked for (`UiTextures::take_wanted`), ready to upload. A
    /// texture that is on its way is kept and answered by a later call; one the set does not have is left out.
    pub fn ui_textures(&mut self, wanted: &[TextureId]) -> Vec<ReadyTexture> {
        let mut ids = std::mem::take(&mut self.tex_waiting);
        ids.extend(wanted.iter().copied());
        let Some(store) = self.ui_set.store() else {
            // Before the set is in, every ask waits for it.
            self.tex_waiting = ids;
            return Vec::new();
        };
        let mut out = Vec::new();
        for id in ids {
            let Some(blob) = store.texture_blob(id) else {
                if store.is_loading(id) {
                    self.tex_waiting.insert(id);
                } else if self.tex_unresolved.insert(id.0) {
                    tracing::warn!(
                        texture = id.0,
                        "the renderer wants a ui texture the set did not give"
                    );
                }
                continue;
            };
            let (format, bytes) = ui_client::upload_form(&blob, store.is_font_texture(id));
            out.push(ReadyTexture {
                id,
                width: blob.width,
                height: blob.height,
                content: store.texture_content(id, &blob),
                format,
                bytes,
            });
        }
        out
    }

    /// The replica, once the core is made.
    pub fn replica(&self) -> Option<Rc<RefCell<Replica>>> {
        self.replica.clone()
    }

    /// The game's localizer, once its texts are in (`None` before, or when they failed to load).
    pub fn localizer(&self) -> Option<Rc<loc::Localizer>> {
        self.text.localizer.clone()
    }

    /// The texts are loaded or failed: [`Self::localizer`] will not change any more.
    pub fn text_settled(&self) -> bool {
        self.text.settled
    }

    /// Why the session cannot run a game (the game data or the core failed); it does nothing more.
    pub fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }

    /// The replica was `Live` in the last frame.
    pub fn live(&self) -> bool {
        self.live
    }

    /// Files of the game data fetched and their number.
    pub fn data_progress(&self) -> (usize, usize) {
        self.data.progress()
    }

    /// Time spent in `gc_create` (ms).
    pub fn create_ms(&self) -> f64 {
        self.create_ms
    }

    /// The core's UI events dropped so far while no game UI dispatched them.
    pub fn drained_events(&self) -> u64 {
        self.drained
    }

    /// The player did something: idle checks of the replica wait.
    pub fn note_input(&self) {
        if let Some(r) = &self.replica
            && let Ok(mut r) = r.try_borrow_mut()
        {
            r.note_input();
        }
    }

    /// The game was left: what waited for the old game goes. The core of the replica boots once per process, so
    /// it stays and takes the next game's snapshot in place.
    pub fn new_game(&mut self) {
        self.backlog.clear();
        self.resync_flush.clear();
        self.blob_wanted.clear();
        self.tex_unresolved.clear();
    }

    /// One frame: the fetches, the network (replica messages and bodies in, the UI's messages out), the core once
    /// the data is in and the seat is known, the pump.
    pub fn frame(&mut self, env: SessionEnv<'_>) -> SessionStep {
        let SessionEnv {
            now_ms,
            loader,
            net,
            clock,
            seat,
            seat_known,
            drain_events,
        } = env;
        let mut step = SessionStep::default();
        if self.failed.is_some() {
            return step;
        }
        self.seat = seat;
        if let Some(loader) = loader {
            self.data.update(loader, now_ms);
            self.text.update(loader);
            // The UI set and its background textures, whichever game UI is on the screen.
            self.ui_state = self.ui_set.update(loader);
        }
        if let Some(m) = self.data.failed() {
            self.failed = Some(format!("the game data: {m}"));
            return step;
        }
        self.network(net, &mut step);
        if self.replica.is_none() && self.data.ready() && seat_known {
            self.make_replica(clock);
            if self.failed.is_some() {
                return step;
            }
        }
        self.live = self.pump(&mut step.steps);
        step.live = self.live;
        if drain_events {
            self.drain_events();
        }
        step
    }

    fn make_replica(&mut self, clock: &Rc<dyn Clock>) {
        let t0 = clock.now_ms_f64();
        let host = GameDataHost::new(Arc::clone(&self.data.files));
        match Replica::new(host, self.replica_seat()) {
            Ok(mut r) => {
                r.set_clock(Box::new(SharedClock(Rc::clone(clock))));
                self.create_ms = clock.now_ms_f64() - t0;
                tracing::info!(ms = self.create_ms, "replica core created");
                self.replica = Some(Rc::new(RefCell::new(r)));
            }
            Err(e) => self.failed = Some(format!("the replica core: {e}")),
        }
    }

    /// The seat of this client's player in the game; the core takes it as the active player.
    fn replica_seat(&self) -> u8 {
        self.seat.clamp(0, 63) as u8
    }

    fn network(&mut self, net: Option<&NetHandle>, step: &mut SessionStep) {
        let _slow = render::perf::slow::scope("m2.network");
        let Some(net) = net else { return };
        let msgs = net.with_client(|c| c.take_replica_msgs());
        if !msgs.is_empty() {
            match self.replica.as_ref().map(|r| r.try_borrow_mut()) {
                Some(Ok(mut r)) => {
                    r.engine_mut().set_seat(self.replica_seat());
                    for m in msgs {
                        r.on_msg(m);
                    }
                }
                // No replica yet, or it is busy: the messages wait (a dropped journal batch is a gap and a resync).
                _ => self.backlog.extend(msgs),
            }
        }
        let Some(rep) = self.replica.clone() else {
            return;
        };
        let Ok(mut r) = rep.try_borrow_mut() else {
            return;
        };
        if !self.backlog.is_empty() {
            for m in std::mem::take(&mut self.backlog) {
                r.on_msg(m);
            }
        }
        for b in r.wanted_blobs() {
            self.blob_wanted.insert(b.hash);
            net.with_client(|c| c.fetch_replica_blob(b.hash));
        }
        for (hash, body) in net.with_client(|c| c.poll_replica_blobs()) {
            self.blob_wanted.remove(&hash);
            match body {
                Some(bytes) => r.blob(hash, bytes),
                None => r.blob_failed(hash),
            }
        }
        // The UI's messages go out; what cannot be sent now stays (AC-G2).
        let mut out = std::mem::take(&mut self.resync_flush);
        out.extend(r.take_outgoing());
        let turn_now = r.turn();
        step.turn = turn_now;
        let mut stale_dropped = 0u32;
        if !out.is_empty() {
            let mut back = Vec::new();
            for o in out {
                let sent = net.with(|core| match &o {
                    Outgoing::GcNet(bytes) => {
                        match core.send_gc_net_at(bytes.clone(), turn_now) {
                            Ok(()) => true,
                            Err(client_core::OrderRefused::TurnChanged) => {
                                // Given on a turn the server has left: it does nothing, it is not kept (B1).
                                stale_dropped += 1;
                                true
                            }
                            Err(_) => false,
                        }
                    }
                    Outgoing::EndTurn => {
                        match core
                            .order_at(protocol::Command::EndTurn { force: false }, Some(turn_now))
                        {
                            Ok(id) => {
                                step.end_turns.push(id);
                                true
                            }
                            Err(_) => false,
                        }
                    }
                    Outgoing::CancelEndTurn => core
                        .order_at(protocol::Command::CancelEndTurn, Some(turn_now))
                        .is_ok(),
                    Outgoing::Resync {
                        epoch,
                        have_raw_hash,
                        at_jseq,
                        reason,
                    } => core
                        .send_gc_resync(*epoch, *have_raw_hash, *at_jseq, *reason)
                        .is_ok(),
                });
                if !sent {
                    back.push(o);
                }
            }
            if !back.is_empty() {
                r.unsend(back);
            }
        }
        if stale_dropped > 0 {
            tracing::info!(
                target: "m2",
                "{stale_dropped} UI message(s) given on turn {turn_now} dropped: the server is in a later turn"
            );
        }
    }

    /// Applies what is due; `true` when the replica is `Live`.
    fn pump(&mut self, steps: &mut Vec<ReplicaStep>) -> bool {
        let _slow = render::perf::slow::scope("m2.pump_replica");
        let Some(rep) = self.replica.clone() else {
            return false;
        };
        let Ok(mut r) = rep.try_borrow_mut() else {
            return self.live;
        };
        // A turn change is one step of seconds on a big game: the pump does at most one stage per frame.
        for _ in 0..PUMP_STEPS {
            let step = r.pump(PUMP_BUDGET_MS);
            steps.push(step);
            if matches!(
                step,
                ReplicaStep::Loaded { .. } | ReplicaStep::TurnSwitched { .. }
            ) {
                tracing::info!(?step, "replica loaded a game");
            }
            if !matches!(step, ReplicaStep::Working) {
                break;
            }
        }
        r.state() == ReplicaState::Live
    }

    /// No game UI dispatches the core's UI events: they are dropped (the queue has one consumer and would grow).
    fn drain_events(&mut self) {
        let Some(rep) = &self.replica else { return };
        let Ok(mut r) = rep.try_borrow_mut() else {
            return;
        };
        r.take_events(&mut self.events);
        self.drained += self.events.len() as u64;
        self.events.clear();
    }

    /// `__civ5stats.replica`: the replica's counters (`None` before the core is made, or while it is busy).
    pub fn replica_stats(&self) -> Option<Vec<(String, f64)>> {
        let r = self.replica.as_ref()?.try_borrow().ok()?;
        let s = r.stats();
        let n = |k: &str, v: f64| (k.to_owned(), v);
        Some(vec![
            n("applied", s.applied as f64),
            n("diverged", s.diverged as f64),
            n("resyncs", s.resyncs as f64),
            n("checks_skipped", s.checks_skipped as f64),
            n("checks_run", s.checks_run as f64),
            n("net_sent", s.net_sent as f64),
            n("net_rejected", s.net_rejected as f64),
            n("turn_reopened", s.turn_reopened as f64),
            n("loads", s.loads as f64),
            n("turn_switch_ms", s.turn_switch_ms),
            n("turn_wait_ms", s.turn_wait_ms),
            n("turn_patch", s.turn_patch as f64),
            n("turn_snapshot", s.turn_snapshot as f64),
            n("patch_fallbacks", s.patch_fallbacks as f64),
            n("last_body_bytes", s.last_body_bytes as f64),
            n("join_ms", s.join_ms),
            n("memory_mb", f64::from(s.memory_mb)),
            n("queued", f64::from(s.queued)),
            n(
                "live",
                if r.state() == ReplicaState::Live {
                    1.0
                } else {
                    0.0
                },
            ),
            n("turn", f64::from(r.turn())),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Zero;

    impl Clock for Zero {
        fn now_ms(&self) -> u64 {
            0
        }
    }

    #[test]
    fn a_session_without_assets_waits_and_has_nothing() {
        let clock: Rc<dyn Clock> = Rc::new(Zero);
        let mut s = GameSession::new();
        let step = s.frame(SessionEnv {
            now_ms: 0,
            loader: None,
            net: None,
            clock: &clock,
            seat: 0,
            seat_known: true,
            drain_events: true,
        });
        assert_eq!(step, SessionStep::default());
        assert!(s.replica().is_none() && s.localizer().is_none());
        assert!(!s.text_settled() && !s.live() && s.failure().is_none());
        assert_eq!(s.data_progress(), (0, 0));
    }

    #[test]
    fn the_localizer_comes_from_the_text_table_with_the_default_plural_rule_without_tables() {
        let mut t = render::asset_format::LocaleTable::new(TEXT_LANG);
        t.entries.push(("TXT_KEY_HELLO".into(), "Hello".into()));
        let bytes = t.encode();
        let l = make_localizer(&bytes, None, TEXT_LANG).expect("a localizer");
        assert_eq!(l.convert("TXT_KEY_HELLO", &[]), "Hello");
        assert!(make_localizer(b"not a table", None, TEXT_LANG).is_err());
    }
}
