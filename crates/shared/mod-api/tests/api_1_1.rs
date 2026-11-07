//! API 1.1: the clock and the tick, `on_detach`, bodies and hits, through the mock host.

use mod_api::testing::{MockCtx, MockFacts};
use mod_api::{
    Body, BodyRef, Cap, CapDenied, Caps, Hit, HitBox, HitKind, HitQueue, HitZone, LogLevel,
    MAX_BODIES, MAX_BOXES_PER_BODY, MAX_HITS_PER_CALL, MOD_API_VERSION, Outgoing, Seat, ServerCtx,
    ServerMod, Version, prepare_bodies,
};
use mod_manifest::{Channel, ModId};

fn id(s: &str) -> ModId {
    ModId::new(s).unwrap()
}

fn chan() -> Channel {
    Channel {
        id: 0,
        name: "c".into(),
        c2s_max: 64,
        s2c_max: 64,
        rate_per_s: 10,
        burst: 5,
    }
}

fn caps(extra: &[Cap]) -> Caps {
    let mut v = vec![Cap::NetChannel];
    v.extend_from_slice(extra);
    Caps::of(&v)
}

fn unit_box(zone: HitZone) -> HitBox {
    HitBox::new([0.0, 1.0, 0.0], [0.5, 0.5, 0.5], zone)
}

fn hit(from: i8, damage: u16) -> Hit {
    Hit {
        from: Seat(from),
        damage,
        zone: HitZone::Chest,
        kind: HitKind::Bullet,
    }
}

/// A shooter: publishes one body per attached seat, on a tick reports the clock, on `shoot` hits every body it sees,
/// on `spam` queues 300 hits; counts the hits it got and answers each with `[damage, target]`. A hit taken tries to
/// hit back (which the host drops).
#[derive(Default)]
struct Shooter {
    seats: Vec<Seat>,
    taken: Vec<(ModId, u32, Hit)>,
    left: Vec<Seat>,
}

impl Shooter {
    fn publish(&self, cx: &mut ServerCtx<'_>) -> Result<(), CapDenied> {
        let bodies = self
            .seats
            .iter()
            .map(|s| Body::new(*s, s.0 as u32, vec![unit_box(HitZone::Head)]))
            .collect();
        cx.set_bodies(bodies)
    }
}

