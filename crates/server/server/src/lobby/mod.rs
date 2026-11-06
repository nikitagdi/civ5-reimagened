//! The lobby actor: rooms, who is where, the lists of games and saves. One task
//! owns it all; sessions and game actors talk to it through [`LobbyMsg`]. Commands: `CreateRoom`,
//! `JoinRoom`, `LeaveRoom`, `Settings`, `Slot`, `PickCiv`, `TakeSlot`, `Ready`, `Start`, `LoadSave`
//! (a room from a save, [`saves`]) and `JoinGame` (the own seat again, or an AI slot of a running
//! game; the first request for a slot wins). A member of a room whose session is lost for
//! `host_lost_s` is removed as if he had left (12 item 13).

pub mod rooms;
pub mod saves;
pub mod validate;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use protocol::{
    BasisClaim, GameId, GameStatus, LobbyCmd, LobbyState, Occupant, OnlinePlayer, PlayerId,
    PlayerPlace, Resume, RoomId, SeatControl, SeatSummary, ServerMsg,
};
use tokio::sync::mpsc;
use tokio::time::Instant;
use wire::Encoder;

use self::rooms::{MAX_HUMANS, Room, SlotEffect};
use self::saves::{SaveEntry, SaveIndex, parse_save_id};
use crate::Shared;
use crate::game::{ActorMsg, AttachRequest, GameInit, GameLoad, JoinRequest, SessionLink};
use crate::session::outbox::Outbox;
use crate::state::meta::MetaStatus;
use crate::supervisor::GameState;

/// `LobbyState` goes out at most this often.
pub const BROADCAST_EVERY: Duration = Duration::from_millis(100);
/// Rooms at once.
pub const MAX_ROOMS: usize = 8;
/// How often the lost room members are looked at.
const SWEEP_EVERY: Duration = Duration::from_millis(250);
/// How often the sleeping games are looked at for the close rule.
const DORMANT_EVERY: Duration = Duration::from_secs(1);

pub type LobbyTx = mpsc::UnboundedSender<LobbyMsg>;

/// A session that logged in.
pub struct LoginInfo {
    pub nick: String,
    pub session: u64,
    pub out: Outbox,
    pub link: SessionLink,
    pub resume: Option<Resume>,
}

/// What reaches the lobby actor.
// `LobbyCmd` carries `GameSettings`; the messages are built and matched once each, a Box is not worth it.
#[allow(clippy::large_enum_variant)]
pub enum LobbyMsg {
    Login(LoginInfo),
    /// A session ended.
    Gone {
        nick: String,
        session: u64,
    },
    Cmd {
        nick: String,
        session: u64,
        cmd: LobbyCmd,
    },
    /// A game is up (from its actor): the room becomes a game, the humans get their seats.
    GameStarted {
        room: Option<RoomId>,
        game: GameId,
        seats: Vec<(PlayerId, String)>,
    },
    /// A game did not start: the room is back in gathering.
    GameFailed {
        room: Option<RoomId>,
        why: String,
    },
    /// A game is over: its humans are free.
    GameEnded {
        game: GameId,
        nicks: Vec<String>,
    },
    /// Something in the list of games changed.
    GamesChanged,
    /// A sleeping game is closed at once (admin `POST /admin/games/{id}/close`): `Finished` on disk, the seats and nicks
    /// are free, the saves stay.
    CloseSleeping {
        game: GameId,
    },
    /// A game wrote a save: it goes into the list of saves.
    SaveWritten(Box<SaveEntry>),
    /// The saves of a game were deleted from the disk (admin `purge`): they leave the list.
    PurgeGame {
        game: GameId,
    },
    /// A `JoinGame` into an AI slot went through: the nick sits there now.
    JoinDone {
        game: GameId,
        player: PlayerId,
        nick: String,
    },
    /// The gateway drains: the rooms are dissolved.
    Drain,
    /// A game came back from the disk: its players keep their seats (the nicks are theirs) whether the game
    /// is running again or asleep.
    Seated {
        game: GameId,
        seats: Vec<(PlayerId, String)>,
    },
    /// The player left the game stream (`GameCtl::Leave`) and keeps the seat: the lobby view
    /// follows.
    Away {
        game: GameId,
        nick: String,
    },
    /// The host or an administrator kicked the player: the seat is the AI's, the nick is free.
    Kicked {
        game: GameId,
        nick: String,
    },
    /// A `LeaveSeat` the game could not carry out: the seat stays.
    GiveUpFailed {
        nick: String,
        why: String,
    },
    /// A `JoinGame` into an AI slot did not happen.
    JoinFailed {
        game: GameId,
        player: PlayerId,
        nick: String,
        why: String,
    },
}

/// The lobby's mailbox, in `Shared`. The actor starts on first use ([`Lobby::start`]) because
/// `Shared` is built outside a runtime.
pub struct Lobby {
    tx: LobbyTx,
    rx: Mutex<Option<mpsc::UnboundedReceiver<LobbyMsg>>>,
}

impl Default for Lobby {
    fn default() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }
}

impl Lobby {
    pub fn tx(&self) -> LobbyTx {
        self.tx.clone()
    }

    pub fn send(&self, msg: LobbyMsg) {
        let _ = self.tx.send(msg);
    }

