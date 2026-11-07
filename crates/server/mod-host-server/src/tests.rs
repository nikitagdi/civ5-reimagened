//! Host-level tests (S6): oversize, rate, repeated violation, panic isolation, state handling.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use mod_api::testing::MockFacts;
use mod_api::{Audience, LogLevel, Outgoing, Seat, ServerCtx, ServerMod, Verdict};
use mod_manifest::{ContentHash, Installed, Manifest, ModId};
use protocol::ModPick;

use crate::*;

fn id(s: &str) -> ModId {
    ModId::new(s).unwrap()
}

/// A cosmetic server mod: one channel `0` (64 bytes in, 256 out, 10/s, burst 3).
fn server_manifest(mod_id: &str, extra_caps: &str, extra_toml: &str) -> Installed {
    server_manifest_rust(mod_id, extra_caps, extra_toml, "")
}

/// [`server_manifest`] with more keys in `[rust]` (such as `tick_hz`).
fn server_manifest_rust(
    mod_id: &str,
    extra_caps: &str,
    extra_toml: &str,
    rust_extra: &str,
) -> Installed {
    let src = format!(
        r#"
[mod]
id = "{mod_id}"
name = "{mod_id}"
version = "1.0.0"
api = "^1"
license = "MIT"
sync = "cosmetic"
client = "optional"
parts = ["server-rs"]
{extra_toml}

[rust]
server = {{ crate = "{mod_id}-server", entry = "{mod_id}_server::register" }}
caps = ["net.channel"{extra_caps}]
state_max = 16
{rust_extra}

[[channels]]
id = 0
name = "main"
c2s_max = 64
s2c_max = 256
rate = "10/s"
burst = 3
"#
    );
    Installed {
        manifest: Manifest::parse(&src).unwrap_or_else(|e| panic!("{e}\n{src}")),
        content_hash: ContentHash([7; 32]),
    }
}

fn client_only_manifest(mod_id: &str) -> Installed {
    let src = format!(
        "[mod]\nid=\"{mod_id}\"\nname=\"{mod_id}\"\nversion=\"1.0.0\"\napi=\"^1\"\nlicense=\"MIT\"\nsync=\"client\"\nparts=[\"assets\"]\n"
    );
    Installed {
        manifest: Manifest::parse(&src).unwrap(),
        content_hash: ContentHash([8; 32]),
    }
}

fn pick(s: &str) -> ModPick {
    ModPick {
        id: s.into(),
        req: "*".into(),
    }
}

#[derive(Default)]
struct Counters {
    on_client: AtomicU32,
    persists: AtomicU32,
    saved: AtomicU32,
}

/// Echoes to the sender and the others; counts messages; state is the count.
struct Echo {
    n: u32,
    dirty: bool,
    c: Arc<Counters>,
}

impl ServerMod for Echo {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, saved: Option<&[u8]>) {
        self.n = saved
            .and_then(|b| b.try_into().ok())
            .map_or(0, u32::from_le_bytes);
    }
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        self.c.on_client.fetch_add(1, Ordering::Relaxed);
        self.n += 1;
        self.dirty = true;
        cx.log(LogLevel::Info, "echoed");
        vec![
            Outgoing::to_sender(ch, p.to_vec()),
            Outgoing::all_except(from, ch, p.to_vec()),
        ]
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        vec![Outgoing::to_sender(0, self.n.to_le_bytes().to_vec())]
    }
    fn persist(&self) -> Vec<u8> {
        self.c.persists.fetch_add(1, Ordering::Relaxed);
        self.n.to_le_bytes().to_vec()
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn mark_saved(&mut self) {
        self.dirty = false;
        self.c.saved.fetch_add(1, Ordering::Relaxed);
    }
}

/// Does what the payload says: `boom` panics, `big` returns an oversize reply, `bad` an undeclared channel,
/// `facts` reports the turn it sees (needs `world.read`), `cost` spends 2 cost tokens.
struct Scripted;

impl ServerMod for Scripted {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        match p {
            b"boom" => panic!("scripted failure"),
            b"big" => vec![Outgoing::to_sender(ch, vec![0; 300])],
            b"bad" => vec![Outgoing::to_sender(9, vec![1])],
            b"facts" => match cx.facts() {
                Ok(f) => vec![Outgoing::to_sender(ch, f.turn().to_le_bytes().to_vec())],
                Err(e) => vec![Outgoing::to_sender(ch, e.to_string().into_bytes())],
            },
            b"cost" => vec![Outgoing::to_sender(
                ch,
                vec![u8::from(cx.allow(from, ch, 2))],
            )],
            _ => Vec::new(),
        }
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        Vec::new()
    }
    fn on_turn(&mut self, _cx: &mut ServerCtx<'_>, turn: i32) {
        if turn == 13 {
            panic!("unlucky turn");
        }
    }
    fn persist(&self) -> Vec<u8> {
        vec![9; 40] // over state_max (16)
    }
}

/// API 1.1, the shooting side: ticks (20 Hz), sees the other bodies, deals hits. Payloads: `shoot` (5 damage on every
/// body it sees, answers `[S, count]`), `ghost` (hits on bodies that do not exist), `spam` (300 hits), `kill` (99
/// damage on the first body), `pub` (publishes 70 bodies of 20 boxes), `tickboom` (the next tick panics).
#[derive(Default)]
struct Gun {
    boom: bool,
}

