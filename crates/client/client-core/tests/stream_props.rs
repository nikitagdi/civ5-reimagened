//! Property of the stream (net-client 9): on 10 000 random steps of a game, with damage mixed in, the
//! client's base view equals the server's by hash, a damaged step never leaves a half-applied view, and
//! every healing costs one `NeedKeyframe` and no reconnect. Own xorshift, no test dependencies.

mod common;

use client_core::{ClientEvent, ConnState, DirtyKeys};
use common::views::Rng;
use common::world::{Damage, World};
use common::*;
use protocol::{KeyframeReason, ServerMsg, Token, WorldView};

const STEPS: u32 = 10_000;
const GAME_STEPS: u32 = 100;

fn world(n: &mut Net) -> &mut World {
    n.srv.world.as_mut().unwrap()
}

fn in_sync(n: &mut Net) -> bool {
    let (view, hash, basis) = {
        let w = n.srv.world.as_ref().unwrap();
        (w.view.clone(), w.view_hash(), w.basis_hash())
    };
    n.core.shown() == Some(&view)
        && n.core.base_hash() == Some(hash)
        && n.core.basis_hash() == Some(basis)
}

fn send(n: &mut Net, token: Token, m: ServerMsg) {
    n.push(token, m);
    n.settle();
}

/// Every plot and every unit or city record that differs between the two views is named by the change.
fn change_covers(
    old: &WorldView,
    new: &WorldView,
    c: &client_core::ViewChange,
) -> Result<(), String> {
    if let Some(list) = &c.dirty_plots {
        for i in 0..new.map.plots.len() {
            if old.map.plots[i] != new.map.plots[i] && !list.contains(&(i as u32)) {
                return Err(format!("plot {i} changed but is not dirty"));
            }
        }
        if list.windows(2).any(|w| w[0] >= w[1]) {
            return Err("dirty plots repeat or are unsorted".into());
        }
    }
    let keyed = |what: &str,
                 keys: &DirtyKeys<(i8, i32)>,
                 old: Vec<((i8, i32), String)>,
                 new: Vec<((i8, i32), String)>|
     -> Result<(), String> {
        let Some(k) = keys else { return Ok(()) };
        for (key, rec) in &new {
            let was = old.iter().find(|(o, _)| o == key).map(|(_, r)| r);
            if was != Some(rec) && !k.upsert.contains(key) {
                return Err(format!("{what} {key:?} changed but is not in upsert"));
            }
        }
        for (key, _) in &old {
            if !new.iter().any(|(n, _)| n == key) && !k.remove.contains(key) {
                return Err(format!("{what} {key:?} vanished but is not in remove"));
            }
        }
        if k.upsert.iter().any(|u| k.remove.contains(u)) {
            return Err(format!("{what}: a key is both upserted and removed"));
        }
        Ok(())
    };
    let units = |v: &WorldView| -> Vec<((i8, i32), String)> {
        v.units
            .iter()
            .map(|u| ((u.owner, u.id), format!("{u:?}")))
            .collect()
    };
    let cities = |v: &WorldView| -> Vec<((i8, i32), String)> {
        v.cities
            .iter()
            .map(|u| ((u.owner, u.id), format!("{u:?}")))
            .collect()
    };
    keyed("unit", &c.dirty_units, units(old), units(new))?;
    keyed("city", &c.dirty_cities, cities(old), cities(new))
}