impl ServerMod for Shooter {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        match p {
            b"shoot" => {
                let targets: Vec<BodyRef> = match cx.bodies() {
                    Ok(b) => b.iter().map(Body::target).collect(),
                    Err(e) => return vec![Outgoing::to_sender(ch, e.to_string().into_bytes())],
                };
                for t in targets {
                    cx.hit(t, hit(from.0, 7)).unwrap();
                }
                Vec::new()
            }
            b"spam" => {
                for i in 0..300u32 {
                    cx.hit(
                        BodyRef {
                            owner: id("other"),
                            id: i,
                        },
                        hit(from.0, 1),
                    )
                    .unwrap();
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
    fn on_attach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        self.seats.push(seat);
        match self.publish(cx) {
            Ok(()) => Vec::new(),
            Err(e) => vec![Outgoing::to_sender(0, e.to_string().into_bytes())],
        }
    }
    fn on_detach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        self.seats.retain(|s| *s != seat);
        self.left.push(seat);
        let _ = self.publish(cx);
        vec![Outgoing::all_except(seat, 0, vec![b'L', seat.0 as u8])]
    }
    fn on_tick(&mut self, cx: &mut ServerCtx<'_>, now_ms: u64) -> Vec<Outgoing> {
        assert_eq!(now_ms, cx.now_ms());
        vec![Outgoing::to_all(0, now_ms.to_le_bytes().to_vec())]
    }
    fn on_hit(
        &mut self,
        cx: &mut ServerCtx<'_>,
        from_mod: &ModId,
        target_id: u32,
        hit: &Hit,
    ) -> Vec<Outgoing> {
        self.taken.push((from_mod.clone(), target_id, *hit));
        // Hitting back from inside on_hit is dropped by the host (one level deep).
        cx.hit(
            BodyRef {
                owner: from_mod.clone(),
                id: 0,
            },
            *hit,
        )
        .unwrap();
        vec![Outgoing::to_all(0, vec![hit.damage as u8, target_id as u8])]
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

/// An API 1.0 mod: none of the new methods. It still builds and the defaults do nothing.
struct Old;

impl ServerMod for Old {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, _saved: Option<&[u8]>) {}
    fn on_client(&mut self, _: &mut ServerCtx<'_>, _: Seat, _: u8, _: &[u8]) -> Vec<Outgoing> {
        Vec::new()
    }
    fn on_attach(&mut self, _: &mut ServerCtx<'_>, _: Seat) -> Vec<Outgoing> {
        Vec::new()
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[test]
fn the_api_is_1_1_and_admitted_by_caret_one() {
    assert_eq!(MOD_API_VERSION, Version::new(1, 1, 0));
    assert!(
        "^1".parse::<mod_manifest::VersionReq>()
            .unwrap()
            .matches(&MOD_API_VERSION)
    );
    assert!(
        "^1.1"
            .parse::<mod_manifest::VersionReq>()
            .unwrap()
            .matches(&MOD_API_VERSION)
    );
    assert_eq!(Cap::parse("world.bodies"), Some(Cap::WorldBodies));
    assert!(!Cap::WorldBodies.client_only());
}

#[test]
fn defaults_of_an_old_mod_do_nothing() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[]), &[chan()]);
    let mut m = Old;
    assert_eq!(h.tick(&mut m), Default::default());
    assert_eq!(h.detach(&mut m, Seat(1)), Default::default());
    assert_eq!(h.hit(&mut m, "other", 3, &hit(0, 5)), Default::default());
}

#[test]
fn the_tick_sees_the_host_clock() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[]), &[chan()]);
    let mut m = Shooter::default();
    h.advance_ms(1234);
    let d = h.tick(&mut m);
    assert_eq!(
        d.sent,
        [Outgoing::to_all(0, 1234u64.to_le_bytes().to_vec())]
    );
    h.advance_ms(50);
    assert_eq!(h.tick(&mut m).sent[0].payload, 1284u64.to_le_bytes());
    assert_eq!(h.cx().now_ms(), 1284);
}

#[test]
fn detach_reaches_the_mod_and_its_answer_is_filtered() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[Cap::WorldBodies]), &[chan()]);
    let mut m = Shooter::default();
    h.attach(&mut m, Seat(1));
    h.attach(&mut m, Seat(2));
    assert_eq!(h.published_bodies().len(), 2);
    let d = h.detach(&mut m, Seat(1));
    assert_eq!(d.sent, [Outgoing::all_except(Seat(1), 0, vec![b'L', 1])]);
    assert_eq!(m.left, [Seat(1)]);
    assert_eq!(h.published_bodies().len(), 1, "the mod took the body away");
}

#[test]
fn bodies_need_the_capability() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[]), &[chan()]);
    let mut m = Shooter::default();
    let d = h.attach(&mut m, Seat(1));
    let text = String::from_utf8(d.sent[0].payload.clone()).unwrap();
    assert!(text.contains("world.bodies"), "{text}");
    assert!(h.published_bodies().is_empty());
    let d = h.deliver(&mut m, Seat(1), 0, b"shoot");
    assert!(String::from_utf8_lossy(&d.sent[0].payload).contains("world.bodies"));
    let mut cx = h.cx();
    assert_eq!(cx.bodies().unwrap_err(), CapDenied(Cap::WorldBodies));
    assert_eq!(
        cx.hit(
            BodyRef {
                owner: id("aa"),
                id: 1
            },
            hit(0, 1)
        ),
        Err(CapDenied(Cap::WorldBodies))
    );
    assert_eq!(cx.set_bodies(Vec::new()), Err(CapDenied(Cap::WorldBodies)));
}