fn one_box() -> mod_api::HitBox {
    mod_api::HitBox::new([0.0; 3], [1.0; 3], mod_api::HitZone::Head)
}

fn gun_hit(from: Seat, damage: u16) -> mod_api::Hit {
    mod_api::Hit {
        from,
        damage,
        zone: mod_api::HitZone::Head,
        kind: mod_api::HitKind::Bullet,
    }
}

impl ServerMod for Gun {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        use mod_api::{Body, BodyRef};
        let targets: Vec<BodyRef> = match cx.bodies() {
            Ok(b) => b.iter().map(Body::target).collect(),
            Err(e) => return vec![Outgoing::to_sender(ch, e.to_string().into_bytes())],
        };
        match p {
            b"shoot" => {
                for t in &targets {
                    cx.hit(t.clone(), gun_hit(from, 5)).unwrap();
                }
                vec![Outgoing::to_sender(ch, vec![b'S', targets.len() as u8])]
            }
            b"ghost" => {
                for (owner, id) in [("nobody", 1), ("steve", 999), ("echo", 0)] {
                    let owner = ModId::new(owner).unwrap();
                    cx.hit(BodyRef { owner, id }, gun_hit(from, 1)).unwrap();
                }
                Vec::new()
            }
            b"spam" => {
                for _ in 0..300 {
                    cx.hit(targets[0].clone(), gun_hit(from, 1)).unwrap();
                }
                Vec::new()
            }
            b"kill" => {
                cx.hit(targets[0].clone(), gun_hit(from, 99)).unwrap();
                Vec::new()
            }
            b"pub" => {
                let many = (0..70)
                    .map(|i| Body::new(from, i, vec![one_box(); 20]))
                    .collect();
                cx.set_bodies(many).unwrap();
                Vec::new()
            }
            b"tickboom" => {
                self.boom = true;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        Vec::new()
    }
    fn on_turn(&mut self, cx: &mut ServerCtx<'_>, _turn: i32) {
        let targets: Vec<_> = cx
            .bodies()
            .map(|b| b.iter().map(mod_api::Body::target).collect())
            .unwrap_or_default();
        for t in targets {
            cx.hit(t, gun_hit(Seat(0), 3)).unwrap();
        }
    }
    fn on_tick(&mut self, cx: &mut ServerCtx<'_>, now_ms: u64) -> Vec<Outgoing> {
        assert!(!self.boom, "tick failure");
        assert_eq!(cx.now_ms(), now_ms);
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

/// API 1.1, the target side: one body per attached seat (id 100 + seat), gone on detach. A hit is answered with
/// `[h, target, damage, len(from_mod)]` and hits back (which the host drops); 99 damage panics.
#[derive(Default)]
struct Steve {
    seats: Vec<Seat>,
}

impl Steve {
    fn publish(&self, cx: &mut ServerCtx<'_>) {
        let bodies = self
            .seats
            .iter()
            .map(|s| mod_api::Body::new(*s, 100 + s.0 as u32, vec![one_box()]))
            .collect();
        cx.set_bodies(bodies).unwrap();
    }
}

impl ServerMod for Steve {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, _: &mut ServerCtx<'_>, _: Seat, _: u8, _: &[u8]) -> Vec<Outgoing> {
        Vec::new()
    }
    fn on_attach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        self.seats.push(seat);
        self.publish(cx);
        Vec::new()
    }
    fn on_detach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        self.seats.retain(|s| *s != seat);
        self.publish(cx);
        Vec::new()
    }
    fn on_hit(
        &mut self,
        cx: &mut ServerCtx<'_>,
        from_mod: &ModId,
        target_id: u32,
        hit: &mod_api::Hit,
    ) -> Vec<Outgoing> {
        assert!(hit.damage != 99, "fatal hit");
        let back = mod_api::BodyRef {
            owner: from_mod.clone(),
            id: 0,
        };
        cx.hit(back, *hit).unwrap();
        vec![Outgoing::to_all(
            0,
            vec![
                b'h',
                target_id as u8,
                hit.damage as u8,
                from_mod.as_str().len() as u8,
            ],
        )]
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// API 1.2, the terrain share: on attach shares plots 9 and 10 to the seat; payloads `share` (9, 10, 17, 18, 9 and
/// 999, off the map, to the sender), `other` (plots 1 and 2 to seat 5), `end` (unshares the sender), `big` (300 plots),
/// `boom` (shares, then panics). Answers the error of a call without the capability.
struct Terra;

impl ServerMod for Terra {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        let r = match p {
            b"share" => cx.share_terrain(from, &[9, 10, 17, 18, 9, 999]),
            b"other" => cx.share_terrain(Seat(5), &[1, 2]),
            b"end" => cx.unshare_terrain(from),
            b"big" => cx.share_terrain(from, &(0..300).map(|i| i % 64).collect::<Vec<_>>()),
            b"boom" => {
                let _ = cx.share_terrain(from, &[3]);
                panic!("scripted failure after a share");
            }
            _ => Ok(()),
        };
        match r {
            Ok(()) => Vec::new(),
            Err(e) => vec![Outgoing::to_sender(ch, e.to_string().into_bytes())],
        }
    }
    fn on_attach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        let _ = cx.share_terrain(seat, &[9, 10]);
        Vec::new()
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

struct Rig {
    reg: HostRegistry,
    store: ModStore,
    counters: Arc<Counters>,
    facts: MockFacts,
    now_ms: u64,
}

impl Rig {
    fn new() -> Self {
        let counters = Arc::new(Counters::default());
        let c = counters.clone();
        let mut reg = HostRegistry::new();
        reg.register(|r| {
            r.add("echo", move || {
                Box::new(Echo {
                    n: 0,
                    dirty: true,
                    c: c.clone(),
                })
            });
            r.add("script", || Box::new(Scripted));
            r.add("plain", || Box::new(Scripted));
            r.add("gun", || Box::new(Gun::default()));
            r.add("steve", || Box::new(Steve::default()));
            r.add("nocap", || Box::new(Gun::default()));
            r.add("slow", || Box::new(Gun::default()));
            r.add("terra", || Box::new(Terra));
            r.add("bare", || Box::new(Terra));
        });
        assert!(reg.problems().is_empty());
        let bodies = ", \"world.bodies\"";
        let store = ModStore::from_installed(vec![
            server_manifest("echo", "", ""),
            server_manifest("script", ", \"world.read\"", "[order]\nafter = [\"echo\"]"),
            server_manifest("plain", "", ""),
            client_only_manifest("looks"),
            server_manifest_rust("gun", bodies, "", "tick_hz = 20"),
            server_manifest("steve", bodies, ""),
            server_manifest("nocap", "", ""),
            server_manifest_rust("slow", bodies, "", "tick_hz = 3"),
            server_manifest("terra", ", \"world.share_terrain\"", ""),
            server_manifest("bare", "", ""),
        ]);
        Rig {
            reg,
            store,
            counters,
            facts: MockFacts::new(8, 8).with_turn(41),
            now_ms: 0,
        }
    }

    fn game(&self, picks: &[&str]) -> GameMods {
        let picks: Vec<ModPick> = picks.iter().map(|p| pick(p)).collect();
        let set = self.store.freeze(&picks).unwrap().set;
        self.reg.instantiate(&self.store, &set).unwrap()
    }

    fn env(&self) -> CallEnv<'_> {
        CallEnv {
            facts: &self.facts,
            now_ms: self.now_ms,
        }
    }
}

#[test]
fn empty_set_is_empty_and_inert() {
    let rig = Rig::new();
    let mut g = rig.game(&[]);
    assert!(g.is_empty() && g.mod_count() == 0);
    g.start(rig.env(), &[]);
    assert!(g.on_attach(rig.env(), Seat(0)).is_empty());
    g.on_turn(rig.env(), 1);
    assert!(g.persist(rig.env()).is_empty());
    assert!(g.take_logs().is_empty());
    let r = g.on_client(rig.env(), Seat(0), 0, 0, b"x");
    assert_eq!(r.dropped, Some(DropReason::NoSuchMod));
    assert!(r.outputs.is_empty());
}

#[test]
fn instantiate_orders_by_load_and_skips_client_only_halves() {
    let rig = Rig::new();
    let g = rig.game(&["script", "looks", "echo"]);
    // Load order: echo, looks (by id, no edges), script (after echo) -> echo, looks, script.
    assert_eq!(g.mod_count(), 3);
    let st = g.statuses();
    assert_eq!(st.len(), 2, "the client-only mod has no instance");
    assert_eq!(st[0].0, id("echo"));
    assert_eq!(st[1].0, id("script"));
    assert!(st.iter().all(|(_, s)| *s == ModStatus::Active));
    assert!(!g.is_empty());
}

#[test]
fn instantiate_errors_read_as_sentences() {
    let rig = Rig::new();
    let set = rig.store.freeze(&[pick("echo")]).unwrap().set;

    // Not installed (an empty store).
    let e = rig.reg.instantiate(&ModStore::empty(), &set).err().unwrap();
    assert!(matches!(e, InstantiateError::NotInstalled { .. }));
    assert!(
        e.to_string().contains("`echo` 1.0.0 is not installed"),
        "{e}"
    );

    // Content changed on disk.
    let mut changed = server_manifest("echo", "", "");
    changed.content_hash = ContentHash([9; 32]);
    let e = rig
        .reg
        .instantiate(&ModStore::from_installed(vec![changed]), &set)
        .err()
        .unwrap();
    assert!(matches!(e, InstantiateError::Changed { .. }), "{e}");

    // The binary has no code for it.
    let e = HostRegistry::new()
        .instantiate(&rig.store, &set)
        .err()
        .unwrap();
    assert!(matches!(e, InstantiateError::NoCode { .. }));
    assert!(e.to_string().contains("does not contain its code"), "{e}");
}

#[test]
fn a_message_reaches_the_mod_and_answers_are_routed() {
    let mut rig = Rig::new();
    let mut g = rig.game(&["echo"]);
    g.start(rig.env(), &[]);
    rig.now_ms = 5;
    let r = g.on_client(rig.env(), Seat(2), 0, 0, b"hi");
    assert_eq!(r.dropped, None);
    assert!(!r.disconnect);
    assert_eq!(r.outputs.len(), 2);
    assert_eq!(r.outputs[0].to, Audience::Sender);
    assert_eq!(r.outputs[1].to, Audience::AllExcept(Seat(2)));
    assert!(
        r.outputs
            .iter()
            .all(|o| o.mod_idx == 0 && o.from == Some(Seat(2)))
    );
    let w = r.outputs[0].to_wire();
    assert_eq!((w.mod_idx, w.ch, w.from), (0, 0, Some(2)));
    assert_eq!(w.payload, b"hi");
    let logs = g.take_logs();
    assert_eq!(logs.len(), 1);
    assert_eq!(
        (logs[0].mod_id.clone(), logs[0].level),
        (id("echo"), LogLevel::Info)
    );
    assert!(g.take_logs().is_empty(), "taken once");
}

#[test]
fn attach_pages_come_in_load_order_without_a_sender() {
    let rig = Rig::new();
    let mut g = rig.game(&["echo", "script"]);
    g.start(rig.env(), &[(id("echo"), 7u32.to_le_bytes().to_vec())]);
    let out = g.on_attach(rig.env(), Seat(1));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].payload, 7u32.to_le_bytes());
    assert_eq!(out[0].from, None);
}