    /// Starts the actor once; later calls do nothing. Must run inside a tokio runtime.
    pub fn start(&self, shared: &Arc<Shared>) {
        let rx = self
            .rx
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(rx) = rx {
            let actor = LobbyActor {
                shared: Arc::downgrade(shared),
                rx,
                rooms: BTreeMap::new(),
                places: HashMap::new(),
                sessions: HashMap::new(),
                dirty: false,
                last_broadcast: None,
                saves: SaveIndex::new(),
                claims: HashMap::new(),
                joining: HashMap::new(),
                room_lost: HashMap::new(),
                sweep_at: None,
                away: HashSet::new(),
                seat_before: HashMap::new(),
                dormant_seen: HashMap::new(),
            };
            tokio::spawn(actor.run());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Room(RoomId),
    Seat { game: GameId, player: PlayerId },
}

struct Sess {
    id: u64,
    out: Outbox,
    link: SessionLink,
    choices_sent: bool,
    nick: String,
}

struct LobbyActor {
    shared: Weak<Shared>,
    rx: mpsc::UnboundedReceiver<LobbyMsg>,
    rooms: BTreeMap<RoomId, Room>,
    /// By lower-case nick.
    places: HashMap<String, Place>,
    sessions: HashMap<String, Sess>,
    dirty: bool,
    last_broadcast: Option<Instant>,
    saves: SaveIndex,
    /// AI slots a `JoinGame` is taking right now, and the nick (first request wins).
    claims: HashMap<(GameId, PlayerId), String>,
    /// The other way round: the slot a nick is joining (by lower-case nick).
    joining: HashMap<String, (GameId, PlayerId)>,
    /// Room members whose session is gone, and since when (`Clock` milliseconds), by lower-case nick.
    room_lost: HashMap<String, u64>,
    sweep_at: Option<Instant>,
    /// Nicks that left their game's stream and keep the seat: they see the lobby (by lower-case nick).
    away: HashSet<String>,
    /// The seat in a sleeping game a nick had when it joined a room made from that game's save (spec 4.8
    /// `LoadSave`: the humans of the save take their reserved slots): out of the room, the seat is its place
    /// again (by lower-case nick).
    seat_before: HashMap<String, (GameId, PlayerId)>,
    /// Since when (`Clock` ms) a sleeping game has had no human connected; it is closed after `empty_close_min`.
    dormant_seen: HashMap<GameId, u64>,
}

fn key(nick: &str) -> String {
    nick.to_lowercase()
}

fn push(out: &Outbox, msg: &ServerMsg) {
    match Encoder::default().frame_msg(msg) {
        // A full queue closes the session by itself.
        Ok(frame) => {
            let _ = out.push(frame);
        }
        Err(e) => tracing::error!(error = %e, "cannot encode a lobby message"),
    }
}

impl LobbyActor {
    async fn run(mut self) {
        let mut dormant_tick = tokio::time::interval(DORMANT_EVERY);
        dormant_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let wait = self.dirty.then(|| {
                self.last_broadcast
                    .map_or_else(Instant::now, |t| t + BROADCAST_EVERY)
            });
            if self.sweep_at.is_none() && !self.room_lost.is_empty() {
                self.sweep_at = Some(Instant::now() + SWEEP_EVERY);
            }
            let sweep = self.sweep_at;
            tokio::select! {
                msg = self.rx.recv() => match msg {
                    Some(m) => {
                        let Some(shared) = self.shared.upgrade() else { return };
                        self.handle(&shared, m);
                    }
                    None => return,
                },
                () = async {
                    match wait {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(shared) = self.shared.upgrade() {
                        self.broadcast(&shared);
                    }
                }
                _ = dormant_tick.tick() => {
                    if let Some(shared) = self.shared.upgrade() {
                        self.sweep_dormant(&shared);
                    }
                }
                () = async {
                    match sweep {
                        Some(t) => tokio::time::sleep_until(t).await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.sweep_at = None;
                    if let Some(shared) = self.shared.upgrade() {
                        self.sweep_room_members(&shared);
                    }
                }
            }
            if self.dirty
                && self
                    .last_broadcast
                    .is_none_or(|t| t.elapsed() >= BROADCAST_EVERY)
                && let Some(shared) = self.shared.upgrade()
            {
                self.broadcast(&shared);
            }
        }
    }

    fn handle(&mut self, shared: &Arc<Shared>, msg: LobbyMsg) {
        match msg {
            LobbyMsg::Login(info) => self.login(shared, info),
            LobbyMsg::Gone { nick, session } => self.gone(shared, &nick, session),
            LobbyMsg::Cmd { nick, session, cmd } => {
                if self
                    .sessions
                    .get(&key(&nick))
                    .is_some_and(|s| s.id == session)
                {
                    self.command(shared, &nick, cmd);
                }
            }
            LobbyMsg::GameStarted { room, game, seats } => {
                self.game_started(shared, room, game, &seats);
            }
            LobbyMsg::GameFailed { room, why } => {
                if let Some(id) = room
                    && let Some(r) = self.rooms.get_mut(&id)
                {
                    r.status = GameStatus::Lobby;
                    for nick in r.humans() {
                        self.error(&nick, &format!("the game did not start: {why}"));
                    }
                }
                self.dirty = true;
            }
            LobbyMsg::GameEnded { game, nicks } => {
                self.claims.retain(|(g, _), _| *g != game);
                self.joining.retain(|_, (g, _)| *g != game);
                self.seat_before.retain(|_, (g, _)| *g != game);
                for nick in nicks {
                    let k = key(&nick);
                    if matches!(self.places.get(&k), Some(Place::Seat { game: g, .. }) if *g == game)
                    {
                        self.places.remove(&k);
                        self.away.remove(&k);
                        shared.identity.release(&nick);
                        // The game told the session `Detached`; the lobby view follows.
                        self.send_lobby(shared, &nick);
                    }
                }
                self.dirty = true;
            }
            LobbyMsg::GamesChanged => self.dirty = true,
            LobbyMsg::CloseSleeping { game } => self.close_sleeping(shared, game),
            LobbyMsg::SaveWritten(entry) => {
                let build = shared.health.core_build();
                if build.as_deref().is_some_and(|b| b != entry.build_id) {
                    tracing::info!(
                        save = %entry.id,
                        build = %entry.build_id,
                        "a save of another core build is not listed"
                    );
                }
                self.saves.insert(*entry);
                self.dirty = true;
            }
            LobbyMsg::PurgeGame { game } => {
                if self.saves.remove_game(game) > 0 {
                    self.dirty = true;
                }
            }
            LobbyMsg::Drain => {
                let rooms = std::mem::take(&mut self.rooms);
                for (id, room) in rooms {
                    for nick in room.humans() {
                        let k = key(&nick);
                        if matches!(self.places.get(&k), Some(Place::Room(r)) if *r == id) {
                            self.out_of_room(shared, &nick);
                            self.error(&nick, "the server is restarting: the room is closed");
                            self.send_lobby(shared, &nick);
                        }
                    }
                }
            }
            LobbyMsg::Seated { game, seats } => {
                for (player, nick) in seats {
                    self.places.insert(key(&nick), Place::Seat { game, player });
                    shared.identity.reserve(&nick);
                }
                self.dirty = true;
            }
            LobbyMsg::Away { game, nick } => {
                let k = key(&nick);
                if matches!(self.places.get(&k), Some(Place::Seat { game: g, .. }) if *g == game) {
                    self.away.insert(k);
                    self.send_lobby(shared, &nick);
                }
                self.dirty = true;
            }
            LobbyMsg::Kicked { game, nick } => {
                let k = key(&nick);
                if self.seat_before.get(&k).is_some_and(|(g, _)| *g == game) {
                    self.seat_before.remove(&k);
                }
                if matches!(self.places.get(&k), Some(Place::Seat { game: g, .. }) if *g == game) {
                    self.places.remove(&k);
                    self.away.remove(&k);
                    shared.identity.release(&nick);
                    self.send_lobby(shared, &nick);
                }
                self.dirty = true;
            }
            LobbyMsg::GiveUpFailed { nick, why } => {
                self.error(&nick, &format!("cannot give up the seat: {why}"));
            }
            LobbyMsg::JoinDone { game, player, nick } => {
                let k = key(&nick);
                self.claims.remove(&(game, player));
                self.joining.remove(&k);
                self.places.insert(k, Place::Seat { game, player });
                shared.identity.reserve(&nick);
                self.dirty = true;
            }
            LobbyMsg::JoinFailed {
                game,
                player,
                nick,
                why,
            } => {
                self.claims.remove(&(game, player));
                self.joining.remove(&key(&nick));
                self.error(&nick, &format!("cannot take the slot: {why}"));
                self.dirty = true;
            }
        }
    }

    // ------------------------------------------------------------ sessions

    fn login(&mut self, shared: &Arc<Shared>, info: LoginInfo) {
        let k = key(&info.nick);
        // A member of a room is back.
        self.room_lost.remove(&k);
        self.away.remove(&k);
        let seat = match self.places.get(&k) {
            Some(Place::Seat { game, player }) => Some((*game, *player)),
            _ => None,
        };
        self.sessions.insert(
            k.clone(),
            Sess {
                id: info.session,
                out: info.out.clone(),
                link: info.link.clone(),
                choices_sent: false,
                nick: info.nick.clone(),
            },
        );
        // The online list of the others changes (the new session gets its own state below).
        self.send_others(shared, &k);
        if let Some((game, player)) = seat {
            if let Some(h) = shared.games.get(game) {
                h.send(ActorMsg::Attach(AttachRequest {
                    player,
                    nick: info.nick.clone(),
                    session: info.session,
                    out: info.out,
                    link: info.link,
                    resume: info.resume,
                    claim: info.resume.and_then(|r| r.basis),
                }));
                return;
            }
            // A game that sleeps wakes for its player; the lobby shows it as starting until `Attached`.
            // A room made from its save that keeps a slot for the player is the way back instead (spec 4.8
            // `LoadSave`): the game sleeps on.
            if shared.games.dormant(game).is_some() && self.room_waits_for(&info.nick, game) {
                self.send_lobby(shared, &info.nick);
                return;
            }
            if shared.games.dormant(game).is_some() {
                if let Err(why) = crate::game::restore::wake(shared, game) {
                    self.send_lobby(shared, &info.nick);
                    self.error(&info.nick, &format!("cannot wake your game: {why}"));
                    return;
                }
                self.dirty = true;
                self.send_lobby(shared, &info.nick);
                return;
            }
            // The game is gone: the nick is free.
            self.places.remove(&key(&info.nick));
            shared.identity.release(&info.nick);
        }
        self.send_lobby(shared, &info.nick);
    }

    fn gone(&mut self, shared: &Arc<Shared>, nick: &str, session: u64) {
        let k = key(nick);
        if self.sessions.get(&k).is_none_or(|s| s.id != session) {
            return;
        }
        self.sessions.remove(&k);
        self.send_others(shared, &k);
        match self.places.get(&k).copied() {
            Some(Place::Seat { game, player }) => {
                if let Some(h) = shared.games.get(game) {
                    h.send(ActorMsg::SessionGone { player, session });
                }
            }
            // A member of a room stays in it for `host_lost_s`: `sweep_room_members`.
            Some(Place::Room(_)) => {
                self.room_lost.insert(k, shared.config.clock.now_ms());
            }
            None => {}
        }
    }

    /// Room members whose session has been gone for `host_lost_s` leave their room; a host hands
    /// it on by the rule of `LeaveRoom`.
    fn sweep_room_members(&mut self, shared: &Arc<Shared>) {
        let now = shared.config.clock.now_ms();
        let limit = u64::from(shared.config.host_lost_s) * 1000;
        let due: Vec<String> = self
            .room_lost
            .iter()
            .filter(|(_, since)| now.saturating_sub(**since) >= limit)
            .map(|(k, _)| k.clone())
            .collect();
        for k in due {
            self.room_lost.remove(&k);
            let Some(Place::Room(id)) = self.places.get(&k).copied() else {
                continue;
            };
            let Some(nick) = self
                .rooms
                .get(&id)
                .and_then(|r| r.humans().into_iter().find(|h| key(h) == k))
            else {
                continue;
            };
            // A room that is starting keeps its members (the game takes them over).
            if self.leave_room(shared, &nick).is_ok() {
                tracing::info!(%nick, room = id, "left the room: no session for {} s", limit / 1000);
                self.dirty = true;
            }
        }
    }

    /// `Choices` once per session, then the lobby view of this nick.
    fn send_lobby(&mut self, shared: &Arc<Shared>, nick: &str) {
        let k = key(nick);
        let state = self.state_for(shared, nick);
        let Some(s) = self.sessions.get_mut(&k) else {
            return;
        };
        if !s.choices_sent {
            s.choices_sent = true;
            push(&s.out, &ServerMsg::Choices((*shared.choices()).clone()));
        }
        push(&s.out, &ServerMsg::LobbyState(state));
    }

    fn error(&self, nick: &str, text: &str) {
        if let Some(s) = self.sessions.get(&key(nick)) {
            push(
                &s.out,
                &ServerMsg::LobbyError {
                    text: text.to_owned(),
                },
            );
        }
    }

    // ------------------------------------------------------------ state

    fn state_for(&self, shared: &Arc<Shared>, nick: &str) -> LobbyState {
        let me = key(nick);
        let room = match self.places.get(&me) {
            Some(Place::Room(id)) => self.rooms.get(id).map(Room::state),
            _ => None,
        };
        // Only somebody without a place can take an AI slot of somebody else's game.
        let free = !self.places.contains_key(&me) && !self.joining.contains_key(&me);
        let games = shared
            .games
            .list()
            .iter()
            .filter_map(|g| {
                let status = g.info.status()?;
                let snap = g.info.snapshot();
                let humans = snap
                    .seats
                    .iter()
                    .filter(|s| matches!(s.control, SeatControl::Human { .. }))
                    .count();
                let running = snap.state == GameState::Running;
                Some(protocol::GameSummary {
                    id: g.id,
                    name: g.info.name.clone(),
                    turn: snap.turn,
                    world_size: g.info.world_size.clone(),
                    host: snap.host.clone(),
                    status,
                    seats: snap
                        .seats
                        .iter()
                        .map(|s| SeatSummary {
                            player: s.player,
                            civ_name: s.civ_name.clone(),
                            control: s.control.clone(),
                            online: s.online,
                            takeable: match &s.control {
                                // An AI slot of a running game (N-D8), while a human is still missing.
                                // The lists do not know which majors are dead: `JoinGame` into the
                                // slot of a dead one is refused by the core with a `LobbyError`.
                                SeatControl::Ai { .. } => {
                                    free && running
                                        && humans < MAX_HUMANS
                                        && !self.claims.contains_key(&(g.id, s.player))
                                }
                                // The own seat after a `Leave` or a drop: `JoinGame` takes it back.
                                SeatControl::Human { nick: owner } => {
                                    running && key(owner) == me && !s.online
                                }
                                SeatControl::Closed => false,
                            },
                        })
                        .collect(),
                })
            })
            .collect();
        let mut games: Vec<protocol::GameSummary> = games;
        let choices = shared.choices();
        for m in shared.games.dormant_list() {
            let snaps = crate::game::restore::seat_snaps(&m, &choices);
            let humans = m.humans().len();
            games.push(protocol::GameSummary {
                id: m.id,
                name: m.name.clone(),
                turn: m.turn,
                world_size: m.settings.world_size.clone(),
                host: m.host.clone(),
                status: GameStatus::Hibernated,
                seats: snaps
                    .into_iter()
                    .map(|s| SeatSummary {
                        player: s.player,
                        civ_name: s.civ_name,
                        takeable: match &s.control {
                            SeatControl::Ai { .. } => free && humans < MAX_HUMANS,
                            SeatControl::Human { nick: owner } => key(owner) == me,
                            SeatControl::Closed => false,
                        },
                        control: s.control,
                        online: false,
                    })
                    .collect(),
            });
        }
        games.sort_by_key(|g| g.id);
        LobbyState {
            rooms: self.rooms.values().map(Room::summary).collect(),
            games,
            saves: self.saves.list(shared.health.core_build().as_deref()),
            room,
            online: self.online(shared),
        }
    }

    /// Every signed-in player and where they are (sorted by nick, case-insensitive). A room member whose session
    /// is gone is not online; a nick has one session, so it is listed once.
    fn online(&self, shared: &Arc<Shared>) -> Vec<OnlinePlayer> {
        let mut list: Vec<OnlinePlayer> = self
            .sessions
            .keys()
            .map(|k| {
                let nick = self.nick_of(k);
                let place = match self.places.get(k) {
                    Some(Place::Room(id)) => self.rooms.get(id).map(|r| PlayerPlace::Room {
                        room: r.id,
                        name: r.settings.name.clone(),
                        host: r.is_host(&nick),
                        ready: r.slots.iter().any(|s| {
                            matches!(&s.occupant, Occupant::Human { nick: n, ready: true } if key(n) == *k)
                        }),
                    }),
                    Some(Place::Seat { game, .. }) if !self.away.contains(k) => {
                        shared.games.get(*game).map(|h| PlayerPlace::Game {
                            game: *game,
                            name: h.info.name.clone(),
                            turn: h.info.snapshot().turn,
                        })
                    }
                    _ => None,
                };
                OnlinePlayer {
                    nick,
                    place: place.unwrap_or(PlayerPlace::Lobby),
                }
            })
            .collect();
        list.sort_by_key(|p| (key(&p.nick), p.nick.clone()));
        list
    }

    /// The nick as the player typed it (the sessions are keyed by the lower-case nick).
    fn nick_of(&self, k: &str) -> String {
        self.sessions
            .get(k)
            .map_or_else(|| k.to_owned(), |s| s.nick.clone())
    }

    /// `LobbyState` at once to everybody in the lobby but `except`: a sign-in or a drop changes the online list
    /// of the others (the nick that signs in gets its own state from `login`).
    fn send_others(&mut self, shared: &Arc<Shared>, except: &str) {
        let nicks: Vec<String> = self
            .sessions
            .keys()
            .filter(|k| {
                k.as_str() != except
                    && (!matches!(self.places.get(*k), Some(Place::Seat { .. }))
                        || self.away.contains(*k))
            })
            .cloned()
            .collect();
        for k in nicks {
            self.send_lobby(shared, &k);
        }
    }

    /// `LobbyState` to everybody who is in the lobby (not in a game), at most every 100 ms.
    fn broadcast(&mut self, shared: &Arc<Shared>) {
        self.dirty = false;
        self.last_broadcast = Some(Instant::now());
        let nicks: Vec<String> = self
            .sessions
            .keys()
            .filter(|k| {
                !matches!(self.places.get(*k), Some(Place::Seat { .. })) || self.away.contains(*k)
            })
            .cloned()
            .collect();
        for k in nicks {
            self.send_lobby(shared, &k);
        }
    }

    // ------------------------------------------------------------ commands

    fn command(&mut self, shared: &Arc<Shared>, nick: &str, cmd: LobbyCmd) {
        let k = key(nick);
        let result = match cmd {
            LobbyCmd::CreateRoom { settings } => self.create_room(shared, nick, settings),
            LobbyCmd::JoinRoom { room } => self.join_room(shared, nick, room),
            LobbyCmd::LeaveRoom => self.leave_room(shared, nick),
            LobbyCmd::Settings(s) => {
                let choices = shared.choices();
                self.my_room(&k)
                    .and_then(|r| r.set_settings(nick, s, &choices))
            }
            LobbyCmd::Slot {
                index,
                occupant,
                civ,
            } => match validate::civ(&shared.choices(), &civ)
                .and_then(|()| self.my_room(&k))
                .and_then(|r| r.slot(nick, index, occupant, civ))
            {
                Ok(SlotEffect::Removed(gone)) => {
                    self.out_of_room(shared, &gone);
                    self.error(&gone, "the host removed you from the room");
                    self.send_lobby(shared, &gone);
                    Ok(())
                }
                Ok(SlotEffect::None) => Ok(()),
                Err(e) => Err(e),
            },
            LobbyCmd::PickCiv { civ } => validate::civ(&shared.choices(), &civ)
                .and_then(|()| self.my_room(&k))
                .and_then(|r| r.pick_civ(nick, civ)),
            LobbyCmd::TakeSlot { index } => self.my_room(&k).and_then(|r| r.take_slot(nick, index)),
            LobbyCmd::Ready(v) => self.my_room(&k).and_then(|r| r.set_ready(nick, v)),
            LobbyCmd::Start => self.start(shared, nick),
            LobbyCmd::LoadSave { save } => self.load_save(shared, nick, &save),
            LobbyCmd::JoinGame {
                game,
                player,
                basis,
            } => self.join_game(shared, nick, game, player, basis),
            LobbyCmd::LeaveSeat => self.leave_seat(shared, nick),
        };
        match result {
            Ok(()) => self.dirty = true,
            Err(text) => self.error(nick, &text),
        }
    }

    fn my_room(&mut self, k: &str) -> Result<&mut Room, String> {
        match self.places.get(k) {
            Some(Place::Room(id)) => self
                .rooms
                .get_mut(id)
                .ok_or_else(|| "no such room".to_owned()),
            Some(Place::Seat { .. }) => Err("you are in a game".to_owned()),
            None => Err("you are not in a room".to_owned()),
        }
    }

    fn free_to_join(&self, nick: &str) -> Result<(), String> {
        match self.places.get(&key(nick)) {
            None => Ok(()),
            Some(Place::Room(_)) => Err("you are in a room already".to_owned()),
            Some(Place::Seat { .. }) => Err("you have a seat in a game".to_owned()),
        }
    }

    fn create_room(
        &mut self,
        shared: &Arc<Shared>,
        nick: &str,
        settings: protocol::GameSettings,
    ) -> Result<(), String> {
        self.free_to_join(nick)?;
        if self.joining.contains_key(&key(nick)) {
            return Err("you are joining a game".to_owned());
        }
        let choices = shared.choices();
        if choices.world_sizes.is_empty() {
            return Err("the lobby is closed: no pick lists yet".to_owned());
        }
        // A new room starts with the defaults of the map script's options, filled in by the server for every client.
        let mut settings = settings;
        validate::fill_map_options(&mut settings, None, &choices);
        validate::settings(&settings, &choices)?;
        if self.rooms.len() >= MAX_ROOMS {
            return Err("too many rooms".to_owned());
        }
        let size = validate::world_size(&choices, &settings.world_size)
            .ok_or_else(|| "unknown world size".to_owned())?;
        let room = Room::new(shared.ids.next_room(), nick, settings, size);
        self.places.insert(key(nick), Place::Room(room.id));
        shared.identity.reserve(nick);
        self.rooms.insert(room.id, room);
        Ok(())
    }

    fn join_room(&mut self, shared: &Arc<Shared>, nick: &str, id: RoomId) -> Result<(), String> {
        let k = key(nick);
        let seat = match self.places.get(&k) {
            Some(Place::Seat { game, player }) => Some((*game, *player)),
            _ => None,
        };
        match seat {
            Some((game, _)) => self.seat_may_join(shared, nick, game, id)?,
            None => self.free_to_join(nick)?,
        }
        if self.joining.contains_key(&k) {
            return Err("you are joining a game".to_owned());
        }
        let room = self
            .rooms
            .get_mut(&id)
            .ok_or_else(|| "no such room".to_owned())?;
        room.join(nick)?;
        if let Some(seat) = seat {
            self.seat_before.insert(k.clone(), seat);
        }
        self.places.insert(k, Place::Room(id));
        Ok(())
    }

    /// A nick seated in `game` joins room `id` only when the room is made from a save of that game, keeps a
    /// slot for the nick (`reserved_for`), and the game sleeps: a running game is the
    /// nick's own seat, to return to with `JoinGame`.
    fn seat_may_join(
        &self,
        shared: &Arc<Shared>,
        nick: &str,
        game: GameId,
        id: RoomId,
    ) -> Result<(), String> {
        let room = self
            .rooms
            .get(&id)
            .ok_or_else(|| "no such room".to_owned())?;
        let of_my_game = room.from_save.as_ref().is_some_and(|f| f.game == game);
        if !of_my_game || !room.reserves(nick) {
            return Err("you have a seat in a game".to_owned());
        }
        if shared.games.get(game).is_some() {
            return Err("your game is running: return to your seat with JoinGame".to_owned());
        }
        Ok(())
    }

    /// The nick leaves its room: its seat of before is its place again, or the nick is free.
    fn out_of_room(&mut self, shared: &Arc<Shared>, nick: &str) {
        let k = key(nick);
        self.room_lost.remove(&k);
        match self.seat_before.remove(&k) {
            Some((game, player)) => {
                // The slot of the save is the nick's seat: it waits for him again.
                if let Some(Place::Room(id)) = self.places.get(&k)
                    && let Some(r) = self.rooms.get_mut(id)
                    && r.from_save.as_ref().is_some_and(|f| f.game == game)
                    && let Ok(index) = usize::try_from(player)
                {
                    r.reserve_again(index, nick);
                }
                self.places.insert(k, Place::Seat { game, player });
            }
            None => {
                self.places.remove(&k);
                shared.identity.release(nick);
            }
        }
    }

    /// A room made from a save of `game` keeps a slot for `nick`: the nick's sleeping game does not wake at its
    /// login, the nick takes the slot instead.
    fn room_waits_for(&self, nick: &str, game: GameId) -> bool {
        self.rooms.values().any(|r| {
            r.status == GameStatus::Lobby
                && r.from_save.as_ref().is_some_and(|f| f.game == game)
                && r.reserves(nick)
        })
    }

    fn leave_room(&mut self, shared: &Arc<Shared>, nick: &str) -> Result<(), String> {
        let k = key(nick);
        let Some(Place::Room(id)) = self.places.get(&k).copied() else {
            return Err("you are not in a room".to_owned());
        };
        let room = self
            .rooms
            .get_mut(&id)
            .ok_or_else(|| "no such room".to_owned())?;
        if room.status != GameStatus::Lobby {
            return Err("the game is starting".to_owned());
        }
        let sessions = &self.sessions;
        let empty = room.leave(nick, |n| sessions.contains_key(&key(n)));
        if empty {
            self.rooms.remove(&id);
        }
        self.out_of_room(shared, nick);
        Ok(())
    }

    fn start(&mut self, shared: &Arc<Shared>, nick: &str) -> Result<(), String> {
        let k = key(nick);
        let choices = shared.choices();
        let mut setup = self
            .my_room(&k)
            .and_then(|r| r.start_setup(nick, &choices))?;
        if let Some(why) = crate::game::restore::no_room_for_a_game(shared) {
            return Err(why.to_owned());
        }
        let Some(Place::Room(id)) = self.places.get(&k).copied() else {
            return Err("you are not in a room".to_owned());
        };
        // A room from a save continues that save (the worker's `Start::Load`).
        let load = match self.rooms.get(&id).and_then(|r| r.from_save.clone()) {
            Some(f) => {
                let dir = shared
                    .supervisor()
                    .map(|s| s.config().state_dir.join("games").join(f.game.to_string()))
                    .ok_or_else(|| "no game worker is configured".to_owned())?;
                Some(GameLoad {
                    file: dir.join(&f.file).to_string_lossy().into_owned(),
                    turn: f.turn,
                    book: None,
                })
            }
            None => None,
        };
        let game = shared.games.next_id();
        let seed = setup.settings.seed.map_or_else(random_seed, u64::from);
        // v12: the settings of the room are the defaults of the host's next room (a room from a save is not).
        let last_room = load.is_none().then(|| setup.settings.clone());
        // The host's mod picks are resolved once, here; the game keeps the frozen set (T15).
        let mods_lock = crate::game::mods::freeze(&shared.mod_store, &mut setup)?;
        // The mods with a server part need their code in this binary: else the start is refused with the sentence.
        if mods_lock.is_some() {
            crate::game::modhost::ModHost::build(
                game,
                Arc::clone(&shared.mod_store),
                Arc::clone(&shared.config.mod_registry),
                mods_lock.as_deref(),
            )?;
        }
        shared.start_game(
            GameInit {
                id: game,
                setup,
                host: nick.to_owned(),
                room: Some(id),
                seed,
                load,
                kicked: BTreeMap::new(),
                created_ms: None,
                mods_lock,
            },
            Arc::clone(&choices),
        )?;
        if let Some(r) = self.rooms.get_mut(&id) {
            r.status = GameStatus::Starting;
        }
        if let Some(settings) = last_room
            && shared.prefs.set_last_room(nick, &settings)
            && let Some(s) = self.sessions.get(&k)
        {
            push(&s.out, &ServerMsg::Account(shared.prefs.get(nick)));
        }
        Ok(())
    }

    /// `LoadSave`: a room whose slots come from the save.
    fn load_save(&mut self, shared: &Arc<Shared>, nick: &str, save: &str) -> Result<(), String> {
        self.free_to_join(nick)?;
        if self.joining.contains_key(&key(nick)) {
            return Err("you are joining a game".to_owned());
        }
        let choices = shared.choices();
        if choices.world_sizes.is_empty() {
            return Err("the lobby is closed: no pick lists yet".to_owned());
        }
        if self.rooms.len() >= MAX_ROOMS {
            return Err("too many rooms".to_owned());
        }
        let build = shared.health.core_build();
        let entry = parse_save_id(save)
            .and_then(|_| self.saves.loadable(save, build.as_deref()))
            .ok_or_else(|| "no such save".to_owned())?;
        let id = shared.ids.next_room();
        let room = Room::from_save(id, nick, entry, &choices)?;
        self.places.insert(key(nick), Place::Room(id));
        shared.identity.reserve(nick);
        self.rooms.insert(id, room);
        Ok(())
    }

    /// `JoinGame`: the own seat again (a `Leave` or a drop), or an AI slot of a running game. A slot is
    /// taken by the first request; the game confirms with `JoinDone` or refuses with `JoinFailed`.
    fn join_game(
        &mut self,
        shared: &Arc<Shared>,
        nick: &str,
        game: GameId,
        player: PlayerId,
        basis: Option<BasisClaim>,
    ) -> Result<(), String> {
        let k = key(nick);
        if self.joining.contains_key(&k) {
            return Err("you are joining a game already".to_owned());
        }
        let own = match self.places.get(&k) {
            Some(Place::Seat { game: g, player: p }) if *g == game && *p == player => true,
            Some(Place::Seat { .. }) => return Err("you have a seat in a game".to_owned()),
            Some(Place::Room(_)) => return Err("you are in a room already".to_owned()),
            None => false,
        };
        // A sleeping game: the slot is checked against its stored seats, then the game wakes.
        let asleep = match shared.games.get(game) {
            Some(_) => None,
            None => shared.games.dormant(game),
        };
        if let Some(meta) = &asleep
            && !own
        {
            let seat = meta
                .seats
                .iter()
                .find(|s| s.player == player)
                .ok_or_else(|| "the game has no such player".to_owned())?;
            if seat.nick.is_some() {
                return Err("that slot is not an AI player's".to_owned());
            }
            if meta.humans().len() >= MAX_HUMANS {
                return Err(format!("the game has {MAX_HUMANS} humans already"));
            }
            if self.claims.contains_key(&(game, player)) {
                return Err("somebody else is taking that slot".to_owned());
            }
        }
        if !self.sessions.contains_key(&k) {
            return Err("you are not logged in".to_owned());
        }
        let handle = match &asleep {
            Some(_) => crate::game::restore::wake(shared, game)?,
            None => shared
                .games
                .get(game)
                .ok_or_else(|| "no such game".to_owned())?,
        };
        let sess = self
            .sessions
            .get(&k)
            .ok_or_else(|| "you are not logged in".to_owned())?;
        if own && asleep.is_some() {
            // The game attaches its players itself as soon as the worker is ready.
            self.away.remove(&k);
            return Ok(());
        }
        if own {
            // The seat is yours: nothing changes in the game, the new connection takes it (NP 4.12).
            let sent = handle.send(ActorMsg::Attach(AttachRequest {
                player,
                nick: nick.to_owned(),
                session: sess.id,
                out: sess.out.clone(),
                link: sess.link.clone(),
                resume: None,
                claim: basis,
            }));
            if sent {
                self.away.remove(&k);
            }
            return if sent {
                Ok(())
            } else {
                Err("the game has ended".to_owned())
            };
        }
        if asleep.is_none() {
            let snap = handle.info.snapshot();
            match snap.state {
                GameState::Running => {}
                GameState::Starting | GameState::Restarting => {
                    return Err("the game is starting; try again in a moment".to_owned());
                }
                GameState::Faulted | GameState::Finished => {
                    return Err("the game is not running".to_owned());
                }
            }
            let seat = snap
                .seats
                .iter()
                .find(|s| s.player == player)
                .ok_or_else(|| "the game has no such player".to_owned())?;
            if !matches!(seat.control, SeatControl::Ai { .. }) {
                return Err("that slot is not an AI player's".to_owned());
            }
            let humans = snap
                .seats
                .iter()
                .filter(|s| matches!(s.control, SeatControl::Human { .. }))
                .count();
            if humans >= MAX_HUMANS {
                return Err(format!("the game has {MAX_HUMANS} humans already"));
            }
            // The first request wins.
            if self.claims.contains_key(&(game, player)) {
                return Err("somebody else is taking that slot".to_owned());
            }
        }
        let sent = handle.send(ActorMsg::Join(JoinRequest {
            player,
            nick: nick.to_owned(),
            session: sess.id,
            out: sess.out.clone(),
            link: sess.link.clone(),
            claim: basis,
        }));
        if !sent {
            return Err("the game has ended".to_owned());
        }
        self.claims.insert((game, player), nick.to_owned());
        self.joining.insert(k, (game, player));
        Ok(())
    }

    /// `LeaveSeat`: the nick gives up its seat. A running game does it through its actor (the safe kick, or a
    /// stop when the nick is the last human: the lobby is told `Kicked`); a sleeping game gets its stored
    /// seat changed to the AI's here. The game without humans sleeps on (an AI slot of it can still be taken).
    fn leave_seat(&mut self, shared: &Arc<Shared>, nick: &str) -> Result<(), String> {
        let k = key(nick);
        let Some(Place::Seat { game, player }) = self.places.get(&k).copied() else {
            return Err("you have no seat in a game".to_owned());
        };
        if self.joining.contains_key(&k) {
            return Err("you are joining a game".to_owned());
        }
        if let Some(h) = shared.games.get(game) {
            if h.send(ActorMsg::GiveUp {
                player,
                nick: nick.to_owned(),
            }) {
                return Ok(());
            }
        } else if let Some(mut meta) = shared.games.take_dormant(game) {
            for s in &mut meta.seats {
                if s.player == player && s.nick.as_deref().is_some_and(|n| key(n) == k) {
                    s.kicked = s.nick.take();
                }
            }
            if let Some(dir) = shared.config.state_dir.as_ref()
                && let Err(e) = meta.write(&dir.join("games").join(game.to_string()))
            {
                tracing::error!(game, error = %format!("{e:#}"), "cannot write the game meta");
            }
            shared.games.add_dormant(meta);
        }
        // A sleeping game with the stored seat changed, or a game that is gone: the nick is free.
        self.seat_before.remove(&k);
        self.places.remove(&k);
        self.away.remove(&k);
        shared.identity.release(nick);
        self.send_lobby(shared, nick);
        Ok(())
    }

    fn game_started(
        &mut self,
        shared: &Arc<Shared>,
        room: Option<RoomId>,
        game: GameId,
        seats: &[(PlayerId, String)],
    ) {
        let old_game = room
            .and_then(|id| self.rooms.get(&id))
            .and_then(|r| r.from_save.as_ref().map(|f| f.game));
        if let Some(id) = room {
            self.rooms.remove(&id);
        }
        let Some(handle) = shared.games.get(game) else {
            return;
        };
        // The sleeping game the room was made from is over before anybody is attached to the new one: a client that
        // sees `Attached` finds the old game `Finished` on disk and no sleeper (the order of these two used to be the other
        // way round, and a look right after `Attached` could find the meta of the old game not yet written).
        let ended = old_game
            .filter(|g| *g != game)
            .and_then(|old| self.finish_sleeping_game(shared, old));
        for (player, nick) in seats {
            let k = key(nick);
            self.room_lost.remove(&k);
            self.away.remove(&k);
            // A seat in the game of the save is left behind: the new game is the place now.
            self.seat_before.remove(&k);
            self.places.insert(
                k.clone(),
                Place::Seat {
                    game,
                    player: *player,
                },
            );
            shared.identity.reserve(nick);
            if let Some(s) = self.sessions.get(&k) {
                handle.send(ActorMsg::Attach(AttachRequest {
                    player: *player,
                    nick: nick.clone(),
                    session: s.id,
                    out: s.out.clone(),
                    link: s.link.clone(),
                    resume: None,
                    claim: None,
                }));
            }
        }
        // The seats of the sleeping game that were not taken into the new game are free again.
        if let Some((old, meta)) = ended {
            self.release_sleeping_seats(shared, old, &meta);
        }
        self.dirty = true;
    }

    /// A game that still sleeps when a room made from its save starts is ended (`Finished` on disk, the saves stay
    /// loadable, no sleeper any more). Its meta comes back for `release_sleeping_seats`.
    fn finish_sleeping_game(
        &mut self,
        shared: &Arc<Shared>,
        old: GameId,
    ) -> Option<(GameId, crate::state::meta::GameMeta)> {
        if shared.games.get(old).is_some() {
            return None;
        }
        let mut meta = shared.games.take_dormant(old)?;
        meta.status = MetaStatus::Finished;
        meta.finished_ms = Some(shared.config.wall_clock.unix_ms());
        if let Some(dir) = shared.config.state_dir.as_ref() {
            meta.write_logged(&dir.join("games").join(old.to_string()));
        }
        Some((old, meta))
    }

    /// The close rule for the sleeping games (the user, 2026-11-02): one with no human connected for `empty_close_min`
    /// minutes is closed, its saves stay. The count starts when the lobby first sees the game asleep, so a server
    /// start gives its players the same delay to come back.
    fn sweep_dormant(&mut self, shared: &Arc<Shared>) {
        let limit = u64::from(shared.config.empty_close_min) * 60_000;
        let list = shared.games.dormant_list();
        self.dormant_seen
            .retain(|id, _| list.iter().any(|m| m.id == *id));
        if limit == 0 {
            self.dormant_seen.clear();
            return;
        }
        let now = shared.config.clock.now_ms();
        for m in list {
            // A human who is here (in a room, or waiting for room to wake) is somebody: the count starts over.
            if m.humans()
                .iter()
                .any(|(_, nick)| self.sessions.contains_key(&key(nick)))
            {
                self.dormant_seen.remove(&m.id);
                continue;
            }
            let since = *self.dormant_seen.entry(m.id).or_insert(now);
            if now.saturating_sub(since) >= limit {
                self.close_sleeping(shared, m.id);
            }
        }
    }

    /// A sleeping game is closed: `Finished` on disk, no sleeper, its humans are free; the saves stay loadable.
    fn close_sleeping(&mut self, shared: &Arc<Shared>, game: GameId) {
        self.dormant_seen.remove(&game);
        if let Some((old, meta)) = self.finish_sleeping_game(shared, game) {
            tracing::info!(game = old, "nobody is here: the sleeping game is closed");
            self.release_sleeping_seats(shared, old, &meta);
            self.dirty = true;
        }
    }

    /// The stored seats of an ended sleeping game are released, so nobody keeps a place in a game that never wakes
    /// (a human who took a place in the new game keeps it).
    fn release_sleeping_seats(
        &mut self,
        shared: &Arc<Shared>,
        old: GameId,
        meta: &crate::state::meta::GameMeta,
    ) {
        self.claims.retain(|(g, _), _| *g != old);
        self.joining.retain(|_, (g, _)| *g != old);
        self.seat_before.retain(|_, (g, _)| *g != old);
        for (_, nick) in meta.humans() {
            let k = key(&nick);
            if matches!(self.places.get(&k), Some(Place::Seat { game: g, .. }) if *g == old) {
                self.places.remove(&k);
                self.away.remove(&k);
                shared.identity.release(&nick);
                self.send_lobby(shared, &nick);
            }
        }
    }
}

fn random_seed() -> u64 {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        return 1;
    }
    u64::from_le_bytes(b)
}
