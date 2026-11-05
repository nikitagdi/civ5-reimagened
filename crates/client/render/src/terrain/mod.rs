//! Terrain for the map: the render-side snapshot of the map and the CPU
//! mesh built from it. No GPU here, so everything is testable; `render` does not know `protocol`
//! (R6), `client-app/world_bridge.rs` converts a `MapView` into a [`TerrainSnapshot`].

pub mod features;
pub mod fog;
pub mod grid;
pub mod height;
pub mod mesh;
pub mod rivers;
pub mod texture_array;
pub mod water;

pub use height::{relief_height, terrain_height, terrain_height_at};
pub use mesh::{
    HILL_TOP, LAND_HEIGHT, MOUNTAIN_TOP, TRIANGLE_BUDGET, TerrainMesh, TerrainVertex, VertexPart,
    WATER_HEIGHT,
};

use glam::{Vec2, Vec3};

use crate::blend::BlendMesh;
use crate::hexgrid::{Dir, GridShape, cell_center, corner_offset, edge_corners};

/// Shape of a cell's surface. `Flat` is plains-like land at [`LAND_HEIGHT`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Relief {
    #[default]
    Flat,
    Hill,
    Mountain,
    Water,
}

/// Terrain material. Lakes are `Coast` (SIMP-008). `Unknown` is a catalog entry the bridge does
/// not recognise.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum TerrainKind {
    #[default]
    Unknown,
    Grass,
    Plains,
    Desert,
    Tundra,
    Snow,
    Coast,
    Ocean,
}

/// Fog state of a cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Fog {
    #[default]
    Unrevealed,
    /// Revealed but not currently seen.
    Hidden,
    Visible,
}

/// Terrain feature of a cell as far as the renderer draws it. `Other` is a catalog
/// feature without art here (natural wonders: SV icon, SIMP-012).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FeatureKind {
    #[default]
    None,
    Forest,
    Jungle,
    Marsh,
    FloodPlains,
    Oasis,
    Ice,
    Fallout,
    Other,
}

impl FeatureKind {
    /// Features with an art-table record, in a fixed order.
    pub const DRAWN: [FeatureKind; 7] = [
        Self::Forest,
        Self::Jungle,
        Self::Marsh,
        Self::FloodPlains,
        Self::Oasis,
        Self::Ice,
        Self::Fallout,
    ];

    /// Type string in the art table (`feature/<type>/<style>`) and in the catalog.
    pub fn type_name(self) -> Option<&'static str> {
        Some(match self {
            Self::Forest => "FEATURE_FOREST",
            Self::Jungle => "FEATURE_JUNGLE",
            Self::Marsh => "FEATURE_MARSH",
            Self::FloodPlains => "FEATURE_FLOOD_PLAINS",
            Self::Oasis => "FEATURE_OASIS",
            Self::Ice => "FEATURE_ICE",
            Self::Fallout => "FEATURE_FALLOUT",
            Self::None | Self::Other => return None,
        })
    }

    /// Kind of a catalog `FEATURE_*` type; unknown names are `Other`.
    pub fn from_type_name(name: &str) -> Self {
        Self::DRAWN
            .into_iter()
            .find(|k| k.type_name() == Some(name))
            .unwrap_or(Self::Other)
    }
}

/// One map cell as the renderer needs it. `rivers` bit i is the edge towards `Dir` i; `owner` is
/// `-1` for none; `feature` is the raw feature id (`-1` for none) and `feat` its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainCell {
    pub relief: Relief,
    pub kind: TerrainKind,
    pub art: u8,
    pub fog: Fog,
    pub rivers: u8,
    pub owner: i8,
    pub feature: i16,
    pub feat: FeatureKind,
}

impl Default for TerrainCell {
    /// An unrevealed cell.
    fn default() -> Self {
        Self {
            relief: Relief::Flat,
            kind: TerrainKind::Unknown,
            art: 0,
            fog: Fog::Unrevealed,
            rivers: 0,
            owner: -1,
            feature: -1,
            feat: FeatureKind::None,
        }
    }
}

impl TerrainCell {
    pub fn is_revealed(&self) -> bool {
        self.fog != Fog::Unrevealed
    }

    /// Relief actually drawn: an unrevealed cell is flat.
    pub fn drawn_relief(&self) -> Relief {
        if self.is_revealed() {
            self.relief
        } else {
            Relief::Flat
        }
    }
}

