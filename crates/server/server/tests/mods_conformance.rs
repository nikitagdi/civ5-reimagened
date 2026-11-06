//! Conformance harness for server mods (T46).
//!
//! [`check_server_mod`] runs the host contract against any registered server mod on the mock host, built from the
//! mod's own `mod.toml`:
//!
//! * `persist` -> `start(saved)` -> `persist` returns the same bytes, and the bytes fit `state_max`;
//! * `on_attach` after a restart from the saved bytes equals `on_attach` of the live mod;
//! * oversize, empty and garbage payloads, garbage saves and unknown channels never panic and never change the
//!   saved state;
//! * a channel the manifest does not declare is refused by the host before the mod sees it.
//!
//! The mod author supplies a few valid messages (`Probe::valid`) so the state is not empty. The public example
//! `hello` runs through it below; copy the call to test another mod.

use hello_server::register;
use hello_shared::{CH_ECHO, Msg};
use mod_api::testing::{Delivery, MockCtx, MockFacts, MockHost};
use mod_api::{Caps, ChannelId, Limiter, Outgoing, Seat, ServerMod, ServerRegistry, Verdict};
use mod_manifest::{Manifest, ModId};

/// What the harness cannot invent: messages the mod accepts. May be empty (then only the state-less checks run).
pub struct Probe {
    /// `(seat, channel, payload)` the mod handles as valid input; they should change its state.
    pub valid: Vec<(Seat, ChannelId, Vec<u8>)>,
    /// Seats to attach for the replay comparison.
    pub attach_seats: Vec<Seat>,
}

fn host_for(m: &Manifest) -> MockCtx {
    MockCtx {
        caps: Caps::from_manifest(m).unwrap_or_else(|e| panic!("caps: {e}")),
        host: MockHost {
            facts: MockFacts::new(8, 8),
            limiter: Limiter::from_manifest(m),
            now_ms: 0,
            logs: Vec::new(),
        },
    }
}

fn instance(reg: &ServerRegistry, id: &ModId) -> Box<dyn ServerMod> {
    reg.create(id)
        .unwrap_or_else(|| panic!("`{id}` is not registered"))
}

/// A comparable form of a delivery: audience, channel, payload per message, in order.
fn shape(d: &Delivery) -> Vec<(String, ChannelId, Vec<u8>)> {
    d.sent
        .iter()
        .map(|o: &Outgoing| (format!("{:?}", o.to), o.ch, o.payload.clone()))
        .collect()
}

/// Deterministic pseudo-random bytes (xorshift), so a failure repeats.
fn noise(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 24) as u8
        })
        .collect()
}