#[test]
fn oversize_payload_never_reaches_the_mod() {
    let rig = Rig::new();
    let mut g = rig.game(&["echo"]);
    g.start(rig.env(), &[]);
    let r = g.on_client(rig.env(), Seat(0), 0, 0, &[0; 65]);
    assert_eq!(
        r.dropped,
        Some(DropReason::Limit(Verdict::TooLarge { len: 65, max: 64 }))
    );
    assert_eq!(rig.counters.on_client.load(Ordering::Relaxed), 0);
    // 64 bytes pass; an undeclared channel does not.
    assert_eq!(
        g.on_client(rig.env(), Seat(0), 0, 0, &[0; 64]).dropped,
        None
    );
    assert_eq!(
        g.on_client(rig.env(), Seat(0), 0, 5, b"x").dropped,
        Some(DropReason::Limit(Verdict::UnknownChannel))
    );
    assert_eq!(rig.counters.on_client.load(Ordering::Relaxed), 1);
}

#[test]
fn rate_limit_is_per_seat_and_refills() {
    let mut rig = Rig::new();
    let mut g = rig.game(&["echo"]);
    g.start(rig.env(), &[]);
    for _ in 0..3 {
        assert_eq!(g.on_client(rig.env(), Seat(0), 0, 0, b"x").dropped, None);
    }
    assert_eq!(
        g.on_client(rig.env(), Seat(0), 0, 0, b"x").dropped,
        Some(DropReason::Limit(Verdict::RateLimited))
    );
    // Another seat has its own bucket.
    assert_eq!(g.on_client(rig.env(), Seat(1), 0, 0, b"x").dropped, None);
    // 10/s: 100 ms later one token is back.
    rig.now_ms = 100;
    assert_eq!(g.on_client(rig.env(), Seat(0), 0, 0, b"x").dropped, None);
    assert_eq!(rig.counters.on_client.load(Ordering::Relaxed), 5);
}

