//! A game on the server side for the stream tests: a view that evolves, its map basis, the batches the
//! gateway would send (keyframes, diffs with hashes) and a log to replay from. It can also send damaged
//! batches while its own state stays right, which is what a lossy path looks like to the client.

#![allow(dead_code)]

use protocol::{
    BasisState, GameBatch, GameMsg, Keyframe, KeyframeReason, MapBasis, ServerMsg, ViewDelta,
    ViewDiff, ViewHash, WorldView, basis_hash_of, diff,
};

use super::views::{Rng, basis_of, gen_basis_plot, gen_view, mutate};

/// How a batch is damaged on its way to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    /// The hash in the diff is not the hash of the result.
    ViewHash,
    /// The diff says it follows another batch.
    Base,
    /// A plot index outside the map.
    OutOfRange,
    /// The basis hash of the diff is wrong (needs a diff with a basis part).
    BasisHash,
    /// A removed key the client does not have.
    UnknownKey,
}

pub struct World {
    pub game: u32,
    pub view: WorldView,
    pub hash: ViewHash,
    pub basis: BasisState,
    /// `seq` of the last batch sent; the next is one more.
    pub seq: u64,
    /// `seq` of the batch with the last view message.
    pub view_seq: u64,
    /// Batches sent, by `seq - 1`.
    pub log: Vec<ServerMsg>,
    pub rng: Rng,
    pub w: u16,
    pub h: u16,
}

impl World {
    pub fn new(game: u32, seed: u64, w: u16, h: u16) -> World {
        let mut rng = Rng(seed | 1);
        let mut view = gen_view(&mut rng, w, h);
        let mut basis = basis_of(&mut rng, w, h, 7);
        // The pair must fit: the map header of the view is the one of the basis.
        let mut m = basis.map().clone();
        m.game = game;
        m.basis_hash = basis_hash_of(&m);
        basis = BasisState::new(m).expect("basis");
        view.map.wrap_x = basis.map().wrap_x;
        view.map.wrap_y = basis.map().wrap_y;
        view.map.terrain_seed = basis.map().terrain_seed;
        let hash = ViewHash::of(&view);
        World {
            game,
            view,
            hash,
            basis,
            seq: 0,
            view_seq: 0,
            log: Vec::new(),
            rng,
            w,
            h,
        }
    }

    pub fn view_hash(&self) -> u64 {
        self.hash.value()
    }

    pub fn basis_hash(&self) -> u64 {
        self.basis.hash()
    }

    /// A new stream (`Attached { Fresh }`): sequence numbers start over.
    pub fn reset_stream(&mut self) {
        self.seq = 0;
        self.view_seq = 0;
        self.log.clear();
    }

    /// A batch of arbitrary messages with the next `seq` (turn state, chat, ...).
    pub fn push(&mut self, msgs: Vec<GameMsg>) -> ServerMsg {
        self.seq += 1;
        let m = ServerMsg::Game(GameBatch {
            seq: self.seq,
            msgs,
        });
        self.log.push(m.clone());
        m
    }

    pub fn map_basis(&self) -> MapBasis {
        self.basis.map().clone()
    }

    /// A batch with the keyframe, and `MapBasis` before it when the client does not hold the basis.
    pub fn keyframe(&mut self, with_basis: bool) -> ServerMsg {
        let mut msgs = Vec::new();
        if with_basis {
            msgs.push(GameMsg::MapBasis(self.map_basis()));
        }
        msgs.push(GameMsg::Keyframe(Keyframe {
            view: self.view.clone(),
            hash: self.hash.value(),
            basis_hash: self.basis.hash(),
        }));
        let m = self.push(msgs);
        self.view_seq = self.seq;
        m
    }

    /// The server moves to `new` and sends the diff.
    pub fn step_to(&mut self, new: WorldView) -> ServerMsg {
        self.step_damaged(new, None)
    }

