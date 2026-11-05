//! The coast grid (WW-3a): `CoastGrid::Init` and its four kinds of job.
//! The grid is filled from the flags of the hexes (`InitGridJob`), the edges between land,
//! shallow and deep water are found (`DetectCoastJob`), linked into coastlines
//! (`GenerateCoastConnectivityJob`) and the segments are smoothed and pushed away from the
//! mountains in ten passes (`SplinePreProcessJob`). The only input is the class of every hex;
//! no step depends on the order the hexes are visited in.
//!
//! Hexes are cube vectors `[x, y, z]` with `z = -(x + y)`; a hex is the cell `(col, row)` with
//! `col = x + (y >> 1)` and `row = y` (the offset layout of the map, odd rows half a hex east).
//! The segments live in the texel plane of the field.
//!
//! Open: the table `HEXVec_i::TRANSLATIONS` and the matrix `g_HexToTex` are filled at run time and
//! empty in the dump, so the six directions are the ones that make the segments run with the land
//! on the left (the sign of every offset of the levels depends on that), and the texel of a hex
//! is `TerrainFrame::hex_to_tex`.
// TODO(RW-10): check `HEXVec_i::TRANSLATIONS` and `g_HexToTex` against a run; the order of the
// directions decides which hex keeps the geometry of a side (and so where a coastline starts).
// TODO(RW-6): the transform of the game grid (`m_kTransform`, `m_mxForward`) is the identity here.

use glam::Vec2;

use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
use crate::world::terrain::sampler::TerrainFrame;

use super::bspline;
use super::styles::kind;

/// A hex in cube coordinates.
pub type Hex = [i32; 3];

/// The six neighbours: the first three are the directions whose side carries the geometry, the
/// last three are their opposites. The ring around a hex in the order of the angle is
/// 4, 0, 5, 1, 3, 2 (counter-clockwise), which is what `aaiSideNeighbors` and `ms_CoastConnect`
/// of the original describe. The order is the original's: the other owner triple (the one we had)
/// keeps the same side of the land but another owner cell, scan start and draw of `rand` per point,
/// so the noised coast was another realisation than the original's for the same seed (S2: a river
/// mouth 0.235 too high on the shore; G-T1 within 1 texel 0.77, now 0.90).
// re: HEXVec_i::TRANSLATIONS 0x0a3c8bf8, filled by _INIT_148 0x0848cd21 (re-b16 C1-3); CoastGrid::Init
// 0x088e4cd0 (`aaiSideNeighbors`)
pub const TRANSLATIONS: [Hex; 6] = [
    [0, 1, -1],
    [-1, 0, 1],
    [1, -1, 0],
    [0, -1, 1],
    [1, 0, -1],
    [-1, 1, 0],
];

/// Per direction 0..2 the neighbours that touch the two ends of the side: before, across, after.
// re: CoastGrid::Init 0x088e4cd0 (cvCoastGrid `aaiSideNeighbors[3][3]`)
const SIDE_NEIGHBORS: [[usize; 3]; 3] = [[4, 0, 5], [5, 1, 3], [3, 2, 4]];

/// Per slot the hex directions that touch the two ends of its side.
// re: CoastGrid::ms_CoastConnect 0x092141e4
const COAST_CONNECT: [[u32; 2]; 3] = [[4, 5], [5, 3], [3, 4]];

/// `m_ucTerrainClass`.
pub const CLASS_LAND: u8 = 0;
pub const CLASS_SHALLOW: u8 = 1;
pub const CLASS_DEEP: u8 = 2;

/// Edge ids: the base of the four kinds of edge, `id % 6` is the direction.
// re: DetectCoastJob 0x088e6484 (kEdgeShoreLand ... kEdgeShelfDeep)
pub const EDGE_SHORE_LAND: u8 = 0;
pub const EDGE_SHORE_SHALLOW: u8 = 6;
pub const EDGE_SHELF_SHALLOW: u8 = 12;
pub const EDGE_SHELF_DEEP: u8 = 18;

/// No peer on a link.
pub const NO_PEER: u8 = 0xff;

/// Flags of a hex of the pattern grid (`HexPatternCell::m_uFlags`) the grid looks at.
mod flags {
    // re: InitGridJob 0x088e6b3c (kLandFlags, kShallowFlags, kWaterKindFlags, kFlag*)
    pub const LAND: u32 = 0xf3ef_9fff;
    pub const SHALLOW: u32 = 0x0c10_2000;
    pub const WATER_KIND: u32 = 0x0010_6000;
    pub const GRASS: u32 = 0x1;
    pub const PLAINS: u32 = 0x2;
    pub const DESERT: u32 = 0x4;
    pub const TUNDRA: u32 = 0x8;
    pub const SNOW: u32 = 0x10;
    pub const GRASS_HILLS: u32 = 0x20;
    pub const PLAINS_HILLS: u32 = 0x40;
    pub const DESERT_HILLS: u32 = 0x80;
    pub const TUNDRA_HILLS: u32 = 0x100;
    pub const SNOW_HILLS: u32 = 0x200;
    pub const MOUNTAIN: u32 = 0x400;
    // The classes of the pieces that the land mask lets through but that have no kind: a natural
    // wonder on land. (0x1000 is in `LAND`.)
    #[cfg(test)]
    pub const WONDER: u32 = 0x1000;
    // The classes of water: coast, ocean, the reef and the Gibraltar class.
    pub const COAST: u32 = 0x2000;
    #[cfg(test)]
    pub const OCEAN: u32 = 0x4000;
    #[cfg(test)]
    pub const REEF: u32 = 0x0010_0000;
    #[cfg(test)]
    pub const GIBRALTAR: u32 = 0x0400_0000;
}