#[test]
fn repeated_violations_raise_disconnect_once_per_run() {
    let rig = Rig::new();
    let mut g = rig.game(&["echo"]);
    g.start(rig.env(), &[]);
    let big = [0u8; 100];
    for i in 1..MAX_STRIKES {
        let r = g.on_client(rig.env(), Seat(3), 0, 0, &big);
        assert!(!r.disconnect, "violation {i}");
    }
    let r = g.on_client(rig.env(), Seat(3), 0, 0, &big);
    assert!(r.disconnect);
    // The count started again.
    assert!(!g.on_client(rig.env(), Seat(3), 0, 0, &big).disconnect);
    // Another seat is not affected; valid traffic does not count.
    assert!(!g.on_client(rig.env(), Seat(4), 0, 0, &big).disconnect);
    assert_eq!(g.on_client(rig.env(), Seat(5), 0, 0, b"ok").dropped, None);
}

#[test]
fn messages_to_missing_mods_are_violations_too() {
    let rig = Rig::new();
    let mut g = rig.game(&["looks", "echo"]); // looks is index 1? load order by id: echo, looks
    g.start(rig.env(), &[]);
    // Index 1 is the client-only mod: no server half.
    let r = g.on_client(rig.env(), Seat(0), 1, 0, b"x");
    assert_eq!(r.dropped, Some(DropReason::NoSuchMod));
    for _ in 1..MAX_STRIKES - 1 {
        assert!(!g.on_client(rig.env(), Seat(0), 77, 0, b"x").disconnect);
    }
    assert!(g.on_client(rig.env(), Seat(0), 77, 0, b"x").disconnect);
    g.forget_seat(Seat(0));
}

#[test]
fn filtered_outgoing_is_dropped_and_logged() {
    let rig = Rig::new();
    let mut g = rig.game(&["script"]);
    g.start(rig.env(), &[]);
    let r = g.on_client(rig.env(), Seat(0), 0, 0, b"big");
    assert!(r.outputs.is_empty());
    let r = g.on_client(rig.env(), Seat(0), 0, 0, b"bad");
    assert!(r.outputs.is_empty());
    let logs = g.take_logs();
    assert_eq!(logs.len(), 2);
    assert!(logs.iter().all(|l| l.level == LogLevel::Warn));
    assert!(logs[0].msg.contains("TooLarge"), "{:?}", logs[0]);
    assert!(logs[1].msg.contains("UnknownChannel"), "{:?}", logs[1]);
}

