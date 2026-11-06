use mod_api::testing::{MockCtx, MockFacts};
use mod_api::{
    Audience, Cap, Caps, CapsError, Limiter, MOD_API_VERSION, Outgoing, Owner, PlotClass, Seat,
    ServerCtx, ServerMod, ServerRegistry, Verdict, WorldFacts,
};
use mod_manifest::{CHANNEL_PAYLOAD_CEILING, Channel, Manifest, VersionReq};

fn chan(id: u8, c2s: u32, s2c: u32, rate: u32, burst: u32) -> Channel {
    Channel {
        id,
        name: format!("c{id}"),
        c2s_max: c2s,
        s2c_max: s2c,
        rate_per_s: rate,
        burst,
    }
}

fn manifest(caps: &str, parts: &str) -> Manifest {
    let channels = if caps.contains("net.channel") {
        "[[channels]]\nid = 0\nname = \"c\"\n"
    } else {
        ""
    };
    Manifest::parse(&format!(
        "[mod]\nid = \"echo\"\nname = \"Echo\"\nversion = \"1.0.0\"\napi = \"^1\"\nlicense = \"MIT\"\n\
         authors = [\"a\"]\nsync = \"cosmetic\"\nparts = [{parts}]\n\
         [rust]\nserver = {{ crate = \"x\", entry = \"x::register\" }}\n\
         client = {{ crate = \"y\", entry = \"y::register\" }}\ncaps = [{caps}]\n{channels}"
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

// ---- limiter -------------------------------------------------------------------------------------------------

#[test]
fn burst_trips_and_recovers_with_time() {
    let mut l = Limiter::new(&[chan(0, 100, 100, 10, 3)], 1024);
    let s = Seat(1);
    for _ in 0..3 {
        assert_eq!(l.check_c2s(s, 0, 1, 0), Verdict::Ok);
    }
    assert_eq!(l.check_c2s(s, 0, 1, 0), Verdict::RateLimited);
    // 10 tokens/s: 99 ms is not yet a whole token, 100 ms is.
    assert_eq!(l.check_c2s(s, 0, 1, 99), Verdict::RateLimited);
    assert_eq!(l.check_c2s(s, 0, 1, 100), Verdict::Ok);
    assert_eq!(l.check_c2s(s, 0, 1, 100), Verdict::RateLimited);
    // A long pause refills to the burst, not beyond.
    for _ in 0..3 {
        assert_eq!(l.check_c2s(s, 0, 1, 1_000_000), Verdict::Ok);
    }
    assert_eq!(l.check_c2s(s, 0, 1, 1_000_000), Verdict::RateLimited);
}

#[test]
fn buckets_are_per_seat_and_per_channel() {
    let mut l = Limiter::new(&[chan(0, 10, 10, 1, 1), chan(1, 10, 10, 1, 1)], 0);
    assert_eq!(l.check_c2s(Seat(1), 0, 1, 0), Verdict::Ok);
    assert_eq!(l.check_c2s(Seat(1), 0, 1, 0), Verdict::RateLimited);
    assert_eq!(l.check_c2s(Seat(2), 0, 1, 0), Verdict::Ok);
    assert_eq!(l.check_c2s(Seat(1), 1, 1, 0), Verdict::Ok);
    l.forget_seat(Seat(1));
    assert_eq!(l.check_c2s(Seat(1), 0, 1, 0), Verdict::Ok);
}

#[test]
fn clock_going_backwards_does_not_refill_or_panic() {
    let mut l = Limiter::new(&[chan(0, 10, 10, 10, 1)], 0);
    assert_eq!(l.check_c2s(Seat(1), 0, 1, 5_000), Verdict::Ok);
    assert_eq!(l.check_c2s(Seat(1), 0, 1, 0), Verdict::RateLimited);
}

#[test]
fn payload_caps_and_ceiling() {
    let mut l = Limiter::new(
        &[
            chan(0, 8, 16, 1000, 1000),
            chan(1, u32::MAX, u32::MAX, 1, 1),
        ],
        0,
    );
    assert_eq!(l.check_c2s(Seat(1), 0, 8, 0), Verdict::Ok);
    assert_eq!(
        l.check_c2s(Seat(1), 0, 9, 0),
        Verdict::TooLarge { len: 9, max: 8 }
    );
    assert_eq!(l.check_s2c(0, 16), Verdict::Ok);
    assert_eq!(l.check_s2c(0, 17), Verdict::TooLarge { len: 17, max: 16 });
    assert_eq!(l.check_c2s(Seat(1), 7, 1, 0), Verdict::UnknownChannel);
    // Clamped to the 56 KiB ceiling.
    let ceil = CHANNEL_PAYLOAD_CEILING as usize;
    assert_eq!(l.check_s2c(1, ceil), Verdict::Ok);
    assert!(matches!(l.check_s2c(1, ceil + 1), Verdict::TooLarge { .. }));
}

#[test]
fn oversize_message_does_not_spend_a_token() {
    let mut l = Limiter::new(&[chan(0, 4, 4, 1, 1)], 0);
    assert!(matches!(
        l.check_c2s(Seat(1), 0, 5, 0),
        Verdict::TooLarge { .. }
    ));
    assert_eq!(l.check_c2s(Seat(1), 0, 4, 0), Verdict::Ok);
}

#[test]
fn cost_bucket_is_independent_and_weighted() {
    let mut l = Limiter::new(&[chan(0, 10, 10, 40, 80)], 0);
    let s = Seat(1);
    assert!(l.allow(s, 0, 64, 0));
    assert!(!l.allow(s, 0, 17, 0));
    assert!(l.allow(s, 0, 16, 0));
    // The message bucket is untouched.
    assert_eq!(l.check_c2s(s, 0, 1, 0), Verdict::Ok);
    assert!(!l.allow(s, 9, 1, 0), "undeclared channel");
}

#[test]
fn state_max_from_manifest() {
    let m = manifest("\"net.channel\"", "\"server-rs\", \"client-rs\"");
    let l = Limiter::from_manifest(&m);
    assert!(l.check_state(1024 * 1024));
    assert!(!l.check_state(1024 * 1024 + 1));
    assert!(l.has_channel(0));
}

// ---- caps ----------------------------------------------------------------------------------------------------

#[test]
fn cap_names_round_trip() {
    for c in Cap::ALL {
        assert_eq!(Cap::parse(c.as_str()), Some(c));
    }
    assert_eq!(Cap::parse("net.socket"), None);
    assert!(Cap::RenderPipeline.trusted_native());
}

#[test]
fn unknown_cap_is_an_error() {
    let m = manifest(
        "\"net.channel\", \"fly.hack\"",
        "\"server-rs\", \"client-rs\"",
    );
    assert_eq!(
        Caps::from_manifest(&m),
        Err(CapsError::Unknown("fly.hack".into()))
    );
}

#[test]
fn known_caps_pass() {
    let m = manifest(
        "\"net.channel\", \"world.read\", \"hud\", \"terrain.cutout\"",
        "\"server-rs\", \"client-rs\"",
    );
    let c = Caps::from_manifest(&m).unwrap();
    assert!(c.has(Cap::NetChannel) && c.has(Cap::TerrainCutout));
    assert!(!c.has(Cap::Camera));
    assert_eq!(c.require(Cap::Camera).unwrap_err().0, Cap::Camera);
}

#[test]
fn client_only_cap_needs_client_part() {
    let m = manifest("\"net.channel\", \"hud\"", "\"server-rs\", \"client-rs\"");
    assert!(Caps::from_manifest(&m).is_ok());
    assert_eq!(Caps::from_names(["hud"]).map(|c| c.has(Cap::Hud)), Ok(true));
}

#[test]
fn api_version_is_admitted_by_caret_one() {
    let req: VersionReq = "^1".parse().unwrap();
    assert!(req.matches(&MOD_API_VERSION));
    let req: VersionReq = "^2".parse().unwrap();
    assert!(!req.matches(&MOD_API_VERSION));
}

// ---- outgoing filtering --------------------------------------------------------------------------------------

#[test]
fn mod_without_net_channel_sends_nothing() {
    let l = Limiter::new(&[chan(0, 10, 10, 1, 1)], 0);
    let f = l.filter_outgoing(
        &Caps::of(&[Cap::WorldRead]),
        vec![Outgoing::to_all(0, vec![1])],
    );
    assert!(f.kept.is_empty());
    assert_eq!(f.dropped[0].reason, Verdict::NoCapability(Cap::NetChannel));
}

#[test]
fn filter_drops_bad_messages_and_keeps_order() {
    let l = Limiter::new(&[chan(0, 10, 4, 1, 1)], 0);
    let out = vec![
        Outgoing::to_sender(0, vec![1]),
        Outgoing::to_all(0, vec![0; 5]),
        Outgoing::to_all(3, vec![1]),
        Outgoing::all_except(Seat(2), 0, vec![2]),
        Outgoing::to_seats(vec![Seat(1), Seat(3)], 0, vec![3]),
    ];
    let f = l.filter_outgoing(&Caps::of(&[Cap::NetChannel]), out);
    let kept: Vec<u8> = f.kept.iter().map(|o| o.payload[0]).collect();
    assert_eq!(kept, [1, 2, 3]);
    assert_eq!(f.dropped.len(), 2);
    assert_eq!(f.dropped[0].reason, Verdict::TooLarge { len: 5, max: 4 });
    assert_eq!(f.dropped[1].reason, Verdict::UnknownChannel);
    assert_eq!(f.kept[1].to, Audience::AllExcept(Seat(2)));
}

// ---- WorldFacts / MockFacts ----------------------------------------------------------------------------------

#[test]
fn facts_wrap_and_bounds() {
    let f = MockFacts::new(4, 3);
    assert_eq!(f.plot_at(0, 0), Some(0));
    assert_eq!(f.plot_at(-1, 0), Some(3), "x wraps");
    assert_eq!(f.plot_at(4, 2), Some(8));
    assert_eq!(f.plot_at(0, -1), None, "y does not wrap");
    assert_eq!(f.plot_at(0, 3), None);
    assert_eq!(f.plot_xy(9), (1, 2));
    let g = MockFacts::new(4, 3).with_wrap(false, true);
    assert_eq!(g.plot_at(-1, 0), None);
    assert_eq!(g.plot_at(0, -1), Some(8));
}

#[test]
fn facts_owner_class_and_unknown() {
    let mut f = MockFacts::new(4, 4).with_seat(Seat(0)).with_turn(7);
    f.set_owner(5, Owner::Seat(Seat(0)));
    f.set_owner(6, Owner::Npc(9));
    f.set_class(5, PlotClass::Hills);
    assert!(f.owned_by(5, Seat(0)));
    assert!(!f.owned_by(6, Seat(0)));
    assert_eq!(f.class(5), PlotClass::Hills);
    assert_eq!(f.class(999), PlotClass::Water);
    assert_eq!(f.owner(999), Owner::None);
    assert_eq!(
        (f.turn(), f.seat_player(Seat(0)), f.seat_player(Seat(4))),
        (7, Some(0), None)
    );
    f.invalidate_owners();
    assert_eq!(f.owner(5), Owner::Unknown);
}

// ---- MockCtx with a small mod --------------------------------------------------------------------------------

/// Claims a plot (payload: x, y) when the sender owns it; replies with an ack and broadcasts to the others.
#[derive(Default)]
struct Claim {
    claimed: Vec<u32>,
    dirty: bool,
}

impl ServerMod for Claim {
    fn start(&mut self, _cx: &mut ServerCtx<'_>, saved: Option<&[u8]>) {
        if let Some(b) = saved {
            self.claimed = b
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                .collect();
        }
    }
    fn on_client(&mut self, cx: &mut ServerCtx<'_>, from: Seat, ch: u8, p: &[u8]) -> Vec<Outgoing> {
        let ok = (|| {
            let facts = cx.facts().ok()?;
            let &[x, y] = p else { return None };
            let plot = facts.plot_at(i16::from(x), i16::from(y))?;
            (facts.owned_by(plot, from) && facts.class(plot) == PlotClass::Land).then_some(plot)
        })();
        match ok {
            Some(plot) => {
                self.claimed.push(plot);
                self.dirty = true;
                vec![
                    Outgoing::to_sender(ch, vec![1]),
                    Outgoing::all_except(from, ch, p.to_vec()),
                ]
            }
            None => vec![Outgoing::to_sender(ch, vec![0])],
        }
    }
    fn on_attach(&mut self, _cx: &mut ServerCtx<'_>, _seat: Seat) -> Vec<Outgoing> {
        vec![Outgoing::to_sender(0, vec![self.claimed.len() as u8])]
    }
    fn persist(&self) -> Vec<u8> {
        self.claimed.iter().flat_map(|c| c.to_le_bytes()).collect()
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn mark_saved(&mut self) {
        self.dirty = false;
    }
}

fn host(caps: Caps) -> MockCtx {
    let mut facts = MockFacts::new(8, 8).with_seat(Seat(0)).with_seat(Seat(1));
    facts.set_owner(9, Owner::Seat(Seat(0))); // (1, 1)
    facts.set_owner(10, Owner::Seat(Seat(0)));
    facts.set_class(10, PlotClass::Mountain); // (2, 1)
    MockCtx::with_channels(facts, caps, &[chan(0, 16, 16, 10, 2)])
}

#[test]
fn ownership_is_checked_against_facts_not_payload() {
    let mut h = host(Caps::of(&[Cap::NetChannel, Cap::WorldRead]));
    let mut m = Claim::default();
    h.start(&mut m, None);

    let d = h.deliver(&mut m, Seat(0), 0, &[1, 1]);
    assert_eq!(d.sent[0], Outgoing::to_sender(0, vec![1]));
    assert_eq!(d.sent[1].to, Audience::AllExcept(Seat(0)));
    // Not the owner.
    assert_eq!(h.deliver(&mut m, Seat(1), 0, &[1, 1]).sent[0].payload, [0]);
    // Owned but a mountain.
    assert_eq!(h.deliver(&mut m, Seat(0), 0, &[2, 1]).sent[0].payload, [0]);

    // Ownership changes between two messages: the second one sees it.
    h.advance_ms(1000);
    h.facts_mut().set_owner(9, Owner::Seat(Seat(1)));
    assert_eq!(h.deliver(&mut m, Seat(0), 0, &[1, 1]).sent[0].payload, [0]);
    assert_eq!(h.deliver(&mut m, Seat(1), 0, &[1, 1]).sent[0].payload, [1]);

    // Unknown table: refuse.
    h.facts_mut().invalidate_owners();
    h.advance_ms(1000);
    assert_eq!(h.deliver(&mut m, Seat(1), 0, &[1, 1]).sent[0].payload, [0]);
}

#[test]
fn mock_host_refuses_before_the_mod_runs() {
    let mut h = host(Caps::of(&[Cap::NetChannel, Cap::WorldRead]));
    let mut m = Claim::default();
    let d = h.deliver(&mut m, Seat(0), 0, &[0; 17]);
    assert_eq!(d.refused, Some(Verdict::TooLarge { len: 17, max: 16 }));
    assert!(m.claimed.is_empty() && d.sent.is_empty());
    assert_eq!(
        h.deliver(&mut m, Seat(0), 5, &[1]).refused,
        Some(Verdict::UnknownChannel)
    );
    h.deliver(&mut m, Seat(0), 0, &[1, 1]);
    h.deliver(&mut m, Seat(0), 0, &[1, 1]);
    assert_eq!(
        h.deliver(&mut m, Seat(0), 0, &[1, 1]).refused,
        Some(Verdict::RateLimited)
    );
    assert!(!h.host.logs.is_empty());
}

#[test]
fn no_world_read_means_no_facts_and_no_net_means_no_sends() {
    let mut h = host(Caps::of(&[Cap::NetChannel]));
    let mut m = Claim::default();
    // Without world.read the mod cannot see facts and denies.
    assert_eq!(h.deliver(&mut m, Seat(0), 0, &[1, 1]).sent[0].payload, [0]);
    assert!(h.cx().facts().is_err());

    let mut h = host(Caps::of(&[Cap::WorldRead]));
    let d = h.deliver(&mut m, Seat(0), 0, &[1, 1]);
    assert!(d.sent.is_empty());
    assert_eq!(d.dropped.len(), 2);
    assert!(h.host.logs.iter().any(|(_, s)| s.contains("dropped")));
}

#[test]
fn persist_restore_dirty_and_attach() {
    let mut h = host(Caps::all());
    let mut m = Claim::default();
    h.start(&mut m, None);
    assert!(!m.is_dirty());
    h.deliver(&mut m, Seat(0), 0, &[1, 1]);
    assert!(m.is_dirty());
    let blob = m.persist();
    m.mark_saved();
    assert!(!m.is_dirty());

    let mut m2 = Claim::default();
    h.start(&mut m2, Some(&blob));
    assert_eq!(h.attach(&mut m2, Seat(1)).sent[0].payload, [1]);
}

#[test]
fn ctx_allow_uses_the_cost_bucket() {
    let mut h = host(Caps::all());
    let mut cx = h.cx();
    assert!(cx.allow(Seat(0), 0, 2));
    assert!(!cx.allow(Seat(0), 0, 1));
}

#[test]
fn registry_rejects_bad_and_duplicate_ids() {
    let mut r = ServerRegistry::new();
    r.add("echo", || Box::new(Claim::default()))
        .add("echo", || Box::new(Claim::default()))
        .add("Bad Id", || Box::new(Claim::default()));
    assert_eq!(r.problems().len(), 2);
    assert_eq!(r.ids().count(), 1);
    assert!(r.create(&"echo".parse().unwrap()).is_some());
    assert!(r.create(&"other".parse().unwrap()).is_none());
}