/// `m_ucLandKind` of a hex that is water, and of a mountain.
const KIND_WATER: u8 = kind::SHALLOWS;
const KIND_MOUNTAIN: u8 = kind::MOUNTAIN;

/// Texels a mountain shore is pushed along its normal.
// re: SplinePreProcessJob 0x088e578e (cvCoastGrid kMountainShift)
const MOUNTAIN_SHIFT: f32 = 4.0;

/// The passes of `SplinePreProcessJob`: 1 smooth, 2 commit, 3 shift the mountain edges.
// re: SplinePreProcessJob::ms_aStateList 0x0a30fca0 / ms_nStateList 0x0a30fcc8
const STATE_LIST: [u8; 10] = [1, 2, 3, 2, 1, 2, 3, 2, 1, 2];

/// A part of a hex side in the texel plane: the middle half of the side.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Segment {
    pub first: Vec2,
    pub second: Vec2,
}

/// Where the coastline goes on from a segment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Link {
    pub marked: bool,
    /// Slot of the edge in the peer cell, [`NO_PEER`] for none.
    pub peer_edge: u8,
    /// The column of the peer cell as the hex gives it: it can lie outside the map when the map wraps.
    pub col: i32,
    pub row: i32,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            marked: false,
            peer_edge: NO_PEER,
            col: 0,
            row: 0,
        }
    }
}

/// One hex of the coast grid.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cell {
    pub next: [Link; 3],
    pub prev: [Link; 3],
    pub segments: [Segment; 3],
    pub terrain_class: u8,
    pub mountain_edge: [u8; 3],
    pub land_kind: u8,
    pub edge_kind: [u8; 3],
    pub num_back: u8,
    pub back_edges: [u8; 3],
    pub num_edges: u8,
    pub edges: [u8; 3],
}

/// A segment of a coastline: the hex and the slot among its edges, `-1` for none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Handle {
    pub hex: Hex,
    pub edge: i32,
}

