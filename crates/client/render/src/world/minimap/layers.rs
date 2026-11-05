//! What the minimap shows of each cell, and the layers drawn from it (world-overlays WOV3, RW-21).
//!
//! The original keeps a grid of bytes per player (`MinimapCell`: terrain, fog, culture owner) that
//! the game events fill; ours is the same, filled from the view: the terrain snapshot gives the
//! ground and the rivers, `set_fog` the fog, `set_owners` the culture. Only what the player has
//! revealed is known: an unrevealed cell is land by its byte (0) and fog by its flag, and the fog
//! layer is drawn last, over it (`cvMinimapRenderer::RenderMiniMap`).
//!
//! World units here are the original's (x east, y north), like `FowMask`'s.

use glam::Vec2;

use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH, ROW_STEP};
use crate::terrain::{FeatureKind, Fog, Relief, TerrainKind, TerrainSnapshot};
use crate::world::fow::state::FogInput;
use crate::world::overlay::OwnersInput;

/// The base of a cell (`terrain & 3` of `MinimapCell`: 0 land, 1 coast, 2 ocean).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Ground {
    #[default]
    Land,
    /// Drawn as shallow water over the water quad.
    Coast,
    /// Has no mesh: the water quad shows through.
    Ocean,
}

/// What the minimap knows of one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MinimapCell {
    pub ground: Ground,
    /// `m_ucFog & 1`: not explored.
    pub unexplored: bool,
    /// The owner (player id), if the cell has culture.
    pub owner: Option<u8>,
    pub ice: bool,
    pub mountain: bool,
    /// Bit i is the river on the edge towards `Dir` i (the snapshot's `rivers`).
    pub rivers: u8,
}

impl MinimapCell {
    /// What lies outside the map and on a cell nobody has seen.
    pub const UNEXPLORED: Self = Self {
        ground: Ground::Land,
        unexplored: true,
        owner: None,
        ice: false,
        mountain: false,
        rivers: 0,
    };
}

/// Changes of the grid that make the meshes again (`MINIMAP_DIRTY_*` of `cvMinimapSystem`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dirty(pub u8);

impl Dirty {
    pub const FOG: u8 = 1;
    pub const CULTURE: u8 = 2;
    /// The explored region grew (or shrank).
    pub const EXPLORED: u8 = 4;

    pub fn contains(self, bit: u8) -> bool {
        self.0 & bit != 0
    }

    pub fn any(self) -> bool {
        self.0 != 0
    }

    pub fn insert(&mut self, bit: u8) {
        self.0 |= bit;
    }

    pub fn take(&mut self) -> Self {
        std::mem::take(self)
    }
}

/// The grid of cells, row-major from y = 0 (south) like the snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MinimapMap {
    pub width: u32,
    pub height: u32,
    pub wrap_x: bool,
    cells: Vec<MinimapCell>,
    /// The colour of each owner (sRGB), by player id.
    colors: Vec<[u8; 3]>,
}

impl MinimapMap {
    /// The ground, rivers and ice of the snapshot; fog and owners as it has them.
    pub fn from_snapshot(snapshot: &TerrainSnapshot) -> Self {
        let cells = snapshot
            .cells()
            .iter()
            .map(|c| {
                let explored = c.fog != Fog::Unrevealed;
                MinimapCell {
                    ground: if !explored {
                        Ground::Land
                    } else {
                        match c.kind {
                            TerrainKind::Coast => Ground::Coast,
                            TerrainKind::Ocean => Ground::Ocean,
                            _ => Ground::Land,
                        }
                    },
                    unexplored: !explored,
                    owner: u8::try_from(c.owner).ok().filter(|_| explored),
                    ice: explored && c.feat == FeatureKind::Ice,
                    mountain: explored && c.relief == Relief::Mountain,
                    rivers: if explored { c.rivers } else { 0 },
                }
            })
            .collect();
        let used = snapshot
            .cells()
            .iter()
            .map(|c| c.owner)
            .max()
            .map_or(0, |m| usize::try_from(m).map_or(0, |m| m + 1));
        let colors = (0..used).map(|id| snapshot.owner_color(id as i8)).collect();
        Self {
            width: snapshot.width(),
            height: snapshot.height(),
            wrap_x: snapshot.wrap_x(),
            cells,
            colors,
        }
    }