#[test]
fn the_client_base_follows_the_server_on_10000_random_steps() {
    let mut r = Rng(0xC0FF_EE12_3456_789B);
    let mut steps = 0u32;
    let mut game_no = 0u64;
    let (mut damaged, mut gaps, mut drops, mut basis_steps, mut jumps) =
        (0u32, 0u32, 0u32, 0u32, 0u32);
    while steps < STEPS {
        game_no += 1;
        let (w, h) = (3 + r.below(5) as u16, 3 + r.below(5) as u16);
        let mut n = Net::start();
        n.srv.world = Some(World::new(game_no as u32, r.next(), w, h));
        n.srv.plan = Some(GamePlan {
            game: game_no as u32,
            player: 1,
            stream: 1,
            last_cmd_id: 0,
            last_seq: 0,
        });
        n.connect();
        n.login();
        let mut token = n.core.token().expect("token");
        n.core.set_catalog(protocol::Catalog::default());
        assert!(in_sync(&mut n), "game {game_no}: the keyframe");
        n.core.take_change();
        let mut need_expected = 0usize;
        let mut stream = 1u64;

        for _ in 0..GAME_STEPS {
            steps += 1;
            let old = n.core.shown().cloned().expect("a view");
            let roll = r.below(100);
            if roll < 55 {
                let m = world(&mut n).step();
                send(&mut n, token, m);
                assert!(in_sync(&mut n), "game {game_no} step {steps}: a game step");
            } else if roll < 72 {
                jumps += 1;
                let m = world(&mut n).jump();
                send(&mut n, token, m);
                assert!(in_sync(&mut n), "game {game_no} step {steps}: a jump");
            } else if roll < 78 {
                basis_steps += 1;
                let m = world(&mut n).basis_step(None);
                send(&mut n, token, m);
                assert!(in_sync(&mut n), "game {game_no} step {steps}: a basis step");
            } else if roll < 94 {
                // A damaged diff: the view stays exactly as it was, one request, one keyframe heals it.
                damaged += 1;
                let (damage, reason) = match r.below(6) {
                    0 => (Damage::ViewHash, KeyframeReason::HashMismatch),
                    1 => (Damage::Base, KeyframeReason::BaseMismatch),
                    2 => (Damage::OutOfRange, KeyframeReason::ApplyError),
                    3 => (Damage::UnknownKey, KeyframeReason::ApplyError),
                    4 => (Damage::ViewHash, KeyframeReason::HashMismatch),
                    _ => (Damage::BasisHash, KeyframeReason::BasisMismatch),
                };
                let (hash, basis) = (n.core.base_hash(), n.core.basis_hash());
                n.srv.ignore_need_keyframe = true;
                let m = if damage == Damage::BasisHash {
                    world(&mut n).basis_step(Some(damage))
                } else {
                    let new = world(&mut n).mutated();
                    world(&mut n).step_damaged(new, Some(damage))
                };
                send(&mut n, token, m);
                need_expected += 1;
                assert_eq!(
                    n.srv.need_keyframe.len(),
                    need_expected,
                    "game {game_no} step {steps}"
                );
                assert_eq!(*n.srv.need_keyframe.last().unwrap(), reason, "{damage:?}");
                assert_eq!(
                    n.core.shown(),
                    Some(&old),
                    "{damage:?}: the view must not change"
                );
                assert_eq!(
                    (n.core.base_hash(), n.core.basis_hash()),
                    (hash, basis),
                    "{damage:?}"
                );
                // More diffs before the keyframe are not applied and not asked for again.
                if r.chance(50) {
                    let m = world(&mut n).step();
                    send(&mut n, token, m);
                    assert_eq!(n.srv.need_keyframe.len(), need_expected);
                    assert_eq!(n.core.shown(), Some(&old));
                }
                n.srv.ignore_need_keyframe = false;
                let kf = world(&mut n).answer(reason);
                send(&mut n, token, kf);
                assert!(
                    in_sync(&mut n),
                    "game {game_no} step {steps}: {damage:?} heals"
                );
            } else if roll < 97 {
                // A batch is lost on the way: the stream breaks and the tail replays.
                gaps += 1;
                world(&mut n).lose_one();
                let m = world(&mut n).step();
                send(&mut n, token, m);
                n.advance(2_000);
                assert!(in_sync(&mut n), "game {game_no} step {steps}: a gap");
                token = n.core.token().unwrap();
                // A minute of quiet: the ladder of stream failures starts over.
                n.advance_coarse(61_000, 500);
            } else {
                // The line drops: Replay, or Fresh when the gateway restarted meanwhile.
                drops += 1;
                if r.chance(50) {
                    stream += 1;
                    n.srv.plan.as_mut().unwrap().stream = stream;
                }
                n.server_drops(token, 1012);
                n.advance(2_000);
                assert!(in_sync(&mut n), "game {game_no} step {steps}: a drop");
                assert_eq!(n.core.conn(), ConnState::Online);
                token = n.core.token().unwrap();
            }
            // The change names what moved between the view before and the view now.
            let new = n.core.shown().cloned().expect("a view");
            let c = n.core.take_change();
            if let Err(e) = change_covers(&old, &new, &c) {
                panic!("game {game_no} step {steps}: {e}");
            }
        }
        // The only requests of the game were the damaged ones, and nothing else went wrong.
        assert_eq!(n.srv.need_keyframe.len(), need_expected);
        assert_eq!(n.core.stats().decode_errors, 0);
        assert!(!n.events.iter().any(|e| matches!(e, ClientEvent::Fatal(_))));
    }
    // The mix really happened.
    assert!(damaged > 800 && gaps > 100 && drops > 50 && basis_steps > 300 && jumps > 1_000);
}