#[test]
fn facts_and_cost_bucket_reach_the_mod_through_the_context() {
    let mut rig = Rig::new();
    let mut g = rig.game(&["script"]);
    g.start(rig.env(), &[]);
    let r = g.on_client(rig.env(), Seat(0), 0, 0, b"facts");
    assert_eq!(r.outputs[0].payload, 41i32.to_le_bytes());
    // The cost bucket (burst 3): 2 tokens twice is one yes, one no.
    rig.now_ms = 1;
    let yes = g.on_client(rig.env(), Seat(0), 0, 0, b"cost").outputs[0].payload[0];
    let no = g.on_client(rig.env(), Seat(0), 0, 0, b"cost").outputs[0].payload[0];
    assert_eq!((yes, no), (1, 0));

    // Without world.read the same request is refused by the context, not by luck.
    let mut g2 = rig.game(&["plain"]);
    g2.start(rig.env(), &[]);
    let r = g2.on_client(rig.env(), Seat(0), 0, 0, b"facts");
    let text = String::from_utf8(r.outputs[0].payload.clone()).unwrap();
    assert!(text.contains("world.read"), "{text}");
}

#[test]
fn persist_keeps_clean_state_and_oversize_state_is_refused() {
    let rig = Rig::new();
    let mut g = rig.game(&["echo", "script"]);
    g.start(rig.env(), &[(id("echo"), 3u32.to_le_bytes().to_vec())]);
    // Restored and never dirtied: the saved bytes come back without calling persist() on the mod... it is dirty
    // by default (dirty = true in the factory), so the first persist() runs once, then it is marked saved.
    let s1 = g.persist(rig.env());
    assert_eq!(s1.len(), 2);
    assert_eq!(s1[0], (id("echo"), 3u32.to_le_bytes().to_vec()));
    assert_eq!(rig.counters.persists.load(Ordering::Relaxed), 1);
    assert_eq!(rig.counters.saved.load(Ordering::Relaxed), 1);
    // Clean now: no second persist() call.
    let s2 = g.persist(rig.env());
    assert_eq!(s2, s1);
    assert_eq!(rig.counters.persists.load(Ordering::Relaxed), 1);
    // The scripted mod returned 40 bytes against state_max 16: refused, empty previous state, logged.
    assert!(s1[1].1.is_empty());
    assert!(
        g.take_logs()
            .iter()
            .any(|l| l.level == LogLevel::Error && l.msg.contains("state_max"))
    );
    // A new message dirties it again.
    g.on_client(rig.env(), Seat(0), 0, 0, b"x");
    let s3 = g.persist(rig.env());
    assert_eq!(s3[0].1, 4u32.to_le_bytes());
}

#[cfg(panic = "unwind")]
mod panics {
    use super::*;

    #[test]
    fn a_panic_disables_only_that_mod() {
        let rig = Rig::new();
        let mut g = rig.game(&["echo", "script"]);
        g.start(rig.env(), &[]);
        g.on_client(rig.env(), Seat(0), 0, 0, b"warm"); // echo state = 1

        let r = g.on_client(rig.env(), Seat(0), 1, 0, b"boom");
        assert!(r.outputs.is_empty() && r.dropped.is_none() && !r.disconnect);
        let st = g.statuses();
        assert_eq!(st[0].1, ModStatus::Active);
        assert_eq!(
            st[1].1,
            ModStatus::Disabled {
                reason: "scripted failure".into()
            }
        );
        let logs = g.take_logs();
        assert!(
            logs.iter().any(|l| l.mod_id == id("script")
                && l.level == LogLevel::Error
                && l.msg.contains("scripted failure")),
            "{logs:?}"
        );

        // The disabled mod drops everything quietly; no strikes however long the client keeps sending.
        for _ in 0..MAX_STRIKES * 2 {
            let r = g.on_client(rig.env(), Seat(0), 1, 0, b"x");
            assert_eq!(r.dropped, Some(DropReason::Disabled));
            assert!(!r.disconnect);
        }
        // The other mod and the game go on.
        let r = g.on_client(rig.env(), Seat(1), 0, 0, b"still");
        assert_eq!(r.outputs.len(), 2);
        assert!(g.on_attach(rig.env(), Seat(1)).len() == 1);
        g.on_turn(rig.env(), 42);
        // Saving does not call the dead mod and keeps its last state.
        let saved = g.persist(rig.env());
        assert_eq!(saved.len(), 2);
        assert_eq!(saved[0].1, 2u32.to_le_bytes());
    }

    #[test]
    fn a_panic_in_on_turn_and_start_is_isolated_too() {
        let rig = Rig::new();
        let mut g = rig.game(&["echo", "script"]);
        g.start(rig.env(), &[]);
        g.on_turn(rig.env(), 13);
        assert!(matches!(g.statuses()[1].1, ModStatus::Disabled { .. }));
        assert_eq!(g.statuses()[0].1, ModStatus::Active);
    }

    #[test]
    fn isolation_flag_matches_the_build() {
        const { assert!(PANIC_ISOLATION) };
    }
}