    /// The fog of the feed. Returns what changed in the grid. A different size is ignored.
    pub fn set_fog(&mut self, fog: &FogInput, snapshot: Option<&TerrainSnapshot>) -> Dirty {
        let mut dirty = Dirty::default();
        if (fog.width, fog.height) != (self.width, self.height) {
            return dirty;
        }
        for (i, f) in fog.cells.iter().enumerate() {
            let unexplored = *f == Fog::Unrevealed;
            let cell = &mut self.cells[i];
            if cell.unexplored == unexplored {
                continue;
            }
            cell.unexplored = unexplored;
            dirty.insert(Dirty::FOG | Dirty::EXPLORED);
            // A cell that has just been revealed takes its ground from the snapshot.
            if let Some(c) = snapshot.and_then(|s| s.cells().get(i)) {
                cell.ground = if unexplored {
                    Ground::Land
                } else {
                    match c.kind {
                        TerrainKind::Coast => Ground::Coast,
                        TerrainKind::Ocean => Ground::Ocean,
                        _ => Ground::Land,
                    }
                };
                cell.ice = !unexplored && c.feat == FeatureKind::Ice;
                cell.mountain = !unexplored && c.relief == Relief::Mountain;
                cell.rivers = if unexplored { 0 } else { c.rivers };
                dirty.insert(Dirty::CULTURE);
            }
        }
        dirty
    }

    /// The owners of the feed.
    pub fn set_owners(&mut self, owners: &OwnersInput) -> Dirty {
        let mut dirty = Dirty::default();
        if (owners.width, owners.height) != (self.width, self.height) {
            return dirty;
        }
        for (cell, &o) in self.cells.iter_mut().zip(&owners.owners) {
            let owner = u8::try_from(o).ok().filter(|_| !cell.unexplored);
            if cell.owner != owner {
                cell.owner = owner;
                dirty.insert(Dirty::CULTURE);
            }
        }
        if self.colors != owners.colors {
            self.colors.clone_from(&owners.colors);
            dirty.insert(Dirty::CULTURE);
        }
        dirty
    }

    /// The colour of an owner (sRGB); grey past the list.
    pub fn owner_color(&self, owner: u8) -> [u8; 3] {
        self.colors
            .get(usize::from(owner))
            .copied()
            .unwrap_or([128, 128, 128])
    }

    /// The cell at `(x, y)`: the map wraps in X when it does, and what lies off the map is
    /// unexplored (`Fun_088a9b1c`).
    pub fn cell(&self, x: i32, y: i32) -> MinimapCell {
        if y < 0 || y >= self.height as i32 {
            return MinimapCell::UNEXPLORED;
        }
        let x = if self.wrap_x {
            x.rem_euclid(self.width as i32)
        } else if (0..self.width as i32).contains(&x) {
            x
        } else {
            return MinimapCell::UNEXPLORED;
        };
        self.cells[y as usize * self.width as usize + x as usize]
    }

    /// The smallest rectangle of explored cells, `(min, max)` in cell coordinates; `None` while
    /// nothing is explored.
    pub fn explored_bounds(&self) -> Option<((i32, i32), (i32, i32))> {
        let w = self.width as usize;
        let mut b: Option<((i32, i32), (i32, i32))> = None;
        for (i, c) in self.cells.iter().enumerate() {
            if c.unexplored {
                continue;
            }
            let (x, y) = ((i % w) as i32, (i / w) as i32);
            b = Some(match b {
                None => ((x, y), (x, y)),
                Some((lo, hi)) => ((lo.0.min(x), lo.1.min(y)), (hi.0.max(x), hi.1.max(y))),
            });
        }
        b
    }

