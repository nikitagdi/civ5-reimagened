//! The mods of a running game (T17-T19): what the clients have built in ([`ClientMods`], [`admit`]), the server halves
//! behind the host limits ([`ModHost`]), and the routing of their answers to seats. The game actor owns one
//! [`ModHost`]; it is plain synchronous state and is tested without an actor.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use super::stream::TerrainSharePart;
use mod_api::{LogLevel, Seat, WorldFacts};
use mod_host_server::{
    CallEnv, ClientOutcome, DropReason, GameMods, HostRegistry, LogLine, ModOutput, ModStatus,
    ModStore,
};
use mod_manifest::{ModId, ModSet, write_lock};
use protocol::{
    BuiltMod, GameId, MAX_MOD_ID_CHARS, MAX_MOD_PROBLEM_CHARS, MAX_MOD_PROBLEMS,
    MAX_MOD_VERSION_CHARS, MAX_MODS, ModC2S, ModProblem, ModS2C, ModSetView, ModState, ModsSection,
    PlayerId,
};

// ------------------------------------------------------------------ what the clients have (T17)

/// What a client says it has built in (`ClientMsg::ModHello`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientMods {
    pub api: (u16, u16, u16),
    pub built_in: Vec<BuiltMod>,
}

impl ClientMods {
    /// The bounds of the protocol: a hello over them is the sign of a broken or hostile client.
    pub fn within_limits(&self) -> bool {
        self.built_in.len() <= MAX_MODS
            && self.built_in.iter().all(|b| {
                b.id.chars().count() <= MAX_MOD_ID_CHARS
                    && b.version.chars().count() <= MAX_MOD_VERSION_CHARS
            })
    }
}

/// The `ModHello` of every logged-in session, by session id. The session writes it (during the handshake, or later),
/// the game actor reads it when the session attaches or joins.
#[derive(Debug, Default)]
pub struct ModHellos {
    map: Mutex<HashMap<u64, Arc<ClientMods>>>,
}

impl ModHellos {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Arc<ClientMods>>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn set(&self, session: u64, mods: ClientMods) {
        self.lock().insert(session, Arc::new(mods));
    }

    /// `None`: the session sent no `ModHello` (it has no mods built in).
    pub fn get(&self, session: u64) -> Option<Arc<ClientMods>> {
        self.lock().get(&session).cloned()
    }

    pub fn remove(&self, session: u64) {
        self.lock().remove(&session);
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A client that may play: the `Mods` message for its stream, and the mods it lacks (all optional).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    pub view: ModSetView,
    /// `load_idx` of the mods marked unavailable for this seat: nothing is sent to it for them.
    pub unavailable: BTreeSet<u16>,
}

/// Compares what a client has with the frozen set of the game (D5). A mod the game requires of its clients (gameplay,
/// or `client = "required"`) that the client lacks, or has in another version, refuses it with a sentence naming
/// the mod and its version. A missing optional mod lets the client in and is marked unavailable for it.
///
/// # Errors
/// The list of problems for `RejectReason::ModMismatch` (at most [`MAX_MOD_PROBLEMS`]).
pub fn admit(plan: &ModSetView, hello: Option<&ClientMods>) -> Result<Admitted, Vec<ModProblem>> {
    let mut view = plan.clone();
    let mut unavailable = BTreeSet::new();
    let mut problems = Vec::new();
    for m in &mut view.mods {
        let built = hello.and_then(|h| h.built_in.iter().find(|b| b.id == m.id));
        if built.is_some_and(|b| b.version == m.version) {
            m.available = true;
            continue;
        }
        m.available = false;
        if !m.client_required {
            unavailable.insert(m.load_idx);
            continue;
        }
        let text = match built {
            Some(b) => format!(
                "mod `{}` {} is required to play this game, your client has version {}",
                m.id, m.version, b.version
            ),
            None => format!(
                "mod `{}` {} is required to play this game, your client does not have it",
                m.id, m.version
            ),
        };
        problems.push(ModProblem {
            text: text.chars().take(MAX_MOD_PROBLEM_CHARS).collect(),
        });
    }
    if problems.is_empty() {
        Ok(Admitted { view, unavailable })
    } else {
        problems.truncate(MAX_MOD_PROBLEMS);
        Err(problems)
    }
}

// ------------------------------------------------------------------ the server halves of a game (T18, T19)

/// What one call of the host produced: the messages by seat (in the order the mods returned them), the raw outputs
/// (for the recording) and what the game must do about the sender.
#[derive(Debug, Default)]
pub struct Delivery {
    pub by_seat: BTreeMap<PlayerId, Vec<ModS2C>>,
    pub outputs: Vec<ModOutput>,
    /// For each of `outputs`, the seats it went to (a seat without the mod is not among them).
    pub targets: Vec<Vec<PlayerId>>,
    pub dropped: Option<DropReason>,
    /// The seat has repeated violations: disconnect it.
    pub disconnect: bool,
    /// The terrain shares of the mods (API 1.2) by seat, in the order made; the seat stream adds the basis records.
    pub terrain: BTreeMap<PlayerId, Vec<TerrainSharePart>>,
}

/// The mods of one running game: the frozen set, the server instances, who lacks which optional mod, and the block
/// the worker holds for the saves.
pub struct ModHost {
    game: GameId,
    store: Arc<ModStore>,
    registry: Arc<HostRegistry>,
    set: ModSet,
    /// `mods.lock` of the set, written once.
    lock: String,
    plan: ModSetView,
    mods: GameMods,
    unavailable: BTreeMap<PlayerId, BTreeSet<u16>>,
    /// The block the worker was last sent (or loaded from the save).
    sent: Option<ModsSection>,
}

impl ModHost {
    /// A game without mods: every method does nothing.
    pub fn empty(game: GameId, store: Arc<ModStore>, registry: Arc<HostRegistry>) -> Self {
        Self {
            game,
            store,
            registry,
            set: ModSet::default(),
            lock: String::new(),
            plan: ModSetView {
                modset_hash: 0,
                mods: Vec::new(),
            },
            mods: GameMods::default(),
            unavailable: BTreeMap::new(),
            sent: None,
        }
    }