#[cfg(panic = "abort")]
#[test]
fn isolation_flag_matches_the_build_abort() {
    // Under `panic = "abort"` a mod panic would end the process: the flag lets the host say so at start-up.
    const { assert!(!PANIC_ISOLATION) };
}

// ---------------------------------------------------------------- API 1.1: tick, detach, bodies, hits

mod api_1_1 {
    use mod_api::{MAX_BODIES, MAX_BOXES_PER_BODY, MAX_HITS_PER_CALL};

    use super::*;

    fn payloads(out: &[ModOutput]) -> Vec<Vec<u8>> {
        out.iter().map(|o| o.payload.clone()).collect()
    }

    fn tick_payload(now: u64) -> Vec<u8> {
        let mut p = vec![b'T'];
        p.extend_from_slice(&now.to_le_bytes());
        p
    }

    fn logs_of(g: &mut GameMods, mod_id: &str) -> Vec<String> {
        g.take_logs()
            .into_iter()
            .filter(|l| l.mod_id == id(mod_id))
            .map(|l| l.msg)
            .collect()
    }

    #[test]
    fn the_tick_follows_tick_hz_on_the_host_clock_and_skips_what_it_missed() {
        let mut rig = Rig::new();
        let mut g = rig.game(&["gun", "slow", "echo"]);
        assert_eq!(g.next_tick_ms(), None, "nothing is scheduled before start");
        rig.now_ms = 1000;
        g.start(rig.env(), &[]);
        // gun is 20 Hz (every 50 ms), slow 3 Hz (333, 666, 1000 ms after the start); echo does not tick.
        assert_eq!(g.next_tick_ms(), Some(1050));
        rig.now_ms = 1049;
        assert!(g.on_tick(rig.env()).is_empty(), "not due yet");
        rig.now_ms = 1050;
        let out = g.on_tick(rig.env());
        assert_eq!(payloads(&out), [tick_payload(1050)]);
        assert_eq!(
            (out[0].mod_idx, out[0].from),
            (1, None),
            "gun is index 1 (echo, gun, slow)"
        );
        assert_eq!(out[0].to, Audience::All);
        assert_eq!(g.next_tick_ms(), Some(1100));
        // A stall: one tick, not ten, and the schedule starts again from now.
        rig.now_ms = 1340;
        let out = g.on_tick(rig.env());
        assert_eq!(out.len(), 2, "gun and slow (due at 1333) once each");
        assert_eq!(out[0].payload, tick_payload(1340));
        assert_eq!(out[1].mod_idx, 2);
        assert_eq!(g.next_tick_ms(), Some(1390));
        // slow keeps its own beat: 1000 + 666.
        rig.now_ms = 1390;
        assert_eq!(g.on_tick(rig.env()).len(), 1);
        rig.now_ms = 1666;
        let out = g.on_tick(rig.env());
        assert_eq!(out.iter().filter(|o| o.mod_idx == 2).count(), 1);
    }

    #[test]
    fn a_game_without_ticking_mods_never_wakes() {
        let rig = Rig::new();
        let mut g = rig.game(&["echo", "steve"]);
        g.start(rig.env(), &[]);
        assert_eq!(g.next_tick_ms(), None);
        assert!(g.on_tick(rig.env()).is_empty());
    }