    /// The extent of the world: every cell and its corners (`m_vWorldMin`, `m_vWorldMax`).
    pub fn world_extent(&self) -> (Vec2, Vec2) {
        // Cell (0, 0) is centred on x = 0; odd rows are half a hex further east.
        let min = Vec2::new(-HEX_WIDTH * 0.5, -CORNER_RADIUS);
        let max = Vec2::new(
            HEX_WIDTH * self.width as f32,
            ROW_STEP * (self.height.saturating_sub(1)) as f32 + CORNER_RADIUS,
        );
        (min, max)
    }

    /// The width of the world in X when the map wraps.
    pub fn world_width(&self) -> f32 {
        HEX_WIDTH * self.width as f32
    }
}

/// The centre of cell `(x, y)` in the original's basis (y north).
pub fn cell_pos(x: i32, y: i32) -> Vec2 {
    Vec2::new(
        HEX_WIDTH * (x as f32 + 0.5 * (y & 1) as f32),
        ROW_STEP * y as f32,
    )
}

/// Corner `i` of a hex relative to its centre, original's basis: angle 30 + 60 i degrees.
pub fn corner(i: usize) -> Vec2 {
    let a = f32::to_radians(30.0 + 60.0 * i as f32);
    Vec2::new(CORNER_RADIUS * a.cos(), CORNER_RADIUS * a.sin())
}

/// The layers of the minimap in the order `RenderMiniMap` draws them. The passes are those of
/// `MiniMapShader`'s effect (`kPass*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    /// Pass 7: a quad over the whole target.
    Water,
    /// Pass 4: coast cells.
    Shallows,
    /// Pass 1: land cells.
    Land,
    /// Pass 2: one mesh per owner, tinted.
    Culture,
    /// Pass 9: ice.
    Ice,
    /// Pass 3: rivers (per copy of the view in the original).
    Rivers,
    /// Pass 6: mountain sprites.
    Mountains,
    /// Pass 0: unexplored cells, last, over everything.
    Fog,
    /// Pass 8: the rectangle of the game camera.
    CameraRect,
}

impl Layer {
    /// Draw order.
    pub const ORDER: [Layer; 9] = [
        Layer::Water,
        Layer::Shallows,
        Layer::Land,
        Layer::Culture,
        Layer::Ice,
        Layer::Rivers,
        Layer::Mountains,
        Layer::Fog,
        Layer::CameraRect,
    ];

