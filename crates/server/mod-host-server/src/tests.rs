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
        });
        assert!(reg.problems().is_empty());
        let store = ModStore::from_installed(vec![
            server_manifest("echo", "", ""),
            server_manifest("script", ", \"world.read\"", "[order]\nafter = [\"echo\"]"),
            server_manifest("plain", "", ""),
            client_only_manifest("looks"),
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