    /// The mods of a game from its stored `mods.lock` (`None`: a game without mods). Every mod must be installed
    /// as frozen and have its code in this binary.
    ///
    /// # Errors
    /// A sentence for the host or the player (S4).
    pub fn build(
        game: GameId,
        store: Arc<ModStore>,
        registry: Arc<HostRegistry>,
        lock: Option<&str>,
    ) -> Result<Self, String> {
        let Some(lock) = lock else {
            return Ok(Self::empty(game, store, registry));
        };
        let set = store
            .restore_set(Some(lock), 0)
            .map_err(|e| e.to_string())?;
        if set.is_empty() {
            return Ok(Self::empty(game, store, registry));
        }
        let mods = registry
            .instantiate(&store, &set)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            game,
            plan: store.set_view(&set),
            mods,
            lock: write_lock(&set),
            set,
            store,
            registry,
            unavailable: BTreeMap::new(),
            sent: None,
        })
    }

    /// The game has a frozen mod set (clients are told, saves carry the block). Data-only mods count.
    pub fn has_set(&self) -> bool {
        !self.set.is_empty()
    }

    /// Some mod has a server half.
    pub fn has_code(&self) -> bool {
        !self.mods.is_empty()
    }

    /// Some mod reads the world (`world.read`): the actor keeps the whole-truth table for it (T22).
    pub fn wants_world(&self) -> bool {
        self.mods.wants_world()
    }

    /// The server's plan (every mod available); `None` for a game without mods.
    pub fn plan(&self) -> Option<&ModSetView> {
        self.has_set().then_some(&self.plan)
    }

    pub fn admit(&self, hello: Option<&ClientMods>) -> Result<Admitted, Vec<ModProblem>> {
        admit(&self.plan, hello)
    }

    /// Remembers the optional mods a seat lacks (its attach was admitted).
    pub fn seat_admitted(&mut self, seat: PlayerId, unavailable: BTreeSet<u16>) {
        if unavailable.is_empty() {
            self.unavailable.remove(&seat);
        } else {
            self.unavailable.insert(seat, unavailable);
        }
    }

    /// The seat's buckets and strikes start over (the list of mods it lacks is set by `seat_admitted` at each attach).
    pub fn forget_seat(&mut self, seat: PlayerId) {
        self.mods.forget_seat(Seat(seat));
    }

    /// (Re)starts the server halves: fresh instances, each given its state from the loaded save (`saved`, the `MODS`
    /// section of the very save the worker loaded; `None` for a new game). A block whose set is not this game's is
    /// ignored with a warning. The block the worker has is the saved one until [`persist`](Self::persist) says
    /// otherwise.
    pub fn start(&mut self, facts: &dyn WorldFacts, now_ms: u64, saved: Option<ModsSection>) {
        if !self.has_set() {
            return;
        }
        match self.registry.instantiate(&self.store, &self.set) {
            Ok(g) => self.mods = g,
            Err(e) => {
                tracing::error!(game = self.game, "the mods cannot be restarted: {e}");
                return;
            }
        }
        self.unavailable.clear();
        let mut states: Vec<(ModId, Vec<u8>)> = Vec::new();
        let mut kept = None;
        if let Some(sec) = saved {
            if sec.lock == self.lock {
                for st in &sec.states {
                    match ModId::new(&st.id) {
                        Ok(id) => states.push((id, st.state.clone())),
                        Err(_) => tracing::warn!(
                            game = self.game,
                            "the save holds a state for a bad mod id, ignored"
                        ),
                    }
                }
                kept = Some(sec);
            } else {
                tracing::warn!(
                    game = self.game,
                    "the mod set of the save is not the game's, the states in it are ignored"
                );
            }
        }
        self.sent = kept;
        self.mods.start(CallEnv { facts, now_ms }, &states);
        self.log_out();
    }

    /// A message of a client to the server half of a mod: host limits, the mod, then the audiences expanded.
    pub fn on_client(
        &mut self,
        facts: &dyn WorldFacts,
        now_ms: u64,
        from: PlayerId,
        msg: &ModC2S,
        humans: &[PlayerId],
    ) -> Delivery {
        let ClientOutcome {
            outputs,
            dropped,
            disconnect,
        } = self.mods.on_client(
            CallEnv { facts, now_ms },
            Seat(from),
            msg.mod_idx,
            msg.ch,
            &msg.payload,
        );
        self.log_out();
        let (by_seat, targets) = self.route(&outputs, Some(from), humans);
        Delivery {
            by_seat,
            outputs,
            targets,
            dropped,
            disconnect,
            terrain: self.route_shares(humans),
        }
    }

    /// A seat attached with a `Fresh` stream: the snapshot of every mod for it (D8, the pattern of the chat).
    pub fn on_attach(
        &mut self,
        facts: &dyn WorldFacts,
        now_ms: u64,
        seat: PlayerId,
        humans: &[PlayerId],
    ) -> Delivery {
        let outputs = self.mods.on_attach(CallEnv { facts, now_ms }, Seat(seat));
        self.log_out();
        let (by_seat, targets) = self.route(&outputs, Some(seat), humans);
        Delivery {
            by_seat,
            outputs,
            targets,
            terrain: self.route_shares(humans),
            ..Delivery::default()
        }
    }

    /// A new turn began. `on_turn` itself answers nothing; what comes back are the answers to the hits it dealt.
    pub fn on_turn(
        &mut self,
        facts: &dyn WorldFacts,
        now_ms: u64,
        turn: i32,
        humans: &[PlayerId],
    ) -> Delivery {
        let outputs = self.mods.on_turn(CallEnv { facts, now_ms }, turn);
        self.delivery(outputs, humans)
    }

    /// The client of a seat went away (lost, left, kicked): every mod hears it (API 1.1). `humans` should no longer
    /// hold the seat.
    pub fn on_detach(
        &mut self,
        facts: &dyn WorldFacts,
        now_ms: u64,
        seat: PlayerId,
        humans: &[PlayerId],
    ) -> Delivery {
        let outputs = self.mods.on_detach(CallEnv { facts, now_ms }, Seat(seat));
        self.delivery(outputs, humans)
    }

    /// When the actor should call [`on_tick`](Self::on_tick) next (host clock); `None`: no mod ticks.
    pub fn next_tick_ms(&self) -> Option<u64> {
        self.mods.next_tick_ms()
    }

    /// The tick of the mods that are due at `now_ms` (`[rust] tick_hz`, API 1.1).
    pub fn on_tick(
        &mut self,
        facts: &dyn WorldFacts,
        now_ms: u64,
        humans: &[PlayerId],
    ) -> Delivery {
        let outputs = self.mods.on_tick(CallEnv { facts, now_ms });
        self.delivery(outputs, humans)
    }

    /// Outputs with no sender, logs out, audiences expanded.
    fn delivery(&mut self, outputs: Vec<ModOutput>, humans: &[PlayerId]) -> Delivery {
        self.log_out();
        let (by_seat, targets) = self.route(&outputs, None, humans);
        Delivery {
            by_seat,
            outputs,
            targets,
            terrain: self.route_shares(humans),
            ..Delivery::default()
        }
    }

    /// The block the saves should carry now, if it differs from the one the worker has. `None`: nothing to send (a
    /// game without a mod set, or no change).
    pub fn persist(&mut self, facts: &dyn WorldFacts, now_ms: u64) -> Option<ModsSection> {
        if !self.has_set() {
            return None;
        }
        let states = self.mods.persist(CallEnv { facts, now_ms });
        self.log_out();
        let sec = ModsSection {
            lock: self.lock.clone(),
            states: states
                .into_iter()
                .map(|(id, state)| ModState {
                    id: id.as_str().to_owned(),
                    state,
                })
                .collect(),
        };
        (self.sent.as_ref() != Some(&sec)).then_some(sec)
    }

    /// The worker was sent `sec`.
    pub fn sent(&mut self, sec: ModsSection) {
        self.sent = Some(sec);
    }

    /// Status of every mod with a server half.
    pub fn statuses(&self) -> Vec<(ModId, ModStatus)> {
        self.mods.statuses()
    }

    /// Expands the audiences of `outputs` into seats. `sender`: the seat of `Audience::Sender`. A seat that lacks
    /// the mod gets nothing of it. Per seat the order is the order of the outputs.
    fn route(
        &self,
        outputs: &[ModOutput],
        sender: Option<PlayerId>,
        humans: &[PlayerId],
    ) -> (BTreeMap<PlayerId, Vec<ModS2C>>, Vec<Vec<PlayerId>>) {
        use mod_api::Audience;
        let mut by_seat: BTreeMap<PlayerId, Vec<ModS2C>> = BTreeMap::new();
        let mut targets = Vec::with_capacity(outputs.len());
        for o in outputs {
            let seats: Vec<PlayerId> = match &o.to {
                Audience::Sender => sender.into_iter().filter(|s| humans.contains(s)).collect(),
                Audience::All => humans.to_vec(),
                Audience::AllExcept(x) => humans
                    .iter()
                    .copied()
                    .filter(|s| *s != x.player())
                    .collect(),
                Audience::Seats(v) => v
                    .iter()
                    .map(|s| s.player())
                    .filter(|s| humans.contains(s))
                    .collect(),
            };
            let wire = o.to_wire();
            let mut got = Vec::new();
            for s in seats {
                if self
                    .unavailable
                    .get(&s)
                    .is_some_and(|u| u.contains(&o.mod_idx))
                {
                    continue;
                }
                by_seat.entry(s).or_default().push(wire.clone());
                got.push(s);
            }
            targets.push(got);
        }
        (by_seat, targets)
    }

    /// The terrain shares the mods made in the call (API 1.2), by seat. Like the messages, a seat that is not a human
    /// with a pipeline, or that lacks the mod, gets nothing of it.
    fn route_shares(&mut self, humans: &[PlayerId]) -> BTreeMap<PlayerId, Vec<TerrainSharePart>> {
        let mut by_seat: BTreeMap<PlayerId, Vec<TerrainSharePart>> = BTreeMap::new();
        for (mod_idx, s) in self.mods.take_terrain_shares() {
            let seat = s.seat.player();
            if !humans.contains(&seat)
                || self
                    .unavailable
                    .get(&seat)
                    .is_some_and(|u| u.contains(&mod_idx))
            {
                continue;
            }
            by_seat.entry(seat).or_default().push(TerrainSharePart {
                mod_idx,
                reset: s.reset,
                plots: s.plots,
            });
        }
        by_seat
    }

    /// The log lines of the mods (and of the host for them) into the server log, tagged with the mod.
    fn log_out(&mut self) {
        for LogLine { mod_id, level, msg } in self.mods.take_logs() {
            let (game, id) = (self.game, mod_id.as_str());
            match level {
                LogLevel::Debug => tracing::debug!(game, mod_id = id, "{msg}"),
                LogLevel::Warn => tracing::warn!(game, mod_id = id, "{msg}"),
                LogLevel::Error => tracing::error!(game, mod_id = id, "{msg}"),
                _ => tracing::info!(game, mod_id = id, "{msg}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mod_api::testing::MockFacts;
    use mod_api::{Outgoing, ServerCtx, ServerMod};
    use mod_manifest::{ContentHash, Installed, Manifest};
    use protocol::ModPick;

    use super::*;

    /// A mod with one channel (64 bytes in, 256 out, 50/s) and a server part.
    fn manifest_text(id: &str, sync: &str, client: &str) -> String {
        format!(
            r#"
[mod]
id = "{id}"
name = "{id}"
version = "1.2.0"
api = "^1"
license = "MIT"
sync = "{sync}"
{client}
parts = ["server-rs"]

[rust]
server = {{ crate = "{id}-server", entry = "{id}_server::register" }}
caps = ["net.channel"]
state_max = 64

[[channels]]
id = 0
name = "main"
c2s_max = 64
s2c_max = 256
rate = "50/s"
burst = 20
"#
        )
    }

    fn manifest(id: &str, sync: &str, client: &str) -> Installed {
        let src = manifest_text(id, sync, client);
        Installed {
            manifest: Manifest::parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}")),
            content_hash: ContentHash([3; 32]),
        }
    }

    /// Echoes to the sender and the others; the state is the count; the snapshot of `on_attach` is the count.
    #[derive(Default)]
    struct Echo {
        n: u32,
        dirty: bool,
    }

    impl ServerMod for Echo {
        fn start(&mut self, _cx: &mut ServerCtx<'_>, saved: Option<&[u8]>) {
            self.n = saved
                .and_then(|b| b.try_into().ok())
                .map_or(0, u32::from_le_bytes);
        }
        fn on_client(
            &mut self,
            _cx: &mut ServerCtx<'_>,
            from: Seat,
            ch: u8,
            p: &[u8],
        ) -> Vec<Outgoing> {
            if p == b"boom" {
                panic!("scripted failure");
            }
            self.n += 1;
            self.dirty = true;
            vec![
                Outgoing::to_sender(ch, p.to_vec()),
                Outgoing::all_except(from, ch, p.to_vec()),
            ]
        }
        fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
            vec![Outgoing::to_sender(0, self.n.to_le_bytes().to_vec())]
        }
        fn persist(&self) -> Vec<u8> {
            self.n.to_le_bytes().to_vec()
        }
        fn is_dirty(&self) -> bool {
            self.dirty
        }
        fn mark_saved(&mut self) {
            self.dirty = false;
        }
    }

    fn share_manifest(id: &str) -> Installed {
        let src = manifest_text(id, "cosmetic", "client = \"optional\"").replace(
            "caps = [\"net.channel\"]",
            "caps = [\"net.channel\", \"world.share_terrain\"]",
        );
        Installed {
            manifest: Manifest::parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}")),
            content_hash: ContentHash([3; 32]),
        }
    }

    /// API 1.2: `mine` shares plots 1, 2 to the sender, `seat2` shares plot 3 to seat 2, `end` unshares the sender.
    struct Arena;

    impl ServerMod for Arena {
        fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
        fn on_client(
            &mut self,
            cx: &mut ServerCtx<'_>,
            from: Seat,
            _ch: u8,
            p: &[u8],
        ) -> Vec<Outgoing> {
            let r = match p {
                b"mine" => cx.share_terrain(from, &[1, 2, 2]),
                b"seat2" => cx.share_terrain(Seat(2), &[3]),
                b"end" => cx.unshare_terrain(from),
                _ => Ok(()),
            };
            assert!(r.is_ok());
            Vec::new()
        }
        fn on_attach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
            cx.share_terrain(seat, &[7]).unwrap();
            Vec::new()
        }
        fn persist(&self) -> Vec<u8> {
            Vec::new()
        }
    }

    fn registry() -> Arc<HostRegistry> {
        let mut r = HostRegistry::new();
        r.register(|r| {
            r.add("hello", || Box::new(Echo::default()));
            r.add("flair", || Box::new(Echo::default()));
            r.add("arena", || Box::new(Arena));
        });
        Arc::new(r)
    }

    fn store() -> Arc<ModStore> {
        Arc::new(ModStore::from_installed(vec![
            // `hello`: gameplay, so every client must have it.
            manifest("hello", "gameplay", ""),
            // `flair`: cosmetic and optional for clients.
            manifest("flair", "cosmetic", "client = \"optional\""),
            // `arena`: cosmetic, optional, shares terrain (API 1.2).
            share_manifest("arena"),
        ]))
    }

    fn lock_of(store: &ModStore, picks: &[&str]) -> String {
        let picks: Vec<ModPick> = picks
            .iter()
            .map(|p| ModPick {
                id: (*p).into(),
                req: "*".into(),
            })
            .collect();
        store.freeze(&picks).unwrap().lock_text()
    }

    fn host(picks: &[&str]) -> ModHost {
        let store = store();
        let lock = lock_of(&store, picks);
        ModHost::build(1, store, registry(), Some(&lock)).unwrap()
    }

    fn built(id: &str, version: &str) -> BuiltMod {
        BuiltMod {
            id: id.into(),
            version: version.into(),
            code_hash: [0; 16],
        }
    }

    fn hello(mods: &[(&str, &str)]) -> ClientMods {
        ClientMods {
            api: (1, 0, 0),
            built_in: mods.iter().map(|(i, v)| built(i, v)).collect(),
        }
    }

    fn c2s(mod_idx: u16, payload: &[u8]) -> ModC2S {
        ModC2S {
            mod_idx,
            ch: 0,
            payload: payload.to_vec(),
        }
    }

    fn idx(h: &ModHost, id: &str) -> u16 {
        h.plan()
            .unwrap()
            .mods
            .iter()
            .find(|m| m.id == id)
            .unwrap()
            .load_idx
    }

    #[test]
    fn the_plan_carries_the_pack_hash_of_a_mod_with_a_pack() {
        let dir = std::env::temp_dir().join(format!("c5-modhost-pack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hello")).unwrap();
        std::fs::write(
            dir.join("hello/mod.toml"),
            manifest_text("hello", "gameplay", ""),
        )
        .unwrap();
        let bytes = mod_assets::Pack::build(
            mod_manifest::ModId::new("hello").unwrap(),
            vec![],
            vec![mod_assets::NewFile {
                id: "data/blob".into(),
                kind: mod_assets::AssetKind::Raw,
                bytes: b"abc".to_vec(),
            }],
        )
        .unwrap()
        .to_bytes();
        std::fs::write(dir.join("hello/hello.pack"), &bytes).unwrap();
        let store = Arc::new(ModStore::scan(&dir));
        assert!(store.problems().is_empty(), "{:?}", store.problems());
        let lock = lock_of(&store, &["hello"]);
        let h = ModHost::build(1, store, registry(), Some(&lock)).unwrap();
        let want = mod_assets::Sha256::of(&bytes).to_string();
        let plan = h.plan().unwrap();
        assert_eq!(plan.mods[0].pack.as_deref(), Some(want.as_str()));
        // What `admit` hands a client (the `GameMsg::Mods` of its stream) carries it too.
        let a = h.admit(Some(&hello(&[("hello", "1.2.0")]))).unwrap();
        assert_eq!(a.view.mods[0].pack.as_deref(), Some(want.as_str()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_client_with_every_mod_is_admitted_and_sees_them_all_available() {
        let h = host(&["hello", "flair"]);
        let a = h
            .admit(Some(&hello(&[("hello", "1.2.0"), ("flair", "1.2.0")])))
            .unwrap();
        assert!(a.unavailable.is_empty());
        assert!(a.view.mods.iter().all(|m| m.available));
        assert_eq!(a.view.modset_hash, h.plan().unwrap().modset_hash);
    }

    #[test]
    fn a_missing_required_mod_is_a_mismatch_that_names_the_mod_and_its_version() {
        let h = host(&["hello", "flair"]);
        for none in [None, Some(hello(&[("flair", "1.2.0")]))] {
            let problems = h.admit(none.as_ref()).unwrap_err();
            assert_eq!(problems.len(), 1, "{problems:?}");
            let t = &problems[0].text;
            assert!(t.contains("`hello`") && t.contains("1.2.0"), "{t}");
            assert!(t.contains("does not have it"), "{t}");
        }
        // The right mod in another version is as good as none, and the text says which version the client has.
        let problems = h.admit(Some(&hello(&[("hello", "1.1.0")]))).unwrap_err();
        assert!(problems[0].text.contains("version 1.1.0"), "{problems:?}");
    }

    #[test]
    fn a_missing_optional_mod_lets_the_client_in_marked_unavailable() {
        let h = host(&["hello", "flair"]);
        let a = h.admit(Some(&hello(&[("hello", "1.2.0")]))).unwrap();
        let flair = a.view.mods.iter().find(|m| m.id == "flair").unwrap();
        assert!(!flair.available);
        assert_eq!(a.unavailable, BTreeSet::from([flair.load_idx]));
        assert!(
            a.view
                .mods
                .iter()
                .find(|m| m.id == "hello")
                .unwrap()
                .available
        );
    }

    #[test]
    fn a_game_without_mods_has_no_plan_and_does_nothing() {
        let store = Arc::new(ModStore::empty());
        let mut h = ModHost::build(1, store, Arc::new(HostRegistry::new()), None).unwrap();
        let facts = MockFacts::new(4, 4);
        assert!(h.plan().is_none() && !h.has_set() && !h.has_code());
        h.start(&facts, 0, None);
        assert!(h.persist(&facts, 0).is_none());
        let d = h.on_client(&facts, 0, 0, &c2s(0, b"x"), &[0, 1]);
        assert!(d.by_seat.is_empty() && !d.disconnect);
    }

    #[test]
    fn a_set_the_binary_has_no_code_for_is_refused_with_a_sentence() {
        let store = store();
        let lock = lock_of(&store, &["hello"]);
        let err = ModHost::build(1, store, Arc::new(HostRegistry::new()), Some(&lock))
            .err()
            .unwrap();
        assert!(err.contains("`hello`") && err.contains("code"), "{err}");
    }

    #[test]
    fn answers_are_expanded_into_seats_in_order_and_skip_seats_without_the_mod() {
        let mut h = host(&["hello", "flair"]);
        let facts = MockFacts::new(4, 4);
        h.start(&facts, 0, None);
        let humans = [0, 1, 2];
        let (flair_i, hello_i) = (idx(&h, "flair"), idx(&h, "hello"));
        // Seat 2 lacks `flair`.
        h.seat_admitted(2, BTreeSet::from([flair_i]));
        let d = h.on_client(&facts, 0, 0, &c2s(flair_i, b"hi"), &humans);
        assert_eq!(d.dropped, None);
        // The sender (0) and everyone but 0 (1, 2) get it, except the seat without the mod.
        assert_eq!(d.by_seat.keys().copied().collect::<Vec<_>>(), [0, 1]);
        assert_eq!(d.by_seat[&0].len(), 1);
        assert_eq!(d.by_seat[&0][0].from, Some(0));
        assert_eq!(d.by_seat[&0][0].payload, b"hi");
        // The same message to `hello` reaches seat 2.
        let d = h.on_client(&facts, 0, 0, &c2s(hello_i, b"yo"), &humans);
        assert_eq!(d.by_seat.keys().copied().collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn limits_stop_a_message_before_the_mod_and_strikes_ask_for_a_disconnect() {
        let mut h = host(&["hello"]);
        let facts = MockFacts::new(4, 4);
        h.start(&facts, 0, None);
        // Oversize (c2s_max is 64) never reaches the mod: no state change, no answers.
        let d = h.on_client(&facts, 0, 0, &c2s(0, &[0; 65]), &[0, 1]);
        assert!(d.dropped.is_some() && d.by_seat.is_empty());
        let first = h.persist(&facts, 0).expect("the first block");
        assert_eq!(first.states[0].state, 0u32.to_le_bytes());
        // A message to a mod that does not exist counts as well; the tenth strike in the window (the oversize one was
        // the first) disconnects, and the count starts over.
        let mut last = d;
        for _ in 1..mod_host_server::MAX_STRIKES {
            assert!(!last.disconnect);
            last = h.on_client(&facts, 0, 0, &c2s(9, b"x"), &[0, 1]);
        }
        assert!(last.disconnect);
        assert!(!h.on_client(&facts, 0, 0, &c2s(9, b"x"), &[0, 1]).disconnect);
    }

    #[test]
    fn a_panicking_mod_is_off_and_the_others_go_on() {
        let mut h = host(&["hello", "flair"]);
        let facts = MockFacts::new(4, 4);
        h.start(&facts, 0, None);
        let (hello_i, flair_i) = (idx(&h, "hello"), idx(&h, "flair"));
        let d = h.on_client(&facts, 0, 0, &c2s(hello_i, b"boom"), &[0, 1]);
        assert!(d.by_seat.is_empty() && !d.disconnect);
        let d = h.on_client(&facts, 0, 0, &c2s(flair_i, b"ok"), &[0, 1]);
        assert_eq!(d.by_seat.len(), 2, "the other mod still answers");
        let off: Vec<_> = h
            .statuses()
            .into_iter()
            .filter(|(_, s)| matches!(s, ModStatus::Disabled { .. }))
            .map(|(id, _)| id.as_str().to_owned())
            .collect();
        assert_eq!(off, ["hello"]);
    }

    #[test]
    fn the_block_for_the_saves_follows_the_mods_and_is_sent_only_when_it_changes() {
        let mut h = host(&["hello"]);
        let facts = MockFacts::new(4, 4);
        h.start(&facts, 0, None);
        // Right after the start the worker has nothing: the block (lock and the first states) goes out once.
        let first = h.persist(&facts, 0).expect("the first block");
        assert_eq!(first.states.len(), 1);
        assert!(first.lock.contains("hello"));
        h.sent(first);
        assert!(h.persist(&facts, 0).is_none(), "no change, nothing to send");
        h.on_client(&facts, 0, 0, &c2s(0, b"a"), &[0]);
        h.on_client(&facts, 0, 0, &c2s(0, b"b"), &[0]);
        let next = h.persist(&facts, 0).expect("the count changed");
        assert_eq!(next.states[0].state, 2u32.to_le_bytes());
    }

    #[test]
    fn a_restart_gives_the_mods_the_state_of_the_save_and_ignores_a_foreign_set() {
        let mut h = host(&["hello"]);
        let facts = MockFacts::new(4, 4);
        let saved = ModsSection {
            lock: h.lock.clone(),
            states: vec![ModState {
                id: "hello".into(),
                state: 41u32.to_le_bytes().to_vec(),
            }],
        };
        h.start(&facts, 0, Some(saved.clone()));
        // The state came back: the snapshot of the mod says 41, and the worker already has that block.
        let d = h.on_attach(&facts, 0, 1, &[0, 1]);
        assert_eq!(d.by_seat[&1][0].payload, 41u32.to_le_bytes());
        assert!(h.persist(&facts, 0).is_none());
        // A block of another set is not used.
        let foreign = ModsSection {
            lock: "something else".into(),
            ..saved
        };
        h.start(&facts, 0, Some(foreign));
        let d = h.on_attach(&facts, 0, 1, &[0, 1]);
        assert_eq!(d.by_seat[&1][0].payload, 0u32.to_le_bytes());
    }

    #[test]
    fn a_terrain_share_goes_to_its_seat_only_and_not_to_a_seat_without_the_mod() {
        let mut h = host(&["hello", "arena"]);
        let facts = MockFacts::new(4, 4);
        h.start(&facts, 0, None);
        let arena = idx(&h, "arena");
        let parts = |d: &Delivery| -> Vec<(PlayerId, u16, bool, Vec<u32>)> {
            d.terrain
                .iter()
                .flat_map(|(p, v)| v.iter().map(|s| (*p, s.mod_idx, s.reset, s.plots.clone())))
                .collect()
        };
        let d = h.on_client(&facts, 0, 0, &c2s(arena, b"mine"), &[0, 1, 2]);
        assert_eq!(parts(&d), [(0, arena, false, vec![1, 2])]);
        assert!(d.by_seat.is_empty(), "no mod message came of it");
        // Again: nothing new.
        let d = h.on_client(&facts, 0, 0, &c2s(arena, b"mine"), &[0, 1, 2]);
        assert!(d.terrain.is_empty());
        // A share to a seat that lacks the mod, or that is not a human with a pipeline, goes nowhere.
        h.seat_admitted(2, BTreeSet::from([arena]));
        let d = h.on_client(&facts, 0, 0, &c2s(arena, b"seat2"), &[0, 1, 2]);
        assert!(d.terrain.is_empty());
        h.seat_admitted(2, BTreeSet::new());
        let d = h.on_client(&facts, 0, 0, &c2s(arena, b"seat2"), &[0, 1]);
        assert!(d.terrain.is_empty(), "seat 2 is not playing");
        // The attach of a seat shares to it; the unshare is a reset.
        let d = h.on_attach(&facts, 0, 1, &[0, 1]);
        assert_eq!(parts(&d), [(1, arena, false, vec![7])]);
        let d = h.on_client(&facts, 0, 0, &c2s(arena, b"end"), &[0, 1]);
        assert_eq!(parts(&d), [(0, arena, true, vec![])]);
    }

    #[test]
    fn the_hellos_of_sessions_come_and_go() {
        let hs = ModHellos::default();
        assert!(hs.get(7).is_none() && hs.is_empty());
        hs.set(7, hello(&[("hello", "1.0.0")]));
        assert_eq!(hs.get(7).unwrap().built_in.len(), 1);
        hs.remove(7);
        assert!(hs.is_empty());
        let mut big = hello(&[]);
        big.built_in = (0..=MAX_MODS)
            .map(|i| built(&format!("m{i}"), "1.0.0"))
            .collect();
        assert!(!big.within_limits());
        assert!(hello(&[("x", "1.0.0")]).within_limits());
    }
}