    /// The pass of the original's effect.
    pub fn pass(self) -> u16 {
        match self {
            Layer::Fog => 0,
            Layer::Land => 1,
            Layer::Culture => 2,
            Layer::Rivers => 3,
            Layer::Shallows => 4,
            Layer::Mountains => 6,
            Layer::Water => 7,
            Layer::CameraRect => 8,
            Layer::Ice => 9,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::TerrainCell;
    use crate::terrain::test_util::{land, map};

    fn revealed(kind: TerrainKind) -> TerrainCell {
        TerrainCell {
            kind,
            fog: Fog::Visible,
            ..land()
        }
    }

    #[test]
    fn the_ground_follows_the_terrain_and_unexplored_cells_are_land_under_fog() {
        let snap = map(3, 1, |x, _| match x {
            0 => revealed(TerrainKind::Grass),
            1 => revealed(TerrainKind::Coast),
            _ => TerrainCell::default(),
        });
        let m = MinimapMap::from_snapshot(&snap);
        assert_eq!(m.cell(0, 0).ground, Ground::Land);
        assert_eq!(m.cell(1, 0).ground, Ground::Coast);
        assert!(m.cell(2, 0).unexplored);
        assert_eq!(m.cell(2, 0).ground, Ground::Land);
        assert!(!m.cell(0, 0).unexplored);
    }

    #[test]
    fn off_the_map_is_unexplored_and_the_map_wraps_in_x_when_it_does() {
        let snap = map(4, 2, |_, _| revealed(TerrainKind::Grass));
        let m = MinimapMap::from_snapshot(&snap);
        assert_eq!(m.cell(0, 5), MinimapCell::UNEXPLORED);
        assert_eq!(m.cell(0, -1), MinimapCell::UNEXPLORED);
        // `map` does not wrap: past the edge is unexplored.
        assert!(!m.wrap_x);
        assert_eq!(m.cell(4, 0), MinimapCell::UNEXPLORED);
        let wrapped = MinimapMap {
            wrap_x: true,
            ..m.clone()
        };
        assert!(!wrapped.cell(4, 0).unexplored);
        assert!(!wrapped.cell(-1, 1).unexplored);
    }

    #[test]
    fn the_explored_bounds_hold_every_explored_cell() {
        let snap = map(5, 4, |x, y| {
            if (1..=3).contains(&x) && (1..=2).contains(&y) {
                revealed(TerrainKind::Plains)
            } else {
                TerrainCell::default()
            }
        });
        let m = MinimapMap::from_snapshot(&snap);
        assert_eq!(m.explored_bounds(), Some(((1, 1), (3, 2))));
        let none = MinimapMap::from_snapshot(&map(2, 2, |_, _| TerrainCell::default()));
        assert_eq!(none.explored_bounds(), None);
    }

    #[test]
    fn a_revealed_cell_marks_fog_and_the_explored_region_dirty() {
        let snap = map(2, 1, |_, _| TerrainCell::default());
        let mut m = MinimapMap::from_snapshot(&snap);
        let mut fog = FogInput::from_snapshot(&snap);
        assert!(!m.set_fog(&fog, Some(&snap)).any(), "nothing changed");
        fog.cells[1] = Fog::Visible;
        let d = m.set_fog(&fog, None);
        assert!(d.contains(Dirty::FOG) && d.contains(Dirty::EXPLORED));
        assert!(!m.cell(1, 0).unexplored);
        assert!(m.cell(0, 0).unexplored);
    }

    #[test]
    fn owners_mark_culture_dirty_only_where_something_changed() {
        let snap = map(2, 1, |_, _| revealed(TerrainKind::Grass));
        let mut m = MinimapMap::from_snapshot(&snap);
        let mut owners = OwnersInput::new(2, 1, vec![-1, -1], vec![]).unwrap();
        assert!(!m.set_owners(&owners).any());
        owners.owners[0] = 0;
        owners.colors = vec![[200, 20, 20]];
        let d = m.set_owners(&owners);
        assert!(d.contains(Dirty::CULTURE));
        assert_eq!(m.cell(0, 0).owner, Some(0));
        assert_eq!(m.owner_color(0), [200, 20, 20]);
        assert_eq!(m.owner_color(9), [128, 128, 128]);
        assert!(!m.set_owners(&owners).any(), "the same owners again");
    }

    #[test]
    fn the_world_extent_holds_every_hex() {
        let m = MinimapMap::from_snapshot(&map(4, 3, |_, _| revealed(TerrainKind::Grass)));
        let (lo, hi) = m.world_extent();
        for y in 0..3 {
            for x in 0..4 {
                for i in 0..6 {
                    let p = cell_pos(x, y) + corner(i);
                    assert!(p.x >= lo.x - 1e-3 && p.x <= hi.x + 1e-3, "{p:?} x");
                    assert!(p.y >= lo.y - 1e-3 && p.y <= hi.y + 1e-3, "{p:?} y");
                }
            }
        }
    }

    #[test]
    fn layers_are_drawn_water_first_and_fog_before_the_camera() {
        assert_eq!(Layer::ORDER[0], Layer::Water);
        let fog = Layer::ORDER.iter().position(|l| *l == Layer::Fog).unwrap();
        assert_eq!(Layer::ORDER[fog + 1], Layer::CameraRect);
        // The passes of the effect.
        assert_eq!(Layer::ORDER.map(Layer::pass), [7, 4, 1, 2, 9, 3, 6, 0, 8]);
    }
}