#[test]
fn published_bodies_get_the_owner_and_the_limits() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[Cap::WorldBodies]), &[chan()])
        .with_mod_id("shooter");
    let many: Vec<Body> = (0..70u32)
        .map(|i| Body::new(Seat(0), i, vec![unit_box(HitZone::Legs); 20]))
        .collect();
    h.cx().set_bodies(many).unwrap();
    let b = h.published_bodies();
    assert_eq!(b.len(), MAX_BODIES);
    assert!(b.iter().all(|b| b.boxes.len() == MAX_BOXES_PER_BODY));
    assert!(b.iter().all(|b| b.owner == id("shooter") && b.alive));
    assert!(
        h.host
            .logs
            .iter()
            .any(|(l, m)| *l == LogLevel::Warn && m.contains("cut off")),
        "{:?}",
        h.host.logs
    );
    // Within the limits nothing is logged.
    let (ok, warn) = prepare_bodies(vec![Body::new(Seat(1), 9, vec![])], &id("zz"));
    assert_eq!((ok[0].owner.as_str(), warn), ("zz", None));
}

#[test]
fn a_mod_sees_the_other_bodies_and_its_hits_are_collected() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[Cap::WorldBodies]), &[chan()]);
    let mut steve = Body::new(Seat(3), 41, vec![unit_box(HitZone::Head)]);
    steve.owner = id("blocks");
    h.set_other_bodies(vec![steve.clone()]);
    assert_eq!(h.cx().bodies().unwrap(), std::slice::from_ref(&steve));
    let mut m = Shooter::default();
    let d = h.deliver(&mut m, Seat(1), 0, b"shoot");
    assert_eq!(
        d.hits,
        [(
            BodyRef {
                owner: id("blocks"),
                id: 41
            },
            hit(1, 7)
        )]
    );
    assert_eq!(h.hits(), d.hits, "the host keeps all of them too");
}

#[test]
fn at_most_256_hits_a_call_and_none_from_inside_on_hit() {
    let mut h = MockCtx::with_channels(MockFacts::new(4, 4), caps(&[Cap::WorldBodies]), &[chan()]);
    let mut m = Shooter::default();
    let d = h.deliver(&mut m, Seat(1), 0, b"spam");
    assert_eq!(d.hits.len(), MAX_HITS_PER_CALL);
    assert!(
        h.host
            .logs
            .iter()
            .any(|(_, m)| m.contains("44 hits over 256")),
        "{:?}",
        h.host.logs
    );
    // A hit delivered to the mod: its answer goes out, its hit back is dropped with a line.
    h.host.logs.clear();
    let d = h.hit(&mut m, "blocks", 5, &hit(2, 9));
    assert_eq!(d.sent, [Outgoing::to_all(0, vec![9, 5])]);
    assert!(d.hits.is_empty());
    assert_eq!(m.taken, [(id("blocks"), 5, hit(2, 9))]);
    assert!(
        h.host.logs.iter().any(|(_, m)| m.contains("inside on_hit")),
        "{:?}",
        h.host.logs
    );
}

#[test]
fn the_hit_queue_by_itself() {
    let mut q = HitQueue::new();
    for i in 0..(MAX_HITS_PER_CALL as u32 + 3) {
        q.push(
            BodyRef {
                owner: id("ab"),
                id: i,
            },
            hit(0, 1),
        );
    }
    let (hits, warn) = q.finish();
    assert_eq!(hits.len(), MAX_HITS_PER_CALL);
    assert_eq!(hits[255].0.id, 255, "in order, the first ones kept");
    assert!(warn.unwrap().contains("3 hits"));
    let mut q = HitQueue::nested();
    q.push(
        BodyRef {
            owner: id("ab"),
            id: 0,
        },
        hit(0, 1),
    );
    let (hits, warn) = q.finish();
    assert!(hits.is_empty() && warn.is_some());
    assert_eq!(HitQueue::new().finish(), (Vec::new(), None));
}

/// The 1.0 way to build the mock host (a struct literal, as the conformance harness does) still compiles.
#[test]
fn the_mock_host_keeps_its_1_0_fields() {
    let mut h = MockCtx {
        caps: Caps::all(),
        host: mod_api::testing::MockHost {
            facts: MockFacts::new(2, 2),
            limiter: mod_api::Limiter::new(&[chan()], 64),
            now_ms: 7,
            logs: Vec::new(),
        },
    };
    assert_eq!(h.cx().now_ms(), 7);
    assert!(h.host.facts.bodies().published.is_empty());
}
