//! `WorldFacts`: whole-truth facts about the map for server mods (spike T01, `spikes/owners.md`).
//!
//! Values only. The host builds a facade for the duration of one call; nothing here names a host type.

use crate::ids::{PlayerId, Seat};

/// Index of a plot: `y * width + x`.
pub type PlotIdx = u32;

/// Shape of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapShape {
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub wrap_y: bool,
}

impl MapShape {
    pub fn plot_count(self) -> u32 {
        u32::from(self.width) * u32::from(self.height)
    }

    /// Plot index of `(x, y)` with wrapping applied on wrapping axes; `None` when off the map.
    pub fn index(self, x: i16, y: i16) -> Option<PlotIdx> {
        let (w, h) = (i32::from(self.width), i32::from(self.height));
        if w == 0 || h == 0 {
            return None;
        }
        let (mut x, mut y) = (i32::from(x), i32::from(y));
        if self.wrap_x {
            x = x.rem_euclid(w);
        }
        if self.wrap_y {
            y = y.rem_euclid(h);
        }
        if (0..w).contains(&x) && (0..h).contains(&y) {
            Some((y * w + x) as PlotIdx)
        } else {
            None
        }
    }
}

/// Who owns a plot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Owner {
    /// Nobody.
    None,
    /// A human seat.
    Seat(Seat),
    /// A non-human player (major AI, city state, barbarians).
    Npc(PlayerId),
    /// The host's table is not current (not yet received, or the worker restarted). Refuse edits.
    Unknown,
}

/// Terrain class of a plot, from the fog-free basis (identical for every seat).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlotClass {
    Land,
    Hills,
    Mountain,
    Water,
    Lake,
    Ice,
}

/// Whole-truth map facts. Object safe; reads are in-memory and synchronous.
pub trait WorldFacts {
    fn map(&self) -> MapShape;
    /// The current game turn.
    fn turn(&self) -> i32;
    /// Plot at `(x, y)`, wrapping like the map does; `None` off the map.
    fn plot_at(&self, x: i16, y: i16) -> Option<PlotIdx>;
    /// Owner right now (not a fogged view). Out-of-range plots are `Owner::None`.
    fn owner(&self, p: PlotIdx) -> Owner;
    /// Terrain class. Out-of-range plots are `PlotClass::Water`.
    fn class(&self, p: PlotIdx) -> PlotClass;
    /// The player a human seat plays; `None` for a seat that is not in the game.
    fn seat_player(&self, seat: Seat) -> Option<PlayerId>;

    /// Coordinates of a plot index.
    fn plot_xy(&self, p: PlotIdx) -> (i16, i16) {
        let w = u32::from(self.map().width).max(1);
        ((p % w) as i16, (p / w) as i16)
    }

    /// True when `seat` owns the plot right now.
    fn owned_by(&self, p: PlotIdx, seat: Seat) -> bool {
        self.owner(p) == Owner::Seat(seat)
    }
}
