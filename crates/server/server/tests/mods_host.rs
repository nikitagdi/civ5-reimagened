//! The mod host in a game (T17-T19, S4-S6) over real sockets with the mock worker: `ModHello` and `ModMismatch`, the
//! mod set as the first message of a `Fresh` stream, `ClientMsg::Mod` through the host limits to a server half and its
//! answers to the seats, the snapshot for a late comer, the state in the `MODS` section of the saves, the way back
//! after a gateway restart and a worker restart (the mods are rewound with the save), and a game without mods.

#![allow(dead_code, unused_imports)]

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{mock_server_config, temp_dir};
use mod_api::{Outgoing, Seat, ServerCtx, ServerMod};
use mod_host_server::HostRegistry;
use protocol::{
    AutoChoices, BuiltMod, ClientMsg, Codec, GameMsg, GameSettings, LobbyCmd, Login, ModC2S,
    ModPick, ModS2C, ModSetView, Occupant, PROTOCOL_VERSION, Postcard, RejectReason, ServerMsg,
    TimerMode, Token, TurnMode,
};
use server::clock::TestClock;
use server::game::{ActorMsg, AdminCtl};
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const PASSWORD: &str = "mods-password";
const ADMIN: &str = "admin-secret";

// ---------------------------------------------------------------- the mods

/// Echoes to the sender and to the others, counts the messages (that is its state), and says the count to a seat that
/// attaches. `boom` panics.
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

/// Answers a question about a plot from the whole-truth facts: `[x, y]` -> `[owner tag, owner value, class, turn, 0xAA]`
/// (owner tags: 0 nobody, 1 seat, 2 npc, 3 unknown; class: land, hills, mountain, water, lake, ice as 0..5). Without
/// `world.read` the answer is `[255; 5]`.
struct Probe;