/// The whole map for the renderer, row-major from y = 0 (south), like the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerrainSnapshot {
    width: u32,
    height: u32,
    wrap_x: bool,
    cells: Vec<TerrainCell>,
    /// sRGB colour per owner id; owners past the end get [`fallback_owner_color`].
    owner_colors: Vec<[u8; 3]>,
}

impl TerrainSnapshot {
    /// `None` if `cells.len() != width * height`.
    pub fn new(width: u32, height: u32, wrap_x: bool, cells: Vec<TerrainCell>) -> Option<Self> {
        (cells.len() == width as usize * height as usize).then_some(Self {
            width,
            height,
            wrap_x,
            cells,
            owner_colors: Vec::new(),
        })
    }

    /// Border colours by owner id (index = player id).
    pub fn with_owner_colors(mut self, colors: Vec<[u8; 3]>) -> Self {
        self.owner_colors = colors;
        self
    }

    /// Colour of an owner's borders (sRGB); `owner` must be `>= 0`.
    pub fn owner_color(&self, owner: i8) -> [u8; 3] {
        usize::try_from(owner)
            .ok()
            .and_then(|i| self.owner_colors.get(i))
            .copied()
            .unwrap_or_else(|| fallback_owner_color(owner))
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn wrap_x(&self) -> bool {
        self.wrap_x
    }

    pub fn cells(&self) -> &[TerrainCell] {
        &self.cells
    }

    pub fn shape(&self) -> GridShape {
        GridShape {
            width: self.width as i32,
            height: self.height as i32,
            wrap_x: self.wrap_x,
        }
    }

    pub fn cell(&self, x: i32, y: i32) -> Option<&TerrainCell> {
        self.index(x, y).map(|i| &self.cells[i])
    }

    pub(crate) fn index(&self, x: i32, y: i32) -> Option<usize> {
        let in_map = (0..self.width as i32).contains(&x) && (0..self.height as i32).contains(&y);
        in_map.then(|| y as usize * self.width as usize + x as usize)
    }

    /// True if the mesh of `other` would be identical: same size, and the same drawn relief in
    /// every cell. Material and fog do not change the geometry.
    pub fn same_geometry(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.wrap_x == other.wrap_x
            && self
                .cells
                .iter()
                .zip(&other.cells)
                .all(|(a, b)| a.drawn_relief() == b.drawn_relief())
    }
}

/// Colour for an owner the snapshot has no colour for.
fn fallback_owner_color(owner: i8) -> [u8; 3] {
    const PALETTE: [[u8; 3]; 4] = [[200, 60, 60], [60, 110, 200], [220, 190, 60], [70, 180, 90]];
    PALETTE[owner.unsigned_abs() as usize % PALETTE.len()]
}

/// Geometry of the edge of cell `(x, y)` facing `dir`, in world `(X, Z)` of the unwrapped cell.
pub(crate) struct Edge {
    pub a: Vec2,
    pub b: Vec2,
    /// Unit vector from the edge towards the cell centre.
    pub inward: Vec2,
}

pub(crate) fn edge(x: i32, y: i32, dir: Dir) -> Edge {
    let c = cell_center(x, y);
    let (ia, ib) = edge_corners(dir);
    let (oa, ob) = (corner_offset(ia), corner_offset(ib));
    Edge {
        a: c + oa,
        b: c + ob,
        inward: -((oa + ob) * 0.5).normalize_or_zero(),
    }
}

/// Point `(X, Z)` lifted to height `y`.
pub(crate) fn lift(p: Vec2, y: f32) -> Vec3 {
    Vec3::new(p.x, y, p.y)
}

/// Everything the blend pipeline draws over the map besides the borders (`world::overlay::borders`): rivers, grid.
pub fn overlay_mesh(snapshot: &TerrainSnapshot) -> BlendMesh {
    let mut mesh = BlendMesh::default();
    rivers::build(snapshot, &mut mesh);
    grid::build(snapshot, &mut mesh);
    mesh
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;

    pub fn land() -> TerrainCell {
        TerrainCell {
            relief: Relief::Flat,
            kind: TerrainKind::Grass,
            fog: Fog::Visible,
            ..TerrainCell::default()
        }
    }

    pub fn water() -> TerrainCell {
        TerrainCell {
            relief: Relief::Water,
            kind: TerrainKind::Ocean,
            ..land()
        }
    }

    /// `w x h` map filled by `f(x, y)`.
    pub fn map(w: u32, h: u32, f: impl Fn(i32, i32) -> TerrainCell) -> TerrainSnapshot {
        let cells = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect();
        TerrainSnapshot::new(w, h, false, cells).unwrap()
    }
}