    /// The next random state of the game, not yet sent.
    pub fn mutated(&mut self) -> WorldView {
        let mut v = mutate(&mut self.rng, &self.view);
        // `mutate` appends a notification per call; a view stays in key order across many steps.
        v.notifications.sort_by_key(|n| n.id);
        v
    }

    /// A random step of the game.
    pub fn step(&mut self) -> ServerMsg {
        let new = self.mutated();
        self.step_to(new)
    }

    /// An unrelated view of the same map: a big jump.
    pub fn jump(&mut self) -> ServerMsg {
        let mut new = gen_view(&mut self.rng, self.w, self.h);
        new.map.wrap_x = self.view.map.wrap_x;
        new.map.wrap_y = self.view.map.wrap_y;
        new.map.terrain_seed = self.view.map.terrain_seed;
        self.step_to(new)
    }

    /// The server's state goes on correctly; what leaves is damaged.
    pub fn step_damaged(&mut self, new: WorldView, damage: Option<Damage>) -> ServerMsg {
        let d = diff(&self.view, &new, None);
        self.send_diff(new, d, damage)
    }

    /// A step that also changes basis records (terraforming) and, optionally, the view.
    pub fn basis_step(&mut self, damage: Option<Damage>) -> ServerMsg {
        let mut d = diff(&self.view, &self.view.clone(), None);
        let n = self.basis.map().plots.len() as u64;
        let mut idx: Vec<u32> = (0..1 + self.rng.below(3))
            .map(|_| self.rng.below(n) as u32)
            .collect();
        idx.sort_unstable();
        idx.dedup();
        let fresh: Vec<(u32, protocol::PlotBasisView)> = idx
            .into_iter()
            .map(|i| (i, gen_basis_plot(&mut self.rng)))
            .collect();
        d.basis = protocol::diff_basis(self.basis.map(), &fresh);
        let view = self.view.clone();
        self.send_diff(view, d, damage)
    }

    fn send_diff(&mut self, new: WorldView, mut d: ViewDiff, damage: Option<Damage>) -> ServerMsg {
        // The true state moves on.
        let base = self.view_seq;
        let mut basis = self.basis.clone();
        let mut view = self.view.clone();
        let mut hash = self.hash.clone();
        protocol::apply(&mut view, &mut hash, &mut basis, &d).expect("a diff the server made");
        debug_assert_eq!(view, new);
        let basis_hash = (!d.basis.is_empty()).then(|| basis.hash());
        let mut delta = ViewDelta {
            base,
            diff: d.clone(),
            hash: hash.value(),
            basis_hash,
        };
        match damage {
            None => {}
            Some(Damage::ViewHash) => delta.hash ^= 0x5555,
            Some(Damage::Base) => delta.base = base.wrapping_add(7),
            Some(Damage::OutOfRange) => {
                d.plots.push((u32::MAX, Default::default()));
                delta.diff = d;
            }
            Some(Damage::BasisHash) => {
                delta.basis_hash = Some(delta.basis_hash.unwrap_or(0) ^ 0xAAAA);
            }
            Some(Damage::UnknownKey) => {
                delta.diff.units.remove.push((100, 100));
            }
        }
        self.view = view;
        self.hash = hash;
        self.basis = basis;
        let m = self.push(vec![GameMsg::Diff(delta)]);
        self.view_seq = self.seq;
        m
    }

    /// The server counts a batch the client never gets.
    pub fn lose_one(&mut self) {
        let _ = self.step();
    }

    /// The answer to `NeedKeyframe`.
    pub fn answer(&mut self, reason: KeyframeReason) -> ServerMsg {
        self.keyframe(reason == KeyframeReason::BasisMismatch)
    }

    /// The tail of the stream after `last_seq`.
    pub fn tail(&self, last_seq: u64) -> Vec<ServerMsg> {
        self.log.iter().skip(last_seq as usize).cloned().collect()
    }
}
