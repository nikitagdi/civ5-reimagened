//! API 1.2: the cosmetic terrain share (`world.share_terrain`), through the mock host.

use mod_api::testing::{MockCtx, MockFacts};
use mod_api::{
    Cap, CapDenied, Caps, LogLevel, MAX_SHARE_PLOTS_PER_CALL, MOD_API_VERSION, Outgoing, PlotIdx,
    Seat, ServerCtx, ServerMod, ShareOut, Version,
};
use mod_manifest::Channel;

/// An arena mod: on attach shares the hexes within one step of plot 9 to the seat, `big` shares 300 plots, `end`
/// unshares the sender, `denied` reports a missing capability.
#[derive(Default)]
struct Arena {
    denied: Option<CapDenied>,
}

const ARENA: [PlotIdx; 5] = [8, 9, 10, 1, 17];

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
            b"big" => {
                let plots: Vec<PlotIdx> = (0..300).collect();
                cx.share_terrain(from, &plots)
            }
            b"end" => cx.unshare_terrain(from),
            _ => cx.share_terrain(from, &ARENA),
        };
        self.denied = r.err();
        Vec::new()
    }
    fn on_attach(&mut self, cx: &mut ServerCtx<'_>, seat: Seat) -> Vec<Outgoing> {
        self.denied = cx.share_terrain(seat, &ARENA).err();
        Vec::new()
    }
    fn persist(&self) -> Vec<u8> {
        Vec::new()
    }
}

fn host(caps: &[Cap]) -> MockCtx {
    let channel = Channel {
        id: 0,
        name: "c".into(),
        c2s_max: 64,
        s2c_max: 64,
        rate_per_s: 100,
        burst: 50,
    };
    let mut all = vec![Cap::NetChannel];
    all.extend_from_slice(caps);
    MockCtx::with_channels(MockFacts::new(32, 20), Caps::of(&all), &[channel])
}

#[test]
fn the_api_is_1_2_with_the_share_capability() {
    // Later minors only add (1.3: the UI v2 for mods), so a `^1.2` mod keeps loading.
    assert!(MOD_API_VERSION >= Version::new(1, 2, 0));
    assert_eq!(MOD_API_VERSION.major, 1);
    assert!(
        "^1.2"
            .parse::<mod_manifest::VersionReq>()
            .unwrap()
            .matches(&MOD_API_VERSION)
    );
    assert_eq!(
        Cap::parse("world.share_terrain"),
        Some(Cap::WorldShareTerrain)
    );
    assert!(!Cap::WorldShareTerrain.client_only());
    assert!(!Cap::WorldShareTerrain.trusted_native());
    assert!(Cap::ALL.contains(&Cap::WorldShareTerrain));
}

#[test]
fn a_share_without_the_capability_is_denied_and_sends_nothing() {
    let mut h = host(&[]);
    let mut m = Arena::default();
    h.attach(&mut m, Seat(1));
    assert_eq!(m.denied, Some(CapDenied(Cap::WorldShareTerrain)));
    assert!(h.terrain_shares().is_empty());
}

#[test]
fn a_share_goes_to_that_seat_once_and_only_what_is_new() {
    let mut h = host(&[Cap::WorldShareTerrain]);
    let mut m = Arena::default();
    h.attach(&mut m, Seat(1));
    assert_eq!(m.denied, None);
    assert_eq!(
        h.terrain_shares(),
        [ShareOut {
            seat: Seat(1),
            reset: false,
            plots: ARENA.to_vec()
        }]
    );
    // The same plots again: nothing new, nothing sent.
    h.deliver(&mut m, Seat(1), 0, b"again");
    assert_eq!(h.terrain_shares().len(), 1);
    assert_eq!(h.shared(Seat(1)), [1, 8, 9, 10, 17]);
    assert!(h.shared(Seat(2)).is_empty(), "another seat gets nothing");
    // Unshare: one reset for that seat, then the share comes whole again.
    h.deliver(&mut m, Seat(1), 0, b"end");
    assert_eq!(
        h.terrain_shares()[1],
        ShareOut {
            seat: Seat(1),
            reset: true,
            plots: vec![]
        }
    );
    assert!(h.shared(Seat(1)).is_empty());
    h.deliver(&mut m, Seat(1), 0, b"again");
    assert_eq!(h.terrain_shares()[2].plots, ARENA);
}

#[test]
fn a_call_shares_at_most_the_limit_and_says_so_in_the_log() {
    let mut h = host(&[Cap::WorldShareTerrain]);
    let mut m = Arena::default();
    h.deliver(&mut m, Seat(0), 0, b"big");
    let sent = h.terrain_shares();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].plots.len(), MAX_SHARE_PLOTS_PER_CALL);
    assert!(
        h.host
            .logs
            .iter()
            .any(|(l, s)| *l == LogLevel::Warn && s.contains("over 256")),
        "{:?}",
        h.host.logs
    );
}

#[test]
fn a_detach_or_a_fresh_attach_forgets_what_the_seat_was_given() {
    let mut h = host(&[Cap::WorldShareTerrain]);
    let mut m = Arena::default();
    h.attach(&mut m, Seat(1));
    h.detach(&mut m, Seat(1));
    assert!(h.shared(Seat(1)).is_empty());
    // The client comes back: the share is sent whole again.
    h.attach(&mut m, Seat(1));
    assert_eq!(h.terrain_shares().len(), 2);
    assert_eq!(h.terrain_shares()[1].plots, ARENA);
}