impl ServerMod for Probe {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(
        &mut self,
        cx: &mut ServerCtx<'_>,
        _from: Seat,
        ch: u8,
        p: &[u8],
    ) -> Vec<Outgoing> {
        use mod_api::{Owner, PlotClass};
        let reply = match cx.facts() {
            Err(_) => vec![255; 5],
            Ok(f) => match f.plot_at(i16::from(p[0]), i16::from(p[1])) {
                None => vec![254; 5],
                Some(i) => {
                    let (t, v) = match f.owner(i) {
                        Owner::None => (0, 0),
                        Owner::Seat(s) => (1, s.player() as u8),
                        Owner::Npc(n) => (2, n as u8),
                        _ => (3, 0),
                    };
                    let c = match f.class(i) {
                        PlotClass::Land => 0,
                        PlotClass::Hills => 1,
                        PlotClass::Mountain => 2,
                        PlotClass::Water => 3,
                        PlotClass::Lake => 4,
                        _ => 5,
                    };
                    vec![t, v, c, f.turn() as u8, 0xAA]
                }
            },
        };
        vec![Outgoing::to_sender(ch, reply)]
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        Vec::new()
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// API 1.1: every tick says `[T, now_ms as 8 LE bytes]` to all; a seat whose client goes says `[D, seat]`.
struct Ticker;

impl ServerMod for Ticker {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, _: &mut ServerCtx<'_>, _: Seat, _: u8, _: &[u8]) -> Vec<Outgoing> {
        Vec::new()
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        Vec::new()
    }
    fn on_tick(&mut self, _cx: &mut ServerCtx<'_>, now_ms: u64) -> Vec<Outgoing> {
        let mut p = vec![b'T'];
        p.extend_from_slice(&now_ms.to_le_bytes());
        vec![Outgoing::to_all(0, p)]
    }
    fn on_detach(&mut self, _cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        vec![Outgoing::to_all(0, vec![b'D', seat.0 as u8])]
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// API 1.2: `mine` shares plots 3, 4, 5 (water, a lake, a hill of the `map=1` map; and 4 again) to the sender, `end` unshares the sender; each answers `[A]` to
/// the sender after the call, so a test knows the batch with the share has been sent.
struct Arena;

impl ServerMod for Arena {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        let r = match p {
            b"mine" => cx.share_terrain(from, &[3, 4, 5, 4]),
            b"end" => cx.unshare_terrain(from),
            _ => Ok(()),
        };
        assert!(r.is_ok(), "{r:?}");
        vec![Outgoing::to_sender(ch, vec![b'A'])]
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
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
        r.add("probe", || Box::new(Probe));
        r.add("blind", || Box::new(Probe));
        r.add("ticker", || Box::new(Ticker));
        r.add("arena", || Box::new(Arena));
    });
    Arc::new(r)
}

/// Installs a mod with a server half under `<dir>/mods/<id>`: one channel, 64 bytes in, 20 messages burst.
fn install(dir: &Path, id: &str, sync: &str, client: &str) -> PathBuf {
    install_caps(dir, id, sync, client, r#""net.channel""#)
}

fn install_caps(dir: &Path, id: &str, sync: &str, client: &str, caps: &str) -> PathBuf {
    install_rust(dir, id, sync, client, caps, "")
}

/// [install_caps] with more keys in `[rust]` (such as `tick_hz`).
fn install_rust(
    dir: &Path,
    id: &str,
    sync: &str,
    client: &str,
    caps: &str,
    rust_extra: &str,
) -> PathBuf {
    let root = dir.join("mods");
    std::fs::create_dir_all(root.join(id)).unwrap();
    std::fs::write(
        root.join(id).join("mod.toml"),
        format!(
            r#"[mod]
id = "{id}"
name = "{id}"
version = "1.0.0"
api = "^1"
license = "MIT"
sync = "{sync}"
{client}
parts = ["server-rs"]

[rust]
server = {{ crate = "{id}-server", entry = "{id}_server::register" }}
caps = [{caps}]
state_max = 64
{rust_extra}

[[channels]]
id = 0
name = "main"
c2s_max = 64
s2c_max = 256
rate = "50/s"
burst = 20
"#
        ),
    )
    .unwrap();
    root
}

/// `hello` (gameplay: every client needs it) and `flair` (cosmetic, optional for clients).
fn install_both(dir: &Path) -> PathBuf {
    install(dir, "flair", "cosmetic", "client = \"optional\"");
    install(dir, "hello", "gameplay", "")
}

// ---------------------------------------------------------------- the server

struct TestServer {
    addr: SocketAddr,
    admin: SocketAddr,
    dir: PathBuf,
    clock: Arc<TestClock>,
    shared: Arc<Shared>,
    rt: Option<tokio::runtime::Runtime>,
}

impl TestServer {
    /// The gateway stops (its workers get EOF); the state stays.
    fn stop(mut self) -> PathBuf {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
        self.dir.clone()
    }
}

fn boot(dir: &Path, mods_dir: Option<&Path>, edit: impl FnOnce(&mut ServerConfig)) -> TestServer {
    boot_mode(dir, mods_dir, "echo turns=1", edit)
}

fn boot_mode(
    dir: &Path,
    mods_dir: Option<&Path>,
    mode: &str,
    edit: impl FnOnce(&mut ServerConfig),
) -> TestServer {
    let clock = Arc::new(TestClock::new());
    let mut config = mock_server_config(
        dir,
        mode,
        Duration::from_secs(20),
        [Duration::ZERO, Duration::ZERO, Duration::ZERO],
    );
    config.password = Some(Secret::new(PASSWORD));
    config.admin_token = Some(Secret::new(ADMIN));
    config.limits = LimitsConfig {
        password_delay: Duration::from_millis(5),
        ..LimitsConfig::default()
    };
    config.clock = clock.clone();
    config.mods_dir = mods_dir.map(Path::to_path_buf);
    config.mod_registry = registry();
    config.test_hooks = true;
    edit(&mut config);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let bind = || {
        rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap()
    };
    let (listener, admin) = (bind(), bind());
    let (addr, admin_addr) = (listener.local_addr().unwrap(), admin.local_addr().unwrap());
    let shared = Shared::new(config).unwrap();
    rt.block_on(shared.supervisor().unwrap().publish_describe(&shared))
        .expect("the mock Describe works");
    {
        let _guard = rt.enter();
        server::game::restore::restore_all(&shared).unwrap();
    }
    rt.spawn(server::serve(listener, shared.clone()));
    rt.spawn(server::serve_admin(admin, shared.clone()));
    TestServer {
        addr,
        admin: admin_addr,
        dir: dir.to_path_buf(),
        clock,
        shared,
        rt: Some(rt),
    }
}

fn http(addr: SocketAddr, method: &str, path: &str) -> String {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.0\r\nHost: localhost\r\nX-Admin-Token: {ADMIN}\r\nContent-Length: 0\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    raw
}

fn admin_game(srv: &TestServer, game: u32, ctl: AdminCtl) {
    assert!(
        srv.shared
            .games
            .get(game)
            .unwrap()
            .send(ActorMsg::Admin(ctl))
    );
}

/// The newest save file of the game, decoded.
fn newest_save(dir: &Path, game: u32) -> game_host::saves::Save {
    let ring = dir.join("games").join(game.to_string()).join("ring");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&ring)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "c5sv"))
            .collect();
        files.sort();
        if let Some(f) = files.last() {
            return game_host::saves::decode(&std::fs::read(f).unwrap()).expect("a C5SV save");
        }
        assert!(Instant::now() < deadline, "no save of game {game}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Waits (real time) until the newest save of the game says `count` for the one mod with state.
fn wait_saved_count(dir: &Path, game: u32, count: u32) -> game_host::saves::Save {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let save = newest_save(dir, game);
        if save
            .mods
            .as_ref()
            .is_some_and(|m| m.states.iter().any(|s| s.state == count.to_le_bytes()))
        {
            return save;
        }
        assert!(
            Instant::now() < deadline,
            "the save does not say {count}: {:?}",
            save.mods
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// ---------------------------------------------------------------- a client

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

struct Client {
    ws: Ws,
    nick: String,
    token: Option<Token>,
    /// Every `Game` batch heard, in order.
    batches: Vec<Vec<GameMsg>>,
    /// `Mods` heard.
    mods: Vec<ModSetView>,
    /// `Mod` messages heard, in order (the next to look at is `taken`).
    inbox: Vec<ModS2C>,
    taken: usize,
}

impl Client {
    /// `hello`: the mods the client has built in (`None`: it sends no `ModHello`).
    fn connect(
        srv: &TestServer,
        nick: &str,
        token: Option<Token>,
        hello: Option<&[(&str, &str)]>,
    ) -> Client {
        let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", srv.addr)).unwrap();
        if let MaybeTlsStream::Plain(s) = ws.get_mut() {
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        }
        let mut c = Client {
            ws,
            nick: nick.into(),
            token,
            batches: Vec::new(),
            mods: Vec::new(),
            inbox: Vec::new(),
            taken: 0,
        };
        c.send(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "test".into(),
            nick: nick.into(),
        });
        if let Some(h) = hello {
            c.send(&ClientMsg::ModHello {
                api: (1, 0, 0),
                built_in: h
                    .iter()
                    .map(|(id, v)| BuiltMod {
                        id: (*id).into(),
                        version: (*v).into(),
                        code_hash: [0; 16],
                    })
                    .collect(),
            });
        }
        c.send(&ClientMsg::Login(Login {
            password: if token.is_some() {
                String::new()
            } else {
                PASSWORD.into()
            },
            token,
            resume: None,
        }));
        c
    }

    fn send(&mut self, msg: &ClientMsg) {
        let body = Postcard.encode(msg).unwrap();
        self.ws
            .send(Message::binary(Encoder::plain(&body)))
            .unwrap();
    }

    fn mod_msg(&mut self, mod_idx: u16, payload: &[u8]) {
        self.send(&ClientMsg::Mod(ModC2S {
            mod_idx,
            ch: 0,
            payload: payload.to_vec(),
        }));
    }

    /// The next server message; `None` when the socket is closed.
    fn try_recv(&mut self) -> Option<ServerMsg> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                Instant::now() < deadline,
                "{}: nothing came within 15 s",
                self.nick
            );
            match self.ws.read() {
                Ok(Message::Binary(b)) => {
                    let msg: ServerMsg = wire::decode(&b, &Limits::CLIENT).unwrap();
                    match &msg {
                        ServerMsg::LoggedIn(s) => self.token = s.token,
                        ServerMsg::Game(b) => {
                            self.batches.push(b.msgs.clone());
                            for m in &b.msgs {
                                match m {
                                    GameMsg::Mods(v) => self.mods.push(v.clone()),
                                    GameMsg::Mod(m) => self.inbox.push(m.clone()),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                    return Some(msg);
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    }

    fn recv(&mut self) -> ServerMsg {
        self.try_recv()
            .unwrap_or_else(|| panic!("{}: the socket closed", self.nick))
    }

    fn until(&mut self, what: &str, mut pred: impl FnMut(&ServerMsg) -> bool) -> ServerMsg {
        for _ in 0..400 {
            let m = self.recv();
            if pred(&m) {
                return m;
            }
        }
        panic!("{}: no {what}", self.nick);
    }

    /// The next `Mod` message from the stream that satisfies `pred` (the earlier ones are passed over).
    fn next_mod(&mut self, what: &str, pred: impl Fn(&ModS2C) -> bool) -> ModS2C {
        for _ in 0..400 {
            while self.taken < self.inbox.len() {
                let m = self.inbox[self.taken].clone();
                self.taken += 1;
                if pred(&m) {
                    return m;
                }
            }
            self.recv();
        }
        panic!(
            "{}: no mod message: {what}; heard {:?}",
            self.nick, self.inbox
        );
    }

    /// The socket is closed by the server within a while (`Reject` and such messages are read on the way).
    fn closed(&mut self) -> Vec<ServerMsg> {
        let mut heard = Vec::new();
        for _ in 0..400 {
            match self.try_recv() {
                Some(m) => heard.push(m),
                None => return heard,
            }
        }
        panic!("{}: the socket stays open", self.nick);
    }

    fn attached(&mut self) {
        self.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    }

    /// The first batch of the stream has been read (the `Mods` message is in it).
    fn await_mods(&mut self) {
        while self.mods.is_empty() {
            self.recv();
        }
    }
}

fn settings(picks: &[&str]) -> GameSettings {
    GameSettings {
        name: "mods".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "m".into(),
        map_options: vec![],
        speed: "s".into(),
        start_era: "e".into(),
        handicap: "h".into(),
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
        mods: picks
            .iter()
            .map(|id| ModPick {
                id: (*id).into(),
                req: "*".into(),
            })
            .collect(),
    }
}

/// alice (host) and bob in a game that picked `picks`; `alice` and `bob` are what each says it has built in.
/// Returns the clients after `Start`, without waiting for the attach.
fn start_game(
    srv: &TestServer,
    picks: &[&str],
    alice: Option<&[(&str, &str)]>,
    bob: Option<&[(&str, &str)]>,
) -> (Client, Client) {
    let mut h = Client::connect(srv, "alice", None, alice);
    h.until("the lobby", |m| matches!(m, ServerMsg::LobbyState(_)));
    h.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(picks),
    }));
    let ServerMsg::LobbyState(s) = h.until(
        "the room",
        |m| matches!(m, ServerMsg::LobbyState(s) if s.room.is_some()),
    ) else {
        unreachable!()
    };
    let room = s.room.unwrap().id;
    let mut g = Client::connect(srv, "bob", None, bob);
    g.until(
        "the room in the list",
        |m| matches!(m, ServerMsg::LobbyState(s) if !s.rooms.is_empty()),
    );
    g.send(&ClientMsg::Lobby(LobbyCmd::JoinRoom { room }));
    g.until("in the room", |m| {
        matches!(m, ServerMsg::LobbyState(s) if s.room.as_ref().is_some_and(|r|
            r.slots.iter().any(|x| matches!(&x.occupant, Occupant::Human { nick, .. } if nick == "bob"))))
    });
    g.send(&ClientMsg::Lobby(LobbyCmd::Ready(true)));
    h.until("ready", |m| {
        matches!(m, ServerMsg::LobbyState(s) if s.room.as_ref().is_some_and(|r|
            r.slots.iter().any(|x| matches!(&x.occupant, Occupant::Human { ready: true, .. }))))
    });
    h.send(&ClientMsg::Lobby(LobbyCmd::Start));
    (h, g)
}

const BOTH: &[(&str, &str)] = &[("hello", "1.0.0"), ("flair", "1.0.0")];
const HELLO_ONLY: &[(&str, &str)] = &[("hello", "1.0.0")];

fn count(payload: &[u8]) -> u32 {
    u32::from_le_bytes(payload.try_into().expect("a count is 4 bytes"))
}

// ---------------------------------------------------------------- tests

/// T17 + T18: the mod set opens the stream, a message goes through the host to the server half, and its answers
/// reach the sender and the other seat; both are in the recording, with the new tag.
#[test]
fn two_clients_echo_through_the_envelope_and_the_mod_set_comes_first() {
    let dir = temp_dir("mh-echo");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    let (mut alice, mut bob) = start_game(&srv, &["hello"], Some(HELLO_ONLY), Some(HELLO_ONLY));
    alice.attached();
    bob.attached();
    // The first batch of the stream starts with the mod set (before the map basis and the keyframe).
    for c in [&mut alice, &mut bob] {
        c.next_mod("the snapshot", |m| m.payload.len() == 4);
        let first = c.batches.first().expect("a batch");
        let Some(GameMsg::Mods(v)) = first.first() else {
            panic!("{}: the stream does not open with Mods: {first:?}", c.nick);
        };
        assert_eq!(v.mods.len(), 1);
        assert_eq!((v.mods[0].id.as_str(), v.mods[0].load_idx), ("hello", 0));
        assert!(v.mods[0].available && v.mods[0].client_required);
        assert!(first.iter().any(|m| matches!(m, GameMsg::Keyframe(_))));
    }
    alice.mod_msg(0, b"ping");
    let mine = alice.next_mod("her echo", |m| m.payload == b"ping");
    let theirs = bob.next_mod("the broadcast", |m| m.payload == b"ping");
    assert_eq!((mine.mod_idx, mine.ch, mine.from), (0, 0, Some(0)));
    assert_eq!(
        theirs.from,
        Some(0),
        "the host sets `from` to the sender's seat"
    );
    // Bob answers; alice hears it from seat 1.
    bob.mod_msg(0, b"pong");
    assert_eq!(
        alice.next_mod("pong", |m| m.payload == b"pong").from,
        Some(1)
    );

    // The recording has both directions under tag 15.
    use recording::Recording;
    use recording::format::Body;
    let rec = std::fs::read_dir(dir.join("recordings"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("g1-"))
        })
        .expect("the game has a recording");
    let mut recs = Vec::new();
    for _ in 0..100 {
        recs = Recording::open(&rec)
            .and_then(|r| r.read_all())
            .map(|segs| {
                segs.into_iter()
                    .flat_map(|s| s.entries)
                    .filter_map(|e| match e.body {
                        Body::Mod(m) => Some(m),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if recs.iter().any(|m| m.c2s && m.payload == b"pong")
            && recs.iter().any(|m| !m.c2s && m.payload == b"pong")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let ping_in = recs
        .iter()
        .find(|m| m.c2s && m.payload == b"ping")
        .expect("ping in");
    assert_eq!(
        (ping_in.mod_idx, ping_in.player, ping_in.taken),
        (0, Some(0), true)
    );
    let ping_out = recs
        .iter()
        .filter(|m| !m.c2s && m.payload == b"ping")
        .map(|m| (m.player, m.to.clone()))
        .collect::<Vec<_>>();
    assert_eq!(ping_out, [(Some(0), vec![0]), (Some(0), vec![1])]);
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T17 (S5): a client without a gameplay mod is refused with the name and the version; the other seat plays on.
#[test]
fn a_client_without_a_required_mod_gets_mod_mismatch_naming_the_mod_and_the_version() {
    let dir = temp_dir("mh-mismatch");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    // Bob sends no `ModHello` at all.
    let (mut alice, mut bob) = start_game(&srv, &["hello"], Some(HELLO_ONLY), None);
    alice.attached();
    let heard = bob.closed();
    let Some(ServerMsg::Reject(RejectReason::ModMismatch(problems))) =
        heard.iter().find(|m| matches!(m, ServerMsg::Reject(_)))
    else {
        panic!("no ModMismatch: {heard:?}");
    };
    assert_eq!(problems.len(), 1);
    assert!(
        problems[0].text.contains("`hello`") && problems[0].text.contains("1.0.0"),
        "{}",
        problems[0].text
    );
    assert!(
        !heard.iter().any(|m| matches!(m, ServerMsg::Attached(_))),
        "a refused client is not attached"
    );
    // Alice is not disturbed.
    alice.mod_msg(0, b"alone");
    alice.next_mod("her echo", |m| m.payload == b"alone");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T17 (S5): a client without an optional mod plays; the mod is marked unavailable for it and sends it nothing.
#[test]
fn a_client_without_an_optional_mod_plays_and_hears_nothing_of_it() {
    let dir = temp_dir("mh-optional");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    // `flair` comes before `hello` in the load order (ids are sorted): index 0.
    let (mut alice, mut bob) = start_game(&srv, &["hello", "flair"], Some(BOTH), Some(HELLO_ONLY));
    alice.attached();
    bob.attached();
    alice.await_mods();
    bob.await_mods();
    let seen = |c: &Client| {
        c.mods[0]
            .mods
            .iter()
            .map(|m| (m.id.clone(), m.available))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        seen(&alice),
        [("flair".to_owned(), true), ("hello".to_owned(), true)]
    );
    assert_eq!(
        seen(&bob),
        [("flair".to_owned(), false), ("hello".to_owned(), true)]
    );
    // Alice talks to `flair` (index 0) and to `hello` (index 1); bob hears only `hello`.
    alice.mod_msg(0, b"to flair");
    alice.next_mod("her flair echo", |m| m.payload == b"to flair");
    alice.mod_msg(1, b"to hello");
    alice.next_mod("her hello echo", |m| m.payload == b"to hello");
    bob.next_mod("the hello broadcast", |m| m.payload == b"to hello");
    assert!(
        bob.inbox.iter().all(|m| m.mod_idx == 1),
        "nothing of the mod bob lacks: {:?}",
        bob.inbox
    );
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T19: a client that attaches with a `Fresh` stream is told the state of the mods (the pattern of the chat history).
#[test]
fn a_late_comer_gets_the_state_of_the_mods() {
    let dir = temp_dir("mh-late");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    let (mut alice, mut bob) = start_game(&srv, &["hello"], Some(HELLO_ONLY), Some(HELLO_ONLY));
    alice.attached();
    bob.attached();
    for i in 0..3u8 {
        alice.mod_msg(0, &[i + 1]);
        alice.next_mod("the echo", |m| m.payload == [i + 1]);
    }
    // Bob's connection goes; he comes back: a new stream, the mod set first and the count behind the keyframe.
    let token = bob.token;
    drop(bob);
    let mut bob = Client::connect(&srv, "bob", token, Some(HELLO_ONLY));
    bob.attached();
    let snap = bob.next_mod("the snapshot", |m| m.payload.len() == 4);
    assert_eq!(count(&snap.payload), 3);
    assert!(matches!(bob.batches[0].first(), Some(GameMsg::Mods(_))));
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T19: the state goes into the `MODS` section of the save, and a gateway that comes back starts the mod from it.
#[test]
fn the_state_is_saved_in_the_mods_section_and_comes_back_after_a_restart() {
    let dir = temp_dir("mh-restart");
    let mods = install_both(&dir);
    let a = boot(&dir, Some(&mods), |_| {});
    let (mut alice, bob) = start_game(&a, &["hello"], Some(HELLO_ONLY), Some(HELLO_ONLY));
    alice.attached();
    for i in 0..2u8 {
        alice.mod_msg(0, &[i + 1]);
        alice.next_mod("the echo", |m| m.payload == [i + 1]);
    }
    admin_game(&a, 1, AdminCtl::Save);
    let save = wait_saved_count(&dir, 1, 2);
    let sec = save.mods.expect("the MODS section");
    assert!(sec.lock.contains("hello") && sec.lock.contains("modset_hash"));
    assert_eq!(sec.states.len(), 1);
    assert_eq!(sec.states[0].id, "hello");
    let token = alice.token;
    drop((alice, bob));
    a.stop();

    let b = boot(&dir, Some(&mods), |_| {});
    let mut alice = Client::connect(&b, "alice", token, Some(HELLO_ONLY));
    alice.attached();
    let snap = alice.next_mod("the snapshot", |m| m.payload.len() == 4);
    assert_eq!(count(&snap.payload), 2, "the mod started from the save");
    assert!(matches!(alice.batches[0].first(), Some(GameMsg::Mods(_))));
    // And it goes on counting from there.
    alice.mod_msg(0, &[9]);
    alice.next_mod("the echo", |m| m.payload == [9]);
    admin_game(&b, 1, AdminCtl::Save);
    wait_saved_count(&dir, 1, 3);
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T19: a worker that restarts loads an older save, and the mods are rewound with it; the seats get the snapshot again.
#[test]
fn a_worker_restart_rewinds_the_mods_to_the_save() {
    let dir = temp_dir("mh-rewind");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    let (mut alice, bob) = start_game(&srv, &["hello"], Some(HELLO_ONLY), Some(HELLO_ONLY));
    alice.attached();
    for i in 0..2u8 {
        alice.mod_msg(0, &[i + 1]);
        alice.next_mod("the echo", |m| m.payload == [i + 1]);
    }
    admin_game(&srv, 1, AdminCtl::Save);
    wait_saved_count(&dir, 1, 2);
    for i in 0..3u8 {
        alice.mod_msg(0, &[10 + i]);
        alice.next_mod("the echo", |m| m.payload == [10 + i]);
    }
    // The worker dies. The next one loads the save of count 2: the three messages since are gone, and the mod with them.
    let out = http(srv.admin, "POST", "/admin/games/1/kill-worker?signal=KILL");
    assert!(out.contains("200"), "{out}");
    let snap = alice.next_mod("the snapshot after the restart", |m| m.payload.len() == 4);
    assert_eq!(count(&snap.payload), 2, "rewound with the save");
    drop(bob);
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T18 (S6): oversize and unknown channels never reach the mod and cost strikes; the tenth strike ends the session; a
/// mod that panics is switched off and the game goes on.
#[test]
fn bad_messages_are_stopped_by_the_host_and_repeated_ones_end_the_session() {
    let dir = temp_dir("mh-limits");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    let (mut alice, mut bob) = start_game(&srv, &["hello", "flair"], Some(BOTH), Some(BOTH));
    alice.attached();
    bob.attached();
    // Oversize: nothing comes back, the mod's count does not move.
    alice.mod_msg(1, &[0; 65]);
    alice.mod_msg(1, b"ok");
    alice.next_mod("the echo of the good one", |m| m.payload == b"ok");
    assert!(
        alice.inbox.iter().all(|m| m.payload.len() != 65),
        "the oversize message never reached the mod"
    );
    // A panic in `flair` (index 0) switches it off for the game; `hello` (index 1) goes on.
    alice.mod_msg(0, b"boom");
    alice.mod_msg(0, b"again");
    alice.mod_msg(1, b"still");
    alice.next_mod("hello after the panic", |m| m.payload == b"still");
    assert!(alice.inbox.iter().all(|m| m.payload != b"again"));
    // Bob keeps sending to a mod that does not exist: from the tenth violation on he is disconnected.
    for _ in 0..12 {
        bob.mod_msg(9, b"x");
    }
    let _ = bob.closed();
    // Alice is still there.
    alice.mod_msg(1, b"alive");
    alice.next_mod("alive", |m| m.payload == b"alive");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Empty registry is byte-identical": a game without mods sends no `Mods`, drops mod messages without strikes, and
/// its saves have no `MODS` section.
#[test]
fn a_game_without_mods_sends_no_mod_message_and_writes_no_mods_section() {
    let dir = temp_dir("mh-none");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |_| {});
    let (mut alice, mut bob) = start_game(&srv, &[], None, None);
    alice.attached();
    bob.attached();
    for _ in 0..30 {
        alice.mod_msg(0, b"nobody listens");
    }
    // Still connected after far more than ten of them: no strikes in a game without mods.
    alice.send(&ClientMsg::Ping {
        seq: 1,
        client_ms: 7,
    });
    alice.until("a pong", |m| matches!(m, ServerMsg::Pong { seq: 1, .. }));
    admin_game(&srv, 1, AdminCtl::Save);
    let save = newest_save(&dir, 1);
    assert!(save.mods.is_none(), "{:?}", save.mods);
    assert!(save.info.settings.mods.is_empty());
    for c in [&alice, &bob] {
        assert!(c.mods.is_empty() && c.inbox.is_empty());
        assert!(
            c.batches
                .iter()
                .flatten()
                .all(|m| !matches!(m, GameMsg::Mods(_) | GameMsg::Mod(_)))
        );
    }
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A game whose mod has a server half the binary does not contain is refused at the start with a sentence.
#[test]
fn a_start_is_refused_when_the_binary_has_no_code_for_a_server_mod() {
    let dir = temp_dir("mh-nocode");
    let mods = install_both(&dir);
    let srv = boot(&dir, Some(&mods), |c| {
        c.mod_registry = Arc::new(HostRegistry::new())
    });
    let (mut alice, _bob) = start_game(&srv, &["hello"], Some(HELLO_ONLY), Some(HELLO_ONLY));
    let ServerMsg::LobbyError { text, .. } =
        alice.until("a refusal", |m| matches!(m, ServerMsg::LobbyError { .. }))
    else {
        unreachable!()
    };
    assert!(text.contains("`hello`") && text.contains("code"), "{text}");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- the whole-truth owners (T22)

const PROBE: &[(&str, &str)] = &[("probe", "1.0.0")];
const BLIND: &[(&str, &str)] = &[("blind", "1.0.0")];

fn install_probes(dir: &Path) -> PathBuf {
    install_caps(
        dir,
        "probe",
        "gameplay",
        "",
        r#""net.channel", "world.read""#,
    );
    install(dir, "blind", "gameplay", "")
}

/// What the probe mod says about a plot: owner (tag, value), class, turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Seen {
    owner: (u8, u8),
    class: u8,
    turn: u8,
}

const UNKNOWN: (u8, u8) = (3, 0);

impl Client {
    /// One question to the probe mod (index 0); `None`: the mod has no `world.read` or the plot is off the map.
    fn probe_once(&mut self, x: u8, y: u8) -> Option<Seen> {
        self.mod_msg(0, &[x, y]);
        let a = self.next_mod("a probe answer", |m| m.payload.len() == 5);
        match a.payload[..] {
            [t, v, class, turn, 0xAA] if t < 4 => Some(Seen {
                owner: (t, v),
                class,
                turn,
            }),
            _ => None,
        }
    }

    /// Asks until the table of owners is there (the first `Full` is on its way when a client attaches).
    fn probe(&mut self, x: u8, y: u8) -> Seen {
        for _ in 0..200 {
            let s = self.probe_once(x, y).expect("the probe answers");
            if s.owner != UNKNOWN {
                return s;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("the owners stay unknown");
    }

    /// An order of the seat, and its answer (`Results`) heard.
    fn order(&mut self, id: u32) {
        self.send(&ClientMsg::Cmd(protocol::Cmd {
            id,
            turn: 1,
            cmd: protocol::Command::CancelEndTurn,
        }));
        self.until("the answer to the order", |m| {
            matches!(m, ServerMsg::Game(b) if b.msgs.iter().any(|g|
                matches!(g, GameMsg::Results(r) if r.iter().any(|x| x.id == id))))
        });
    }
}

fn probe_game(
    dir: &Path,
    mode: &str,
    picks: &[&str],
    hello: Option<&[(&str, &str)]>,
) -> (TestServer, Client, Client) {
    let mods = install_probes(dir);
    let srv = boot_mode(dir, Some(&mods), mode, |_| {});
    let (mut alice, mut bob) = start_game(&srv, picks, hello, hello);
    alice.attached();
    bob.attached();
    (srv, alice, bob)
}

/// T22: a mod with `world.read` reads the owners the worker sent (the truth), the terrain classes of the basis, the
/// shape and the turn; an owner that changes between two batches is seen new in the second one, and never older than
/// the last answer the seat got.
#[test]
fn a_mod_reads_the_whole_truth_and_sees_an_owner_change_between_two_batches() {
    let dir = temp_dir("mh-owners");
    let (srv, mut alice, _bob) = probe_game(&dir, "echo turns=1 owners=1", &["probe"], Some(PROBE));
    let at = |c: &mut Client, i: u8| c.probe(i % 8, i / 8);
    // Plot 1 is player 0's (alice's seat), plot 2 is player 5's, plot 0 nobody's.
    assert_eq!(at(&mut alice, 0).owner, (0, 0));
    assert_eq!(at(&mut alice, 1).owner, (1, 0));
    assert_eq!(at(&mut alice, 2).owner, (2, 5));
    // The classes of the basis: land, water, lake, hills, mountain.
    let classes: Vec<u8> = (3..7).map(|i| at(&mut alice, i).class).collect();
    assert_eq!(classes, [3, 4, 1, 2]);
    assert_eq!(at(&mut alice, 0).class, 0);
    assert_eq!(alice.probe_once(8, 0), None, "off the map");
    // Batch one: plot 9 is nobody's. An order changes it (the Delta is ahead of the order's Results) ...
    let first = at(&mut alice, 9);
    assert_eq!(first.owner, (0, 0));
    alice.order(1);
    // ... batch two: no waiting, the new owner (player 1 = bob's seat) is there.
    assert_eq!(
        at(&mut alice, 9).owner,
        (1, 1),
        "never older than the Results"
    );
    alice.order(2);
    assert_eq!(at(&mut alice, 9).owner, (0, 0), "and back");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T22: a `Delta` that does not follow makes the table unknown (mods refuse) until the worker's `Full` arrives.
#[test]
fn a_lost_delta_makes_the_owners_unknown_until_the_worker_sends_the_table_again() {
    let dir = temp_dir("mh-owners-gap");
    let log = dir.join("wants.log");
    let mode = format!("echo turns=1 owners=1 gap=1 want_log={}", log.display());
    let (srv, mut alice, _bob) = probe_game(&dir, &mode, &["probe"], Some(PROBE));
    assert_eq!(alice.probe(1, 1).owner, (0, 0));
    alice.order(1);
    // The flip was lost; once the answer to the second ask is in, the truth (player 1) is back.
    assert_eq!(alice.probe(1, 1).owner, (1, 1));
    let wants = std::fs::read_to_string(&log).unwrap();
    assert_eq!(wants.lines().count(), 2, "the start and the gap: {wants:?}");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// T22: only a game with a mod that reads the world makes the worker keep the table: no `WantOwners` for a mod without
/// `world.read` (the mod is refused the facts) and none for a game without mods.
#[test]
fn the_worker_is_asked_for_owners_only_when_a_mod_reads_the_world() {
    // A mod without world.read.
    let dir = temp_dir("mh-owners-blind");
    let log = dir.join("wants.log");
    let mode = format!("echo turns=1 owners=1 want_log={}", log.display());
    let (srv, mut alice, _bob) = probe_game(&dir, &mode, &["blind"], Some(BLIND));
    assert_eq!(
        alice.probe_once(1, 0),
        None,
        "no facts without the capability"
    );
    alice.order(1);
    assert!(!log.exists(), "the worker was never asked");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
    // A game without mods.
    let dir = temp_dir("mh-owners-none");
    let log = dir.join("wants.log");
    let mode = format!("echo turns=1 owners=1 want_log={}", log.display());
    let (srv, mut alice, _bob) = probe_game(&dir, &mode, &[], None);
    alice.order(1);
    alice.send(&ClientMsg::Ping {
        seq: 1,
        client_ms: 7,
    });
    alice.until("a pong", |m| matches!(m, ServerMsg::Pong { seq: 1, .. }));
    assert!(!log.exists(), "the worker was never asked");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- API 1.1: the tick and the detach

const TICKER: &[(&str, &str)] = &[("ticker", "1.0.0")];

/// Moves the test clock 20 ms every 10 ms of real time while it lives (the tick runs on the host clock).
struct RunningClock {
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RunningClock {
    fn start(clock: Arc<TestClock>) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                clock.advance(20);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for RunningClock {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn tick_ms(m: &ModS2C) -> u64 {
    u64::from_le_bytes(m.payload[1..9].try_into().unwrap())
}

/// API 1.1: a mod with `tick_hz` is ticked by the game actor on the host clock and its answers reach every seat; a
/// seat whose connection goes is announced to the mod (`on_detach`), whose answer reaches the others.
#[test]
fn a_ticking_mod_broadcasts_on_the_host_clock_and_hears_a_seat_go() {
    let dir = temp_dir("mh-tick");
    let mods = install_rust(
        &dir,
        "ticker",
        "cosmetic",
        "client = \"optional\"",
        r#""net.channel""#,
        "tick_hz = 10",
    );
    let srv = boot(&dir, Some(&mods), |_| {});
    let (mut alice, mut bob) = start_game(&srv, &["ticker"], Some(TICKER), Some(TICKER));
    alice.attached();
    bob.attached();
    let clock = RunningClock::start(srv.clock.clone());
    let is_tick = |m: &ModS2C| m.payload.len() == 9 && m.payload[0] == b'T';
    let t1 = alice.next_mod("a tick", is_tick);
    let t2 = alice.next_mod("another tick", is_tick);
    assert_eq!((t1.mod_idx, t1.from), (0, None));
    // 10 Hz: at least 100 ms of the host clock between two ticks.
    assert!(
        tick_ms(&t2) >= tick_ms(&t1) + 100,
        "{} {}",
        tick_ms(&t1),
        tick_ms(&t2)
    );
    bob.next_mod("bob hears the ticks too", is_tick);
    // Bob's connection goes: the mod hears it and tells the others.
    drop(bob);
    alice.next_mod("bob's seat went", |m| m.payload == [b'D', 1]);
    drop(clock);
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- API 1.2: the terrain share

fn shares(c: &Client) -> Vec<protocol::TerrainShare> {
    c.batches
        .iter()
        .flatten()
        .filter_map(|m| match m {
            GameMsg::TerrainShare(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn basis(c: &Client) -> protocol::MapBasis {
    c.batches
        .iter()
        .flatten()
        .find_map(|m| match m {
            GameMsg::MapBasis(b) => Some(b.clone()),
            _ => None,
        })
        .expect("a map basis")
}

/// A mod shares terrain to one seat: that seat's stream gets one `TerrainShare` with the records of its map basis for
/// the new plots only, the other seat gets nothing; an unshare is a reset to that seat.
#[test]
fn a_terrain_share_goes_to_its_seat_only_with_the_records_of_the_basis() {
    let dir = temp_dir("mh-share");
    let mods = install_caps(
        &dir,
        "arena",
        "cosmetic",
        "client = \"optional\"",
        r#""net.channel", "world.share_terrain""#,
    );
    // `map=1`: the mock worker's map (8 x 4) and basis have water, a lake and a hill.
    let srv = boot_mode(&dir, Some(&mods), "echo turns=1 map=1", |_| {});
    let both: &[(&str, &str)] = &[("arena", "1.0.0")];
    let (mut alice, mut bob) = start_game(&srv, &["arena"], Some(both), Some(both));
    alice.attached();
    bob.attached();
    alice.await_mods();
    bob.await_mods();
    alice.mod_msg(0, b"mine");
    alice.next_mod("the answer", |m| m.payload == b"A");
    // Again: nothing new, no second share.
    alice.mod_msg(0, b"mine");
    alice.next_mod("the second answer", |m| m.payload == b"A");
    let got = shares(&alice);
    assert_eq!(got.len(), 1, "{got:?}");
    let b = basis(&alice);
    let want: Vec<(u32, protocol::PlotBasisView)> = [3u32, 4, 5]
        .iter()
        .map(|&i| (i, b.plots[i as usize].clone()))
        .collect();
    assert_eq!(want[2].1.plot_type, protocol::PlotType::Hill);
    assert_eq!((got[0].mod_idx, got[0].reset), (0, false));
    assert_eq!(got[0].plots, want);
    // Bob hears an answer of his own after alice's share went out: nothing of it reached him.
    bob.mod_msg(0, b"hi");
    bob.next_mod("his answer", |m| m.payload == b"A");
    assert!(shares(&bob).is_empty(), "{:?}", shares(&bob));
    // The unshare: a reset with no plots, to alice.
    alice.mod_msg(0, b"end");
    alice.next_mod("the third answer", |m| m.payload == b"A");
    let got = shares(&alice);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got[1].reset && got[1].plots.is_empty());
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
