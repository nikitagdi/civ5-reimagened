//! The cosmetic terrain share of the mods (v14, mod API 1.2) on the scripted server: the shared plots are kept beside
//! the view, never in it (the view, its revealed plots and its hash stay as the server sent them), per mod, and they
//! go with a fresh stream or a broken batch.

mod common;

use common::world::World;
use common::*;
use protocol::{
    DetachReason, GameMsg, PlotBasisView, PlotFlags, PlotType, ServerMsg, TerrainShare,
};

fn plan() -> GamePlan {
    GamePlan {
        game: 7,
        player: 1,
        stream: 3,
        last_cmd_id: 0,
        last_seq: 0,
    }
}

fn game() -> (Net, protocol::Token) {
    let mut n = Net::start();
    n.srv.world = Some(World::new(7, 11, 6, 5));
    n.srv.plan = Some(plan());
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    (n, token)
}

fn hill() -> PlotBasisView {
    PlotBasisView {
        plot_type: PlotType::Hill,
        terrain: 1,
        feature: -1,
        river_flow: [-1; 3],
        ..PlotBasisView::default()
    }
}

fn share(mod_idx: u16, reset: bool, plots: &[u32]) -> GameMsg {
    GameMsg::TerrainShare(TerrainShare {
        mod_idx,
        reset,
        plots: plots.iter().map(|&i| (i, hill())).collect(),
    })
}

fn shared(n: &Net) -> Vec<u32> {
    n.core
        .terrain_share()
        .plots()
        .iter()
        .map(|(i, _)| *i)
        .collect()
}

/// The plots of the view the seat has revealed.
fn revealed(n: &Net) -> Vec<usize> {
    n.core
        .shown()
        .expect("a view")
        .map
        .plots
        .iter()
        .enumerate()
        .filter(|(_, p)| p.flags.contains(PlotFlags::REVEALED))
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn a_share_is_kept_beside_the_view_and_reveals_nothing() {
    let (mut n, token) = game();
    let before = revealed(&n);
    let hash = n.core.base_hash();
    let rev = n.core.revision();
    // Plots the seat has not revealed (the far corner of the 6 x 5 map), from two mods.
    let unrevealed: Vec<u32> = (0..30u32)
        .filter(|i| !before.contains(&(*i as usize)))
        .take(4)
        .collect();
    assert_eq!(unrevealed.len(), 4, "the fixture leaves plots unrevealed");
    let batch = n.srv.world.as_mut().unwrap().push(vec![
        share(1, false, &unrevealed[..3]),
        share(2, false, &unrevealed[2..]),
    ]);
    n.push(token, batch);
    n.settle();
    assert_eq!(shared(&n), unrevealed);
    let (_, rec) = n.core.terrain_share().plots()[0];
    assert_eq!(*rec, hill(), "the record as the server sent it");
    // The view is untouched: the same revealed plots, the same hash. The batch itself was taken.
    assert_eq!(revealed(&n), before);
    assert_eq!(n.core.base_hash(), hash);
    assert!(n.core.revision() > rev);
    // One mod unshares: the other's plots stay.
    let batch = n
        .srv
        .world
        .as_mut()
        .unwrap()
        .push(vec![share(1, true, &[])]);
    n.push(token, batch);
    n.settle();
    assert_eq!(shared(&n), &unrevealed[2..]);
}

#[test]
fn a_broken_batch_shares_nothing_and_a_detach_forgets_the_share() {
    let (mut n, token) = game();
    let w = n.srv.world.as_mut().unwrap();
    let _lost = w.push(vec![share(0, false, &[1])]);
    let gap = w.push(vec![share(0, false, &[2])]);
    n.push(token, gap);
    n.settle();
    assert!(n.core.terrain_share().is_empty(), "{:?}", shared(&n));
    // A share that is taken, then the seat is detached: nothing is shared any more.
    let mut n2 = game();
    let batch =
        n2.0.srv
            .world
            .as_mut()
            .unwrap()
            .push(vec![share(0, false, &[3])]);
    let token2 = n2.1;
    n2.0.push(token2, batch);
    n2.0.settle();
    assert_eq!(shared(&n2.0), [3]);
    let rev = n2.0.core.terrain_share().rev();
    n2.0.push(
        token2,
        ServerMsg::Detached {
            reason: DetachReason::Left,
        },
    );
    n2.0.settle();
    assert!(n2.0.core.terrain_share().is_empty());
    assert!(n2.0.core.terrain_share().rev() > rev);
}

#[test]
fn a_share_over_the_limit_is_dropped() {
    let (mut n, token) = game();
    let many: Vec<u32> = (0..=protocol::MAX_TERRAIN_SHARE_PLOTS as u32).collect();
    let batch = n
        .srv
        .world
        .as_mut()
        .unwrap()
        .push(vec![share(0, false, &many)]);
    n.push(token, batch);
    n.settle();
    assert!(n.core.terrain_share().is_empty());
}