fn add(a: Hex, b: Hex) -> Hex {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// The hex of the cell `(col, row)`.
pub fn hex_of_cell(col: i32, row: i32) -> Hex {
    let x = col - (row >> 1);
    [x, row, -(x + row)]
}

/// The cell `(col, row)` of a hex, columns as the hex gives them (no wrap).
pub fn cell_of_hex(hex: Hex) -> (i32, i32) {
    ((hex[1] >> 1) + hex[0], hex[1])
}

/// The flags of a hex of the pattern grid: the class of the piece the hex takes, as the bridge of the
/// matcher makes it. A natural wonder overrides the class of the terrain after the terrain test, so a
/// wonder on water other than the reef and Gibraltar (Krakatoa) is land for the coast; El Dorado and
/// the Fountain of Youth keep the class of their terrain.
// re: InitGridJob 0x088e6b3c (the flags tested); StartInitTerrainFromGameplayMap (LoadGameState.cpp:
// 148-203, re-b16 C1-2): the class the coast grid gets is the pattern cell's own
pub fn pattern_flags(plot: &PlotBasis) -> u32 {
    crate::world::terrain::build::bridge::plot_cell(plot).0
}
fn same_class(id: u32, corner: u32) -> bool {
    (id < 12 && corner < 12) || (id > 11 && corner > 11)
}

/// The grid of the coast.
pub struct CoastGrid {
    width: i32,
    height: i32,
    wrap: bool,
    /// Columns and rows of padding round the map, all of them coast (`kCellCoast`): 2 rows above
    /// and below, 2 columns each side on a map that does not wrap.
    pad: (i32, i32),
    cells: Vec<Cell>,
    outside: Cell,
    wrap_shift: Vec2,
    templates: [Segment; 3],
    frame: TerrainFrame,
}

impl CoastGrid {
    /// `CoastGrid::Init`: all four steps, for the map of `basis`. The map wraps along X when
    /// `basis.wrap_x` (the original clears `field_9c` when `g_eWrapMode` is 0).
    // re: CoastGrid::Init 0x088e4cd0; TerrainSystem::Init (TerrainSystem.cpp:717-720)
    // The grid is the padded game grid of `LoadGameState` (LoadGameState.cpp:102-131, re-b16 C1-1):
    // land at the border of a map gets its shore and the deep sea its shelf there, every line is
    // closed. Our cells keep the columns and rows of the game, the padding at negative ones and past
    // the map.
    pub fn new(basis: &TerrainBasis, frame: &TerrainFrame) -> Self {
        let mut grid = Self::empty(basis.width.into(), basis.height.into(), basis.wrap_x, frame);
        grid.init_cells(|col, row| {
            let on_map = (0..i32::from(basis.width)).contains(&col)
                && (0..i32::from(basis.height)).contains(&row);
            if on_map {
                basis.plot(col, row).map_or(0, pattern_flags)
            } else {
                flags::COAST
            }
        });
        grid.detect();
        grid.connect();
        grid.preprocess();
        grid
    }

    /// The grid with its geometry (templates, wrap shift) and no edges.
    fn empty(width: i32, height: i32, wrap: bool, frame: &TerrainFrame) -> Self {
        // The texel offset of the six neighbours; the map to texels is affine, so the origin
        // hex does not matter.
        let origin = frame.hex_to_tex(0, 0);
        let mut offsets = [Vec2::ZERO; 6];
        for (o, t) in offsets.iter_mut().zip(TRANSLATIONS) {
            let (col, row) = cell_of_hex(t);
            *o = frame.hex_to_tex(col, row) - origin;
        }
        let mut templates = [Segment::default(); 3];
        for (t, n) in templates.iter_mut().zip(SIDE_NEIGHBORS) {
            let (before, across, after) = (offsets[n[0]], offsets[n[1]], offsets[n[2]]);
            // The corners of the side are a third of the way to the neighbour pairs; the segment
            // is the middle half.
            let a = (before + across) / 3.0;
            let b = (across + after) / 3.0;
            t.first = a * 0.75 + b * 0.25;
            t.second = a * 0.25 + b * 0.75;
        }
        let pad = (if wrap { 0 } else { 2 }, 2);
        let cells = ((width + 2 * pad.0) * (height + 2 * pad.1)).max(0) as usize;
        Self {
            width,
            height,
            wrap,
            pad,
            cells: vec![Cell::default(); cells],
            outside: Cell::default(),
            wrap_shift: frame.hex_to_tex(width, 0) - origin,
            templates,
            frame: *frame,
        }
    }

    pub fn width(&self) -> i32 {
        self.width
    }

    pub fn height(&self) -> i32 {
        self.height
    }

    /// The columns of the grid with its padding (from -2 on a map that does not wrap).
    pub fn cols(&self) -> std::ops::Range<i32> {
        -self.pad.0..self.width + self.pad.0
    }

    /// The rows of the grid with its padding (from -2).
    pub fn rows(&self) -> std::ops::Range<i32> {
        -self.pad.1..self.height + self.pad.1
    }

    /// Whether the columns wrap (`field_9c`).
    pub fn wraps(&self) -> bool {
        self.wrap
    }

    /// The texel shift of one width of the map.
    pub fn wrap_shift(&self) -> Vec2 {
        self.wrap_shift
    }

    /// The templates of the sides toward the directions 0..2, relative to the centre of a hex.
    pub fn templates(&self) -> &[Segment; 3] {
        &self.templates
    }

    fn slot(&self, col: i32, row: i32) -> Option<usize> {
        if (!self.wrap && !self.cols().contains(&col)) || !self.rows().contains(&row) {
            return None;
        }
        let col = if self.wrap {
            col.rem_euclid(self.width)
        } else {
            col
        };
        let stride = self.width + 2 * self.pad.0;
        Some(((row + self.pad.1) * stride + col + self.pad.0) as usize)
    }

    /// The cell at a column and a row; a place outside gives the empty outside cell.
    // re: Fun_088e5718 0x088e5718 (`m_kOutsideCell`, the wrap of the column)
    pub fn cell(&self, col: i32, row: i32) -> &Cell {
        self.slot(col, row)
            .map_or(&self.outside, |i| &self.cells[i])
    }

    /// The cell of a hex.
    // re: Fun_088e56a2 0x088e56a2
    pub fn cell_of(&self, hex: Hex) -> &Cell {
        let (col, row) = cell_of_hex(hex);
        self.cell(col, row)
    }

    /// `InitGridJob` for every row. The cell variable runs on from one column to the next, so a
    /// water hex with no kind flag keeps the kind of the hex before it, as in the original.
    // re: InitGridJob::Execute 0x088e6b3c
    fn init_cells(&mut self, flags_of: impl Fn(i32, i32) -> u32) {
        for row in self.rows() {
            let mut cell = Cell::default();
            for col in self.cols() {
                cell.terrain_class = CLASS_DEEP;
                let f = flags_of(col, row);
                if f & flags::LAND != 0 {
                    cell.terrain_class = CLASS_LAND;
                    cell.land_kind = 0;
                } else {
                    if f & flags::SHALLOW != 0 {
                        cell.terrain_class = CLASS_SHALLOW;
                    }
                    if f & flags::WATER_KIND != 0 {
                        cell.land_kind = KIND_WATER;
                    }
                }
                cell.mountain_edge = [0; 3];
                cell.edge_kind = [0; 3];
                let kinds = [
                    (flags::GRASS, kind::GRASS),
                    (flags::GRASS_HILLS, kind::GRASS_HILL),
                    (flags::PLAINS, kind::PLAINS),
                    (flags::PLAINS_HILLS, kind::PLAINS_HILL),
                    (flags::SNOW, kind::SNOW),
                    (flags::SNOW_HILLS, kind::SNOW_HILL),
                    (flags::DESERT, kind::DESERT),
                    (flags::DESERT_HILLS, kind::DESERT_HILL),
                    (flags::TUNDRA, kind::TUNDRA),
                    (flags::TUNDRA_HILLS, kind::TUNDRA_HILL),
                ];
                if f & flags::MOUNTAIN != 0 {
                    cell.land_kind = KIND_MOUNTAIN;
                    cell.mountain_edge = [2; 3];
                } else if let Some((_, k)) = kinds.iter().find(|(bit, _)| f & bit != 0) {
                    cell.land_kind = *k;
                }
                let dest = self.slot(col, row).expect("a cell of the grid");
                self.cells[dest] = cell;
            }
        }
    }

    /// `DetectCoastJob` for every row.
    // re: DetectCoastJob::Execute 0x088e6484
    fn detect(&mut self) {
        // Whether the cell of a hex is in the map (`IsOffsetInGrid`, 0x087bab18).
        let in_grid = |g: &Self, col: i32, row: i32| -> bool { g.slot(col, row).is_some() };
        // The coast edge between a cell and its neighbour in the direction `dir`: id and kind.
        let classify = |cell: &Cell, neighbor: &Cell, dir: u8| -> Option<(u8, u8)> {
            match (cell.terrain_class, neighbor.terrain_class) {
                (CLASS_DEEP, CLASS_SHALLOW) => Some((EDGE_SHELF_DEEP + dir, KIND_WATER)),
                (CLASS_SHALLOW, CLASS_LAND) => Some((EDGE_SHORE_SHALLOW + dir, neighbor.land_kind)),
                (CLASS_SHALLOW, CLASS_DEEP) => Some((EDGE_SHELF_SHALLOW + dir, KIND_WATER)),
                (CLASS_LAND, CLASS_SHALLOW) => Some((EDGE_SHORE_LAND + dir, cell.land_kind)),
                _ => None,
            }
        };
        for row in self.rows() {
            for col in self.cols() {
                let hex = hex_of_cell(col, row);
                let Some(index) = self.slot(col, row) else {
                    continue;
                };
                let tex = self.frame.hex_to_tex(col, row);
                let mut cell = self.cells[index];
                // Directions 0..2: the geometry of the side is kept in this cell.
                for dir in 0..3u8 {
                    let (nc, nr) = cell_of_hex(add(hex, TRANSLATIONS[usize::from(dir)]));
                    if !in_grid(self, nc, nr) {
                        continue;
                    }
                    let neighbor = *self.cell(nc, nr);
                    let slot = usize::from(cell.num_edges);
                    if let Some((id, kind)) = classify(&cell, &neighbor, dir) {
                        let t = self.templates[usize::from(dir)];
                        cell.edges[slot] = id;
                        cell.segments[slot] = Segment {
                            first: t.first + tex,
                            second: t.second + tex,
                        };
                        cell.edge_kind[slot] = kind;
                        cell.num_edges += 1;
                    } else {
                        cell.segments[slot] = Segment::default();
                    }
                }
                // Directions 3..5: the neighbour keeps the geometry, this cell only lists the edge.
                for dir in 3..6u8 {
                    let (nc, nr) = cell_of_hex(add(hex, TRANSLATIONS[usize::from(dir)]));
                    if !in_grid(self, nc, nr) {
                        continue;
                    }
                    let neighbor = *self.cell(nc, nr);
                    if let Some((id, _)) = classify(&cell, &neighbor, dir) {
                        cell.back_edges[usize::from(cell.num_back)] = id;
                        cell.num_back += 1;
                    }
                }
                self.cells[index] = cell;
            }
        }
    }

    /// `GenerateCoastConnectivityJob` for every row: the link of every edge to the next and the
    /// previous one along the coastline. Edges seen from the outer side (`id % 12 >= 6`) run the
    /// other way, so their links swap.
    // re: GenerateCoastConnectivityJob::Execute 0x088e6038
    fn connect(&mut self) {
        let mut links = Vec::new();
        for row in self.rows() {
            for col in self.cols() {
                let Some(index) = self.slot(col, row) else {
                    continue;
                };
                let cell = self.cells[index];
                let hex = hex_of_cell(col, row);
                for slot in 0..cell.num_edges {
                    let inland = cell.edges[usize::from(slot)] % 12 < 6;
                    let handle = Handle {
                        hex,
                        edge: i32::from(slot),
                    };
                    links.push((
                        index,
                        usize::from(slot),
                        self.link_to(self.next_in_coastline(handle, inland)),
                        self.link_to(self.next_in_coastline(handle, !inland)),
                    ));
                }
            }
        }
        for (index, slot, next, prev) in links {
            self.cells[index].next[slot] = next;
            self.cells[index].prev[slot] = prev;
        }
    }

    fn link_to(&self, peer: Handle) -> Link {
        let (col, row) = cell_of_hex(peer.hex);
        Link {
            marked: false,
            peer_edge: if peer.edge < 0 {
                NO_PEER
            } else {
                peer.edge as u8
            },
            col,
            row,
        }
    }

    /// The segment that follows (or precedes) a segment along the coastline, by the ends of its
    /// side: through a back edge of this hex to the hex across, or to the edge of the diagonal
    /// hex that touches both ends. The step back is the mirror of the step forward.
    // re: CoastGrid::NextSplineInCoastline 0x088e4554 (forward, the code of the dump);
    // RefactorfoRVS 0x088e4acf (backward: not in the dump, mirrored from the forward step: the
    // call passes the ends the other way round, `cellA` with `uiCornerA`, the diagonal with `uiCornerB`)
    // TODO(RW-10): read the body of RefactorfoRVS when it is dumped.
    pub fn next_in_coastline(&self, handle: Handle, forward: bool) -> Handle {
        let cell = self.cell_of(handle.hex);
        let id = u32::from(cell.edges[handle.edge.max(0) as usize % 3]);
        let [mut corner_a, mut corner_b] = COAST_CONNECT[(id % 3) as usize];
        if id >= 12 {
            corner_a += 12;
            corner_b += 12;
        }
        let dir_a = (corner_a % 6) as usize;
        let dir_b = (corner_b % 6) as usize;
        let hex_a = add(handle.hex, TRANSLATIONS[dir_a]);
        let hex_b = add(handle.hex, TRANSLATIONS[dir_b]);
        let diagonal = add(hex_b, TRANSLATIONS[dir_a]);
        let turn = |dir: usize| -> i32 { if dir < 3 { 3 } else { -3 } };
        // The direction a corner has seen from the hex across.
        let opposite = |dir: usize, corner: u32| -> u32 { (turn(dir) + corner as i32) as u32 % 6 };
        // The edge of `target` of this class that has the direction `wanted`, among the edges
        // the cell lists.
        let find_edge = |target: Hex, corner: u32, wanted: u32| -> Option<Handle> {
            let c = self.cell_of(target);
            (0..usize::from(c.num_edges))
                .find(|&e| {
                    let nid = u32::from(c.edges[e]);
                    same_class(nid, corner) && nid % 6 == wanted
                })
                .map(|e| Handle {
                    hex: target,
                    edge: e as i32,
                })
        };
        // A side of this hex in the back list belongs to the neighbour across it; the match is
        // that neighbour's edge in the opposite direction.
        let through_back = |across_dir: usize, across: Hex, corner: u32| -> Option<Handle> {
            (0..usize::from(cell.num_back))
                .filter(|&b| {
                    let bid = u32::from(cell.back_edges[b]);
                    same_class(bid, corner) && bid as usize % 6 == across_dir
                })
                .find_map(|_| find_edge(across, corner, opposite(across_dir, corner)))
        };
        let found = if forward {
            through_back(dir_b, hex_b, corner_b)
                .or_else(|| find_edge(diagonal, corner_a, opposite(dir_a, corner_a)))
        } else {
            through_back(dir_a, hex_a, corner_a)
                .or_else(|| find_edge(diagonal, corner_b, opposite(dir_b, corner_b)))
        };
        found.unwrap_or(Handle {
            hex: handle.hex,
            edge: -1,
        })
    }

    /// The six control points of the spline through one segment: the segment, extended by its
    /// own length at both ends, with the ends replaced by the neighbouring segments when the
    /// coastline goes on. `hex` is the hex of `cell`.
    // re: CoastGrid::ReconstructCoastSpline 0x088e3f3a (the edge slots; the back-slot variant is not used by the stages)
    pub fn reconstruct_spline(&self, cell: &Cell, hex: Hex, edge: usize) -> [Vec2; 6] {
        let seg = cell.segments[edge];
        let delta = seg.first - seg.second;
        let half = delta * 0.5;
        let mut p = [
            seg.first + delta,
            seg.first + half,
            seg.first,
            seg.second,
            seg.second - half,
            seg.second - delta,
        ];
        let handle = Handle {
            hex,
            edge: edge as i32,
        };
        // The segment of a neighbour, its ends swapped, moved by the wrap of its column.
        let neighbor = |h: Handle| -> Option<(Vec2, Vec2)> {
            if h.edge < 0 {
                return None;
            }
            let (col, row) = cell_of_hex(h.hex);
            let other = self.cell(col, row);
            // A neighbour across the wrapped seam sits one map width of texels away.
            let wrapped = if self.wrap {
                col.rem_euclid(self.width)
            } else {
                col
            };
            let shift = Vec2::new(
                ((col - wrapped) / self.width) as f32 * self.wrap_shift.x,
                0.0,
            );
            let s = other.segments[h.edge as usize];
            Some((s.second + shift, s.first + shift))
        };
        if let Some((a, b)) = neighbor(self.next_in_coastline(handle, true)) {
            p[4] = a;
            p[5] = b;
        }
        if let Some((a, b)) = neighbor(self.next_in_coastline(handle, false)) {
            p[0] = a;
            p[1] = b;
        }
        // Outer-side edges run opposite to their stored segment.
        if cell.edges[edge] % 12 >= 6 {
            p.swap(0, 5);
            p.swap(1, 4);
            p.swap(2, 3);
        }
        p
    }

    /// `SplinePreProcessJob`: ten passes, each reads the cells and writes a scratch copy, a
    /// commit pass copies it back.
    // re: SplinePreProcessJob::Execute 0x088e578e (states 1 smooth, 2 commit, 3 shift mountains)
    fn preprocess(&mut self) {
        let mut scratch = vec![[Segment::default(); 3]; self.cells.len()];
        for state in STATE_LIST {
            match state {
                // The weights 1-2-1 over the spline points around each end of the segment.
                1 => {
                    let mut out = Vec::new();
                    self.for_each_edge(|g, index, cell, hex, edge| {
                        let p = g.reconstruct_spline(cell, hex, edge);
                        let first = p[1] * 0.25 + p[2] * 0.5 + p[3] * 0.25;
                        let second = p[2] * 0.25 + p[3] * 0.5 + p[4] * 0.25;
                        let seg = if cell.edges[edge] % 12 >= 6 {
                            Segment {
                                first: second,
                                second: first,
                            }
                        } else {
                            Segment { first, second }
                        };
                        out.push((index, edge, seg));
                    });
                    for (index, edge, seg) in out {
                        scratch[index][edge] = seg;
                    }
                }
                2 => {
                    for (cell, s) in self.cells.iter_mut().zip(&scratch) {
                        let n = usize::from(cell.num_edges);
                        cell.segments[..n].copy_from_slice(&s[..n]);
                    }
                }
                _ => {
                    let mut out = Vec::new();
                    self.for_each_edge(|g, index, cell, hex, edge| {
                        let mut seg = cell.segments[edge];
                        if cell.mountain_edge[edge] != 0 {
                            let p = g.reconstruct_spline(cell, hex, edge);
                            let normal = |t: f32| {
                                let gr = bspline::gradient(&p, t);
                                let len = (gr.x * gr.x + gr.y * gr.y).sqrt();
                                Vec2::new(gr.y / len, -gr.x / len)
                            };
                            let (na, nb) = (normal(1.5), normal(2.5));
                            if cell.edges[edge] % 12 < 6 {
                                seg.second += na * MOUNTAIN_SHIFT;
                                seg.first += nb * MOUNTAIN_SHIFT;
                            } else {
                                seg.second += nb * MOUNTAIN_SHIFT;
                                seg.first += na * MOUNTAIN_SHIFT;
                            }
                        }
                        out.push((index, edge, seg));
                    });
                    for (index, edge, seg) in out {
                        scratch[index][edge] = seg;
                    }
                }
            }
        }
    }

    /// Calls `f` for every edge of every cell (the cell, its hex and the slot of the edge).
    fn for_each_edge(&self, mut f: impl FnMut(&Self, usize, &Cell, Hex, usize)) {
        for row in self.rows() {
            for col in self.cols() {
                let index = self.slot(col, row).expect("a cell of the grid");
                let cell = &self.cells[index];
                let hex = hex_of_cell(col, row);
                for edge in 0..usize::from(cell.num_edges) {
                    f(self, index, cell, hex, edge);
                }
            }
        }
    }

    /// Marks the next link `slot` of the cell at `(col, row)`; used by the walk that cuts the
    /// coastlines.
    pub fn set_marked(&mut self, col: i32, row: i32, slot: usize, marked: bool) {
        if let Some(i) = self.slot(col, row) {
            self.cells[i].next[slot].marked = marked;
        }
    }

    /// `ClearMarks`.
    // re: cvCoastSplineSystem::ClearMarks 0x088ecda6
    pub fn clear_marks(&mut self) {
        for cell in &mut self.cells {
            for l in cell.next.iter_mut().chain(cell.prev.iter_mut()) {
                l.marked = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::testkit::{frame_of, island, island_at};
    use super::*;
    use crate::terrain::{Relief, TerrainKind};
    use crate::world::terrain::basis::{BasisFeature, NwPiece};

    /// re-b16 C1-2: the wonder overrides the class of the terrain under it: Krakatoa on the coast is
    /// land for the coast (its own class bit, in the land mask), the reef and Gibraltar stay shallow,
    /// El Dorado and the Fountain of Youth keep the class of their terrain.
    #[test]
    fn a_wonder_on_water_takes_the_class_of_the_wonder() {
        let on_coast = |feature| PlotBasis {
            feature,
            ..plot(Relief::Water, TerrainKind::Coast)
        };
        let krakatoa = pattern_flags(&on_coast(BasisFeature::NaturalWonder(NwPiece::Krakatoa)));
        assert_ne!(krakatoa & flags::LAND, 0);
        for nw in [NwPiece::Reef, NwPiece::Gibraltar] {
            let f = pattern_flags(&on_coast(BasisFeature::NaturalWonder(nw)));
            assert_eq!(f & flags::LAND, 0, "{nw:?}");
            assert_ne!(f & flags::SHALLOW, 0, "{nw:?}");
        }
        assert_eq!(
            pattern_flags(&on_coast(BasisFeature::WonderKeepsTerrain)),
            flags::COAST
        );
    }

    fn edges(g: &CoastGrid) -> Vec<(i32, i32, usize)> {
        let mut out = Vec::new();
        for row in g.rows() {
            for col in g.cols() {
                for e in 0..usize::from(g.cell(col, row).num_edges) {
                    out.push((col, row, e));
                }
            }
        }
        out
    }

    fn plot(relief: Relief, terrain: TerrainKind) -> PlotBasis {
        PlotBasis {
            relief,
            terrain,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    #[test]
    fn flags_follow_the_class_of_the_piece() {
        let p = plot;
        assert_eq!(
            pattern_flags(&p(Relief::Flat, TerrainKind::Grass)),
            flags::GRASS
        );
        assert_eq!(
            pattern_flags(&p(Relief::Hill, TerrainKind::Snow)),
            flags::SNOW_HILLS
        );
        assert_eq!(
            pattern_flags(&p(Relief::Mountain, TerrainKind::Grass)),
            flags::MOUNTAIN
        );
        assert_eq!(
            pattern_flags(&p(Relief::Water, TerrainKind::Coast)),
            flags::COAST
        );
        assert_eq!(
            pattern_flags(&p(Relief::Water, TerrainKind::Ocean)),
            flags::OCEAN
        );
        // The land mask lets the land through and keeps the water out.
        for f in [
            flags::GRASS,
            flags::MOUNTAIN,
            flags::SNOW_HILLS,
            flags::WONDER,
        ] {
            assert_ne!(f & flags::LAND, 0);
        }
        for f in [flags::COAST, flags::OCEAN, flags::REEF, flags::GIBRALTAR] {
            assert_eq!(f & flags::LAND, 0);
        }
        // Coast, reef and Gibraltar are shallow, the ocean is not.
        assert_ne!(flags::COAST & flags::SHALLOW, 0);
        assert_ne!(flags::REEF & flags::SHALLOW, 0);
        assert_ne!(flags::GIBRALTAR & flags::SHALLOW, 0);
        assert_eq!(flags::OCEAN & flags::SHALLOW, 0);
    }

    #[test]
    fn the_segment_templates_are_the_middle_half_of_the_sides() {
        let b = island(12, 12, false, 2);
        let g = CoastGrid::new(&b, &frame_of(&b));
        for t in g.templates() {
            let len = (t.second - t.first).length();
            assert!(len > 10.0 && len < 40.0, "{len}");
        }
        // The three sides are 120 degrees apart: the sum of the midpoints is zero.
        let sum: Vec2 = g
            .templates()
            .iter()
            .map(|t| (t.first + t.second) * 0.5)
            .sum();
        assert!(sum.length() < 1e-3, "{sum}");
    }

    #[test]
    fn an_island_has_an_edge_for_every_side_between_two_classes() {
        // Land of radius 2 (19 hexes) has 6 * 5 = 30 sides on its shore; the ring of coast of
        // radius 3 has 18 hexes and 6 * 7 = 42 sides to the ocean.
        let b = island(16, 16, false, 2);
        let g = CoastGrid::new(&b, &frame_of(&b));
        let (mut shore, mut shelf) = (0, 0);
        // The shelf of the border (the deep sea of the map against the coast of the padding) is
        // left out: the cells of the island are well inside.
        let inner = |&(c, r, _): &(i32, i32, usize)| (2..14).contains(&c) && (2..14).contains(&r);
        for (c, r, e) in edges(&g).into_iter().filter(inner) {
            // Each side has one geometry, in the hex whose direction is 0..2; the ids of the
            // owner say which side of the line the owner is on.
            let id = g.cell(c, r).edges[e];
            if id < 12 {
                shore += 1;
            } else {
                shelf += 1;
            }
        }
        assert_eq!((shore, shelf), (30, 42));
    }

    #[test]
    fn following_the_links_goes_round_the_island_and_back() {
        let b = island(16, 16, false, 2);
        let g = CoastGrid::new(&b, &frame_of(&b));
        let all = edges(&g);
        for &(c, r, e) in &all {
            // Every edge of a closed coast has both neighbours.
            let cell = g.cell(c, r);
            assert_ne!(cell.next[e].peer_edge, NO_PEER, "{c},{r},{e}");
            assert_ne!(cell.prev[e].peer_edge, NO_PEER, "{c},{r},{e}");
            // The previous edge of the next edge is this one.
            let nl = cell.next[e];
            let back = g.cell(nl.col, nl.row).prev[usize::from(nl.peer_edge)];
            assert_eq!((back.col, back.row, usize::from(back.peer_edge)), (c, r, e));
        }
        // The walk of one loop of the island comes back to its start after as many steps as the
        // loop has edges.
        let (c0, r0, e0) = *all
            .iter()
            .find(|&&(c, r, _)| (5..12).contains(&c) && (5..12).contains(&r))
            .unwrap();
        let (mut c, mut r, mut e) = (c0, r0, e0);
        let mut steps = 0;
        loop {
            let l = g.cell(c, r).next[e];
            (c, r, e) = (l.col, l.row, usize::from(l.peer_edge));
            steps += 1;
            if (c, r, e) == (c0, r0, e0) || steps > all.len() {
                break;
            }
        }
        assert!(steps == 30 || steps == 42, "{steps}");
        assert_eq!((c, r, e), (c0, r0, e0));
    }

    /// re-b16 C1-1: the grid is the padded game grid (2 rows above and below, 2 columns each side
    /// without wrap, all coast), so an island cut by the left border goes on into the padding and
    /// every edge has both neighbours; with wrap the shore goes on across the seam instead.
    #[test]
    fn a_map_without_wrap_links_its_border_coast_through_the_padding() {
        let b = island_at(12, 12, false, (0, 6), 2);
        let g = CoastGrid::new(&b, &frame_of(&b));
        assert!(!edges(&g).is_empty());
        assert!(edges(&g).into_iter().all(|(c, r, e)| {
            g.cell(c, r).next[e].peer_edge != NO_PEER && g.cell(c, r).prev[e].peer_edge != NO_PEER
        }));
        assert!(edges(&g).into_iter().any(|(c, _, _)| c < 0));
        assert_eq!((g.cols(), g.rows()), (-2..14, -2..14));
        // With wrap the same island meets the seam: the shore goes on across it.
        let w = island_at(12, 12, true, (0, 6), 2);
        let gw = CoastGrid::new(&w, &frame_of(&w));
        assert_eq!((gw.cols(), gw.rows()), (0..12, -2..14));
        let across = edges(&gw).into_iter().any(|(c, r, e)| {
            let l = gw.cell(c, r).next[e];
            l.peer_edge != NO_PEER && !(0..12).contains(&l.col)
        });
        assert!(across);
        // And every edge has both neighbours.
        assert!(edges(&gw).into_iter().all(|(c, r, e)| {
            gw.cell(c, r).next[e].peer_edge != NO_PEER && gw.cell(c, r).prev[e].peer_edge != NO_PEER
        }));
    }
    #[test]
    fn a_hex_and_its_cell_convert_both_ways() {
        for row in -3..8 {
            for col in -3..8 {
                assert_eq!(cell_of_hex(hex_of_cell(col, row)), (col, row));
                let h = hex_of_cell(col, row);
                assert_eq!(h[0] + h[1] + h[2], 0);
            }
        }
        // The directions are three vectors 120 degrees apart and their opposites.
        let s: i32 = (0..3).map(|i| TRANSLATIONS[i][0]).sum();
        assert_eq!(s, 0);
        for i in 0..3 {
            let (a, b) = (TRANSLATIONS[i], TRANSLATIONS[i + 3]);
            assert_eq!([a[0] + b[0], a[1] + b[1], a[2] + b[2]], [0, 0, 0]);
        }
    }

    #[test]
    fn a_mountain_shore_is_pushed_four_texels_off_the_hex_side() {
        // A mountain island and the same island of grass: the segments of the mountain's shore
        // differ from the grass ones by about the shift.
        let g = |relief: Relief| {
            let b = crate::world::water::coast::testkit::basis_from(16, 16, false, |x, y| {
                let d = crate::world::water::coast::testkit::distance_from_center(16, 16, x, y);
                if d <= 2 {
                    plot(relief, TerrainKind::Grass)
                } else if d == 3 {
                    plot(Relief::Water, TerrainKind::Coast)
                } else {
                    plot(Relief::Water, TerrainKind::Ocean)
                }
            });
            CoastGrid::new(&b, &frame_of(&b))
        };
        let (grass, mountain) = (g(Relief::Flat), g(Relief::Mountain));
        let mut moved = 0.0f32;
        for (c, r, e) in edges(&grass) {
            let (a, b) = (
                grass.cell(c, r).segments[e],
                mountain.cell(c, r).segments[e],
            );
            if grass.cell(c, r).edges[e] < 12 {
                moved = moved.max((a.first - b.first).length());
            }
        }
        assert!(moved > 1.0, "{moved}");
    }
}