    #[test]
    fn detach_reaches_every_mod_in_load_order() {
        let rig = Rig::new();
        let mut g = rig.game(&["steve", "gun", "echo"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        assert_eq!(g.bodies()[2].1.len(), 1, "steve has the body of seat 1");
        let out = g.on_detach(rig.env(), Seat(1));
        assert_eq!(payloads(&out), [vec![b'D', 1]]);
        assert_eq!(out[0].from, None);
        assert!(g.bodies()[2].1.is_empty(), "steve took the body away");
    }

    #[test]
    fn a_mod_sees_the_other_bodies_and_its_hits_reach_the_owner() {
        let rig = Rig::new();
        let mut g = rig.game(&["gun", "steve"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        g.on_attach(rig.env(), Seat(2));
        let steve = &g.bodies()[1];
        assert_eq!(steve.0, id("steve"));
        assert!(steve.1.iter().all(|b| b.owner == id("steve")));
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"shoot");
        // The shooter's answer first, then the owner's answers to the hits, in order and without a sender.
        assert_eq!(
            payloads(&r.outputs),
            [vec![b'S', 2], vec![b'h', 101, 5, 3], vec![b'h', 102, 5, 3]]
        );
        assert_eq!(r.outputs[0].from, Some(Seat(0)));
        assert!(
            r.outputs[1..]
                .iter()
                .all(|o| o.mod_idx == 1 && o.from.is_none())
        );
        // Steve's hit back from inside on_hit was dropped, once per call, with a line.
        let logs = logs_of(&mut g, "steve");
        assert_eq!(
            logs.iter().filter(|m| m.contains("inside on_hit")).count(),
            2,
            "{logs:?}"
        );
        // A mod does not see its own bodies; the others see a change from their next call on.
        g.on_client(rig.env(), Seat(0), 0, 0, b"pub");
        assert_eq!(g.bodies()[0].1.len(), MAX_BODIES);
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"shoot");
        assert_eq!(r.outputs[0].payload, [b'S', 2]);
        g.on_detach(rig.env(), Seat(1));
        // (Another seat: seat 0 has used up its burst of 3.)
        let r = g.on_client(rig.env(), Seat(5), 0, 0, b"shoot");
        assert_eq!(r.outputs[0].payload, [b'S', 1]);
    }

    #[test]
    fn published_bodies_are_cut_to_the_limits_with_a_warning() {
        let rig = Rig::new();
        let mut g = rig.game(&["gun"]);
        g.start(rig.env(), &[]);
        g.on_client(rig.env(), Seat(3), 0, 0, b"pub");
        let b = g.bodies()[0].1;
        assert_eq!(b.len(), MAX_BODIES);
        assert!(
            b.iter()
                .all(|b| b.boxes.len() == MAX_BOXES_PER_BODY && b.owner == id("gun"))
        );
        let logs = logs_of(&mut g, "gun");
        assert!(logs.iter().any(|m| m.contains("cut off")), "{logs:?}");
    }

    #[test]
    fn hits_on_unknown_owners_or_bodies_are_dropped_with_a_line() {
        let rig = Rig::new();
        let mut g = rig.game(&["gun", "steve", "echo"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        let r = g.on_client(rig.env(), Seat(0), 1, 0, b"ghost");
        assert!(r.outputs.is_empty());
        let logs = logs_of(&mut g, "gun");
        assert_eq!(logs.len(), 3, "{logs:?}");
        assert!(logs[0].contains("`nobody`") && logs[0].contains("no such mod"));
        assert!(logs[1].contains("`steve`") && logs[1].contains("no such body"));
        assert!(logs[2].contains("`echo`") && logs[2].contains("no such body"));
    }

    #[test]
    fn at_most_256_hits_a_call() {
        let rig = Rig::new();
        let mut g = rig.game(&["gun", "steve"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"spam");
        assert_eq!(r.outputs.len(), MAX_HITS_PER_CALL);
        let logs = logs_of(&mut g, "gun");
        assert!(
            logs.iter().any(|m| m.contains("44 hits over 256")),
            "{logs:?}"
        );
    }

    #[test]
    fn without_world_bodies_a_mod_neither_sees_nor_hits() {
        let rig = Rig::new();
        let mut g = rig.game(&["nocap", "steve"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"shoot");
        assert_eq!(r.outputs.len(), 1);
        let text = String::from_utf8(r.outputs[0].payload.clone()).unwrap();
        assert!(text.contains("world.bodies"), "{text}");
    }

    #[test]
    fn hits_queued_in_on_turn_are_delivered() {
        let rig = Rig::new();
        let mut g = rig.game(&["gun", "steve"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(4));
        let out = g.on_turn(rig.env(), 2);
        assert_eq!(payloads(&out), [vec![b'h', 104, 3, 3]]);
    }

    #[test]
    fn tick_answers_go_through_the_output_filter() {
        let mut rig = Rig::new();
        let mut g = rig.game(&["gun"]);
        g.start(rig.env(), &[]);
        rig.now_ms = 50;
        // `T` + 8 bytes is well under s2c_max 256: kept. (The filter itself is the one on_client uses.)
        assert_eq!(g.on_tick(rig.env()).len(), 1);
        assert!(g.take_logs().is_empty());
    }

    #[cfg(panic = "unwind")]
    #[test]
    fn a_panic_in_on_tick_or_on_hit_switches_only_that_mod_off() {
        let mut rig = Rig::new();
        let mut g = rig.game(&["gun", "steve"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        // on_hit panics: steve is off, its bodies are gone, later hits on it are dropped.
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"kill");
        assert!(r.outputs.is_empty());
        assert!(matches!(g.statuses()[1].1, ModStatus::Disabled { .. }));
        assert!(g.bodies()[1].1.is_empty());
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"shoot");
        assert_eq!(
            r.outputs[0].payload,
            [b'S', 0],
            "the bodies of a mod that is off are gone"
        );
        // on_tick panics: gun is off and nothing ticks any more.
        g.on_client(rig.env(), Seat(0), 0, 0, b"tickboom");
        rig.now_ms = 50;
        assert!(g.on_tick(rig.env()).is_empty());
        assert!(matches!(g.statuses()[0].1, ModStatus::Disabled { .. }));
        assert_eq!(g.next_tick_ms(), None);
        let logs: Vec<_> = g.take_logs().into_iter().map(|l| l.msg).collect();
        assert!(
            logs.iter().any(|m| m.contains("panicked in on_hit")),
            "{logs:?}"
        );
        assert!(
            logs.iter().any(|m| m.contains("panicked in on_tick")),
            "{logs:?}"
        );
    }

    #[cfg(panic = "unwind")]
    #[test]
    fn a_hit_on_a_mod_that_is_off_is_dropped_as_such() {
        let rig = Rig::new();
        let mut g = rig.game(&["gun", "steve"]);
        g.start(rig.env(), &[]);
        g.on_attach(rig.env(), Seat(1));
        // A fatal hit switches steve off; `ghost` then aims at steve again (body 999): the mod is off.
        g.on_client(rig.env(), Seat(0), 0, 0, b"kill");
        let _ = g.take_logs();
        let r = g.on_client(rig.env(), Seat(0), 0, 0, b"ghost");
        assert!(r.outputs.is_empty());
        let logs = logs_of(&mut g, "gun");
        assert!(
            logs.iter()
                .any(|m| m.contains("`steve`") && m.contains("off")),
            "{logs:?}"
        );
    }
}

// ------------------------------------------------------------------ API 1.2: the terrain share

fn plots_of(shares: &[(u16, mod_api::ShareOut)]) -> Vec<(u16, i8, bool, Vec<u32>)> {
    shares
        .iter()
        .map(|(i, s)| (*i, s.seat.0, s.reset, s.plots.clone()))
        .collect()
}

#[test]
fn a_share_goes_to_its_seat_once_without_duplicates_or_plots_off_the_map() {
    let mut rig = Rig::new();
    let mut g = rig.game(&["echo", "terra"]);
    g.start(rig.env(), &[]);
    let terra = 1u16;
    // The attach of seat 1 shares 9 and 10 to it.
    g.on_attach(rig.env(), Seat(1));
    assert_eq!(
        plots_of(&g.take_terrain_shares()),
        [(terra, 1, false, vec![9, 10])]
    );
    assert!(g.take_terrain_shares().is_empty(), "taken once");
    // Only the new plots go, once each, none off the 8 x 8 map.
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(1), terra, 0, b"share");
    assert_eq!(
        plots_of(&g.take_terrain_shares()),
        [(terra, 1, false, vec![17, 18])]
    );
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(1), terra, 0, b"share");
    assert!(g.take_terrain_shares().is_empty(), "nothing new");
    assert_eq!(g.shared_terrain(terra, Seat(1)), [9, 10, 17, 18]);
    // A share names its seat; another seat's share is its own.
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(1), terra, 0, b"other");
    assert_eq!(
        plots_of(&g.take_terrain_shares()),
        [(terra, 5, false, vec![1, 2])]
    );
    assert_eq!(g.shared_terrain(terra, Seat(1)), [9, 10, 17, 18]);
    // Unshare: a reset for the sender only.
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(1), terra, 0, b"end");
    assert_eq!(
        plots_of(&g.take_terrain_shares()),
        [(terra, 1, true, vec![])]
    );
    assert!(g.shared_terrain(terra, Seat(1)).is_empty());
    assert_eq!(g.shared_terrain(terra, Seat(5)), [1, 2]);
    // Ordinary messages are unchanged by it.
    rig.now_ms += 1000;
    let r = g.on_client(rig.env(), Seat(1), 0, 0, b"hi");
    assert_eq!(r.outputs.len(), 2);
    assert!(g.take_terrain_shares().is_empty());
}

#[test]
fn a_share_needs_the_capability_and_a_call_shares_at_most_the_limit() {
    let mut rig = Rig::new();
    let mut g = rig.game(&["bare", "terra"]);
    g.start(rig.env(), &[]);
    let (bare, terra) = (0u16, 1u16);
    rig.now_ms += 1000;
    let r = g.on_client(rig.env(), Seat(1), bare, 0, b"share");
    assert!(
        String::from_utf8_lossy(&r.outputs[0].payload).contains("world.share_terrain"),
        "{:?}",
        r.outputs
    );
    // The attach of the mod without the capability shared nothing either.
    g.on_attach(rig.env(), Seat(2));
    let all = plots_of(&g.take_terrain_shares());
    assert!(all.iter().all(|(i, ..)| *i == terra), "{all:?}");
    // 300 plots in one call: the first 256 are taken (64 distinct ones on this map), the rest is logged.
    let _ = g.take_logs();
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(3), terra, 0, b"big");
    let s = g.take_terrain_shares();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].1.plots.len(), 64);
    let logs = g.take_logs();
    assert!(
        logs.iter()
            .any(|l| l.level == LogLevel::Warn && l.msg.contains("44 plots over 256")),
        "{logs:?}"
    );
}