/// Runs the whole host contract against the server mod `id` registered in `reg`, with its `mod.toml` text.
pub fn check_server_mod(reg: &ServerRegistry, mod_toml: &str, probe: &Probe) {
    let m = Manifest::parse(mod_toml).unwrap_or_else(|e| panic!("mod.toml: {e}"));
    let id = m.id.clone();
    assert!(reg.problems().is_empty(), "registry: {:?}", reg.problems());
    assert!(
        reg.ids().any(|i| *i == id),
        "`{id}` from mod.toml is not registered"
    );
    let state_max = m.rust.state_max as usize;

    // --- a fresh game, driven with valid input ---
    let mut h = host_for(&m);
    let mut live = instance(reg, &id);
    h.start(live.as_mut(), None);
    let empty_state = live.persist();
    assert!(empty_state.len() <= state_max, "fresh state over state_max");
    for (seat, ch, payload) in &probe.valid {
        let d = h.deliver(live.as_mut(), *seat, *ch, payload);
        assert_eq!(
            d.refused, None,
            "a valid probe message was refused by the host"
        );
    }
    let state = live.persist();
    assert!(
        state.len() <= state_max,
        "state {} bytes over state_max {state_max}",
        state.len()
    );

    // --- persist -> restore round trip is stable ---
    let mut h2 = host_for(&m);
    let mut restored = instance(reg, &id);
    h2.start(restored.as_mut(), Some(&state));
    assert_eq!(
        restored.persist(),
        state,
        "persist -> start -> persist changed the bytes"
    );
    let mut again = instance(reg, &id);
    h2.start(again.as_mut(), Some(&restored.persist()));
    assert_eq!(
        again.persist(),
        state,
        "a second round trip changed the bytes"
    );
    // A loaded state is clean: nothing to save until something changes.
    restored.mark_saved();
    assert!(
        !restored.is_dirty() || restored.persist() == state,
        "mark_saved left the mod dirty with a different state"
    );

    // --- on_attach replay after a restart equals the live state ---
    for seat in &probe.attach_seats {
        let live_att = shape(&h.attach(live.as_mut(), *seat));
        let rest_att = shape(&h2.attach(restored.as_mut(), *seat));
        assert_eq!(
            live_att, rest_att,
            "on_attach after a restart differs from the live mod for {seat:?}"
        );
    }

    // --- garbage never panics and never changes the state ---
    let before = live.persist();
    let mut declared: Vec<(ChannelId, usize)> = m
        .channels
        .iter()
        .map(|c| (c.id, c.c2s_max as usize))
        .collect();
    declared.sort();
    for (ch, c2s_max) in &declared {
        let mut payloads: Vec<Vec<u8>> = vec![
            vec![],
            vec![0],
            vec![0xFF; 3],
            noise(1, 7),
            noise(2, *c2s_max),
            noise(3, c2s_max + 1),
            vec![0u8; 64 * 1024],
            vec![0xFF; 64 * 1024],
        ];
        for s in 0..64u64 {
            payloads.push(noise(100 + s, (s as usize * 13) % (c2s_max + 8)));
        }
        for (i, p) in payloads.iter().enumerate() {
            // Through the host: oversize and rate-limited ones are refused, the rest reach the mod.
            h.advance_ms(1000);
            let d = h.deliver(live.as_mut(), Seat(1), *ch, p);
            if p.len() > *c2s_max {
                assert!(
                    matches!(d.refused, Some(Verdict::TooLarge { .. })),
                    "payload {i} over c2s_max was not refused: {:?}",
                    d.refused
                );
            }
            // Straight at the mod, as if the host's checks were absent: still no panic.
            let mut ctx = h.cx();
            let _ = live.on_client(&mut ctx, Seat(2), *ch, p);
        }
    }
    // An undeclared channel is refused before the mod, and a mod called with it anyway ignores it.
    let unknown = (0..=u8::MAX)
        .find(|c| declared.iter().all(|(d, _)| d != c))
        .expect("a free channel id");
    let d = h.deliver(live.as_mut(), Seat(1), unknown, &[1, 2, 3]);
    assert_eq!(d.refused, Some(Verdict::UnknownChannel));
    assert!(d.sent.is_empty());
    {
        let mut ctx = h.cx();
        let _ = live.on_client(&mut ctx, Seat(1), unknown, &[1, 2, 3]);
        let _ = live.on_client(&mut ctx, Seat(1), unknown, &[]);
    }
    assert_eq!(
        live.persist(),
        before,
        "garbage or an unknown channel changed the persisted state"
    );
    for seat in &probe.attach_seats {
        let _ = h.attach(live.as_mut(), *seat);
    }

    // --- garbage saves: start never panics, and the state it leaves fits state_max ---
    let mut saves: Vec<Vec<u8>> = vec![
        vec![],
        vec![0],
        vec![0xFF; 5],
        vec![0u8; 1024 * 1024],
        noise(9, 4096),
        state[..state.len().saturating_sub(1)].to_vec(),
        [state.clone(), vec![0xAB; 9]].concat(),
    ];
    for s in 0..32u64 {
        saves.push(noise(500 + s, (s as usize * 7) % 300));
    }
    for (i, bytes) in saves.iter().enumerate() {
        let mut g = instance(reg, &id);
        let mut hg = host_for(&m);
        hg.start(g.as_mut(), Some(bytes));
        assert!(
            g.persist().len() <= state_max,
            "garbage save {i}: state over state_max"
        );
        for seat in &probe.attach_seats {
            let _ = hg.attach(g.as_mut(), *seat);
        }
        // The game goes on: the mod still takes the valid input.
        for (seat, ch, payload) in &probe.valid {
            let _ = hg.deliver(g.as_mut(), *seat, *ch, payload);
        }
    }

    // --- a turn event does not panic ---
    {
        let mut ctx = h.cx();
        live.on_turn(&mut ctx, 1);
        live.on_turn(&mut ctx, i32::MAX);
    }
}

const HELLO_TOML: &str = include_str!("../../../../examples/mods/hello/mod.toml");

fn say(text: &str) -> Vec<u8> {
    Msg::Say { text: text.into() }.encode()
}

#[test]
fn hello_conforms_to_the_server_mod_contract() {
    let mut reg = ServerRegistry::new();
    register(&mut reg);
    check_server_mod(
        &reg,
        HELLO_TOML,
        &Probe {
            valid: vec![
                (Seat(1), CH_ECHO, say("hi")),
                (Seat(2), CH_ECHO, say("there")),
                (Seat(1), CH_ECHO, say("")),
            ],
            attach_seats: vec![Seat(0), Seat(1), Seat(7)],
        },
    );
}

#[test]
fn the_harness_catches_a_mod_whose_restore_is_not_stable() {
    // A mod that persists a counter but restores it off by one: the round trip must fail.
    struct Drifting(u8);
    impl ServerMod for Drifting {
        fn start(&mut self, _: &mut mod_api::ServerCtx<'_>, saved: Option<&[u8]>) {
            self.0 = saved.and_then(|s| s.first().copied()).map_or(0, |b| b + 1);
        }
        fn on_client(
            &mut self,
            _: &mut mod_api::ServerCtx<'_>,
            _: Seat,
            _: ChannelId,
            _: &[u8],
        ) -> Vec<Outgoing> {
            self.0 = self.0.wrapping_add(1);
            Vec::new()
        }
        fn on_attach(&mut self, _: &mut mod_api::ServerCtx<'_>, _: Seat) -> Vec<Outgoing> {
            Vec::new()
        }
        fn persist(&self) -> Vec<u8> {
            vec![self.0]
        }
    }
    let mut reg = ServerRegistry::new();
    reg.add("hello", || Box::new(Drifting(0)));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        check_server_mod(
            &reg,
            HELLO_TOML,
            &Probe {
                valid: vec![(Seat(1), CH_ECHO, say("x"))],
                attach_seats: vec![Seat(1)],
            },
        )
    }));
    assert!(r.is_err(), "the harness accepted an unstable restore");
}