#[test]
fn a_detach_forgets_the_share_and_a_panic_sends_none_of_its_call() {
    let mut rig = Rig::new();
    let mut g = rig.game(&["terra"]);
    g.start(rig.env(), &[]);
    g.on_attach(rig.env(), Seat(1));
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(1), 0, 0, b"share");
    // Not taken yet when the seat goes: dropped with what the host remembers.
    g.on_detach(rig.env(), Seat(1));
    assert!(g.take_terrain_shares().is_empty());
    assert!(g.shared_terrain(0, Seat(1)).is_empty());
    // It comes back: the attach shares whole again.
    g.on_attach(rig.env(), Seat(1));
    assert_eq!(
        plots_of(&g.take_terrain_shares()),
        [(0, 1, false, vec![9, 10])]
    );
    // A fresh attach without a detach starts over too (the client's fresh stream has no share).
    g.on_attach(rig.env(), Seat(1));
    assert_eq!(
        plots_of(&g.take_terrain_shares()),
        [(0, 1, false, vec![9, 10])]
    );
    // A panic: the share of that call is not sent, the mod is off.
    rig.now_ms += 1000;
    g.on_client(rig.env(), Seat(1), 0, 0, b"boom");
    assert!(g.take_terrain_shares().is_empty());
    assert!(matches!(g.statuses()[0].1, ModStatus::Disabled { .. }));
    // The seat leaves the game: forgotten.
    g.forget_seat(Seat(1));
    assert!(g.shared_terrain(0, Seat(1)).is_empty());
}
