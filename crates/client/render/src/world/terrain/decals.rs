//! The layer of decals: the port of `TerrainDecalLayer`, the CPU part. A
//! decal is a mesh of a few triangles with a texture that the bake draws into the page of every cell it
//! touches ("Decal Layer", and "Decal Macro Shadow" for the ones that cast a shadow). Floodplains and the
//! banks of rivers come from the build stages (`create`), roads, fields and the shadows of trees from
//! the modules of the objects (`set_object_decals`).
//!
//! The decals are kept in banks of at most `0xffff` vertices (the index buffer of the original is 16 bit)
//! and in a grid of 256 units that tells which decals lie in an area; a decal that is larger than 256
//! units on a side is not in the grid but in the list of the big ones. The bake asks for the decals of an
//! area ([`DecalLayer::in_area`]) sorted by priority, the lowest first, so the ones with a high priority
//! are drawn over. Creating or destroying a decal records the area it covers
//! ([`DecalLayer::take_dirty`]); the terrain makes the pages of the cells in it again
//! (`Terrain::invalidate_region`).
//!
//! Positions are in our world (X east, Z south), as the producers have them; the bake turns them into
//! texels of the field.
// re: TerrainDecalLayer.cpp:21-43 (the categories, the grid and the big decals), :160-318 (create,
// destroy, the variant of the type)

use std::collections::{BTreeMap, HashMap};

use asset_format::{DecalDef, LogicalId};
use glam::Vec2;

use super::build::rand::MsvcRand;
use crate::world::decal::{
    BIG_DECAL_EXTENT, DEFAULT_PRIORITY, DecalCategory, DecalType, DecalTypes, GRID_CELL_SIZE,
    MAX_BANK_VERTICES,
};

/// The decal is drawn into the albedo and the height of the page ("Decal Layer").
pub const PASS_LAYER: u32 = 1;
/// The decal is drawn into the macro shadow of the page ("Decal Macro Shadow").
pub const PASS_SHADOW: u32 = 2;

/// A decal that was made; `destroy` takes it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DecalHandle(pub u32);

/// A vertex of a decal: the place in our world and the texture coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecalVertex {
    pub pos: Vec2,
    pub uv: Vec2,
}

/// Who makes the decals that `set_object_decals` replaces: a source owns the decals it made, and a call
/// replaces them on the hexes it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DecalSource {
    Roads,
    Farms,
    Trees,
    /// The ground meshes of the landmarks (`LandmarkShader`, see [`MeshDecal`]).
    Landmarks,
    /// The ground quads of the cities (`LandmarkShader`, see [`MeshDecal`]).
    Cities,
    Other(u8),
}

/// A decal of a model's mesh: the triangles as they are, with the textures of the material. The
/// original turns every `LandmarkShader` mesh of a landmark into one (`DecalContainerBuilderContext::
/// InitMeshDecal`: Position and TextureCoordinates0 of the mesh, the colour texture of slot 0 and the
/// second of slot 1) and calls `CreateDecal` with a type of its own, not one of a category.
// re: rw-w8-answers.txt q2 (InitMeshDecal 0x088be424), TerrainDecalLayer.cpp:160 (`CreateDecal(DecalType
// const&, ...)`)
#[derive(Clone, Debug, PartialEq)]
pub struct MeshDecal {
    pub albedo: LogicalId,
    pub height: Option<LogicalId>,
    pub priority: f32,
    pub verts: Vec<DecalVertex>,
    pub tris: Vec<[u16; 3]>,
    /// The hex (its index in the map) the decal belongs to.
    pub cell: u32,
}

/// The shape of the decal of an object, in our world.
#[derive(Clone, Debug, PartialEq)]
pub enum DecalShape {
    /// Four corners in order round the quad; the texture covers it (0,0), (1,0), (1,1), (0,1).
    Quad([Vec2; 4]),
    /// Pairs of points, left and right of a line, from one end to the other; the texture runs along it
    /// (`u` across, `v` from 0 to 1 along).
    Strip(Vec<[Vec2; 2]>),
    /// Quads that make one decal: four corners each, in order round the quad; the texture covers each
    /// (0,1), (1,1), (1,0), (0,0), as the shadows of the trees of a hex (`ForestVis::BuildShadows`).
    Quads(Vec<[Vec2; 4]>),
    /// Points with their own texture coordinates and the triangles over them, as the producer lays
    /// the texture (the fields of a farm: in the world, past 1 where the texture repeats).
    Mesh {
        verts: Vec<(Vec2, Vec2)>,
        tris: Vec<[u16; 3]>,
    },
}

/// One decal of an object for `set_object_decals`.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectDecal {
    /// The kind in `decal_filenames.xml` (`Main_Road`, `Detail_Road`, `Crops`, `Tree_Shadow` ...).
    pub category: DecalCategory,
    /// The albedo, if the producer has its own; else the type picked in the category gives it.
    pub texture: Option<LogicalId>,
    pub shape: DecalShape,
    /// The hex (its index in the map) the decal belongs to.
    pub cell: u32,
    /// Drawn into the macro shadow ("Decal Macro Shadow") instead of the albedo and the height ("Decal
    /// Layer"): the `bPassFlag` of `CreateDecal`, which puts a decal in one of the two passes only.
    // re: TerrainDecalLayer.cpp:236 (`m_flags = bPassFlag ? kEntryPassFlag : 0`), :390 (`Render` draws the
    // decals whose flag equals its own pass)
    pub casts_shadow: bool,
}

/// A decal in the layer.
#[derive(Clone, Debug, PartialEq)]
pub struct DecalData {
    pub handle: DecalHandle,
    pub category: DecalCategory,
    pub albedo: Option<LogicalId>,
    pub height: Option<LogicalId>,
    pub priority: f32,
    /// `PASS_LAYER | PASS_SHADOW` bits.
    pub pass: u32,
    pub verts: Vec<DecalVertex>,
    pub tris: Vec<[u16; 3]>,
    /// The bounds in our world.
    pub min: Vec2,
    pub max: Vec2,
    pub bank: u32,
    source: Option<(DecalSource, u32)>,
}

impl DecalData {
    fn big(&self) -> bool {
        let e = self.max - self.min;
        e.x > BIG_DECAL_EXTENT || e.y > BIG_DECAL_EXTENT
    }

    /// Whether the bounds overlap the rectangle.
    pub fn overlaps(&self, min: Vec2, max: Vec2) -> bool {
        self.min.x <= max.x && self.max.x >= min.x && self.min.y <= max.y && self.max.y >= min.y
    }
}

/// A bank of vertices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bank {
    pub vertices: u32,
    pub decals: u32,
}

/// The layer.
#[derive(Default)]
pub struct DecalLayer {
    types: DecalTypes,
    decals: BTreeMap<u32, DecalData>,
    next: u32,
    banks: Vec<Bank>,
    grid: HashMap<(i32, i32), Vec<u32>>,
    big: Vec<u32>,
    by_source: HashMap<(DecalSource, u32), Vec<u32>>,
    dirty: Vec<(Vec2, Vec2)>,
    /// How many areas were marked, by [`dirty_slot`].
    dirty_by_source: [u64; 7],
    revision: u64,
    types_epoch: u64,
}

/// The names of the counters of [`DecalLayer::dirty_counters`], in the order of [`dirty_slot`].
const DIRTY_NAMES: [&str; 7] = [
    "terrain.decal_dirty.farms_total",
    "terrain.decal_dirty.roads_total",
    "terrain.decal_dirty.trees_total",
    "terrain.decal_dirty.landmarks_total",
    "terrain.decal_dirty.cities_total",
    "terrain.decal_dirty.build_total",
    "terrain.decal_dirty.other_total",
];

fn dirty_slot(source: Option<DecalSource>) -> usize {
    match source {
        Some(DecalSource::Farms) => 0,
        Some(DecalSource::Roads) => 1,
        Some(DecalSource::Trees) => 2,
        Some(DecalSource::Landmarks) => 3,
        Some(DecalSource::Cities) => 4,
        None => 5,
        Some(DecalSource::Other(_)) => 6,
    }
}

/// A decal that a call of `set_object_decals` or `set_mesh_decals` would make for a hex.
struct Planned {
    category: DecalCategory,
    ty: DecalType,
    verts: Vec<DecalVertex>,
    tris: Vec<[u16; 3]>,
    pass: u32,
}

/// Whether the mesh fits a bank and its indices lie in its vertices (else `insert` refuses it).
fn fits(verts: &[DecalVertex], tris: &[[u16; 3]]) -> bool {
    let count = verts.len() as u32;
    !verts.is_empty()
        && !tris.is_empty()
        && count <= MAX_BANK_VERTICES
        && tris.iter().all(|t| t.iter().all(|&i| u32::from(i) < count))
}

fn grid_range(min: Vec2, max: Vec2) -> impl Iterator<Item = (i32, i32)> {
    let cell = |v: f32| (v / GRID_CELL_SIZE).floor() as i32;
    let (x0, x1, y0, y1) = (cell(min.x), cell(max.x), cell(min.y), cell(max.y));
    (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
}

/// A number from a hex, the place in its list and the category: the variant of the type of a decal of
/// an object.
fn variant_hash(cell: u32, ordinal: u32, category: u8) -> u32 {
    let mut z = u64::from(cell) << 24 | u64::from(ordinal) << 8 | u64::from(category);
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 16) as u32 & 0x7fff
}

impl DecalLayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// The types of `decal_filenames.xml` (`WorldSettings::decals`).
    pub fn set_types(&mut self, defs: &[DecalDef]) {
        self.types = DecalTypes::from_defs(defs);
        self.types_epoch += 1;
    }

    /// Grows each time the types are set (what the textures of the types are can change with them).
    pub fn types_epoch(&self) -> u64 {
        self.types_epoch
    }

    pub fn types(&self) -> &DecalTypes {
        &self.types
    }

    pub fn len(&self) -> usize {
        self.decals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.decals.is_empty()
    }

    /// Grows with every create and destroy: the pages that were baked with the decals are older.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn banks(&self) -> &[Bank] {
        &self.banks
    }

    /// Decals in the list of the big ones.
    pub fn big_count(&self) -> usize {
        self.big.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &DecalData> {
        self.decals.values()
    }

    pub fn get(&self, handle: DecalHandle) -> Option<&DecalData> {
        self.decals.get(&handle.0)
    }

    /// The areas that changed since the last call, in our world.
    pub fn take_dirty(&mut self) -> Vec<(Vec2, Vec2)> {
        std::mem::take(&mut self.dirty)
    }

    /// A decal of `category`, its type picked by `rand() % number of types` from the stream of the
    /// caller (a stage of the build uses the stream of the build, so the choice is the original's).
    /// `None` if the category has no type or the mesh does not fit a bank.
    pub fn create(
        &mut self,
        category: DecalCategory,
        verts: Vec<DecalVertex>,
        tris: Vec<[u16; 3]>,
        pass: u32,
        rng: &mut MsvcRand,
    ) -> Option<DecalHandle> {
        let n = self.types.of(category).len();
        if n == 0 {
            return None;
        }
        let ty = self.types.of(category)[rng.next() as usize % n].clone();
        self.insert(category, &ty, verts, tris, pass, None)
    }

    /// [`Self::create`] with the draw of `rand()` made by the caller: the type is `roll % number of
    /// types`. For the producers that build their decals before the layer has its types (the banks of
    /// the rivers) and draw the numbers in the order of the original.
    // re: TerrainDecalLayer.cpp:311-318 (`typeIndex += rand_wincompatible() % numTypes`)
    pub fn create_rolled(
        &mut self,
        category: DecalCategory,
        roll: u32,
        verts: Vec<DecalVertex>,
        tris: Vec<[u16; 3]>,
        pass: u32,
    ) -> Option<DecalHandle> {
        let n = self.types.of(category).len();
        if n == 0 {
            return None;
        }
        let ty = self.types.of(category)[roll as usize % n].clone();
        self.insert(category, &ty, verts, tris, pass, None)
    }

    /// A decal of a given type (`create_typed`): the type is an index into the category.
    pub fn create_typed(
        &mut self,
        category: DecalCategory,
        type_index: usize,
        verts: Vec<DecalVertex>,
        tris: Vec<[u16; 3]>,
        pass: u32,
    ) -> Option<DecalHandle> {
        let ty = self.types.of(category).get(type_index)?.clone();
        self.insert(category, &ty, verts, tris, pass, None)
    }

    fn insert(
        &mut self,
        category: DecalCategory,
        ty: &DecalType,
        verts: Vec<DecalVertex>,
        tris: Vec<[u16; 3]>,
        pass: u32,
        source: Option<(DecalSource, u32)>,
    ) -> Option<DecalHandle> {
        if !fits(&verts, &tris) {
            return None;
        }
        let count = verts.len() as u32;
        let bank = match self
            .banks
            .iter()
            .position(|b| b.vertices + count <= MAX_BANK_VERTICES)
        {
            Some(b) => b,
            None => {
                self.banks.push(Bank::default());
                self.banks.len() - 1
            }
        };
        self.banks[bank].vertices += count;
        self.banks[bank].decals += 1;
        let (mut min, mut max) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for v in &verts {
            min = min.min(v.pos);
            max = max.max(v.pos);
        }
        let handle = DecalHandle(self.next);
        self.next += 1;
        let data = DecalData {
            handle,
            category,
            albedo: ty.albedo.clone(),
            height: ty.heightmap.clone(),
            priority: ty.priority,
            pass,
            verts,
            tris,
            min,
            max,
            bank: bank as u32,
            source,
        };
        if data.big() {
            self.big.push(handle.0);
        } else {
            for key in grid_range(data.min, data.max) {
                self.grid.entry(key).or_default().push(handle.0);
            }
        }
        if let Some(key) = source {
            self.by_source.entry(key).or_default().push(handle.0);
        }
        self.mark_dirty(source.map(|s| s.0), data.min, data.max);
        self.revision += 1;
        self.decals.insert(handle.0, data);
        Some(handle)
    }

    /// Takes a decal out; the area it covered is made again. `false` if there was none.
    pub fn destroy(&mut self, handle: DecalHandle) -> bool {
        let Some(data) = self.decals.remove(&handle.0) else {
            return false;
        };
        let bank = &mut self.banks[data.bank as usize];
        bank.vertices -= data.verts.len() as u32;
        bank.decals -= 1;
        if data.big() {
            self.big.retain(|&h| h != handle.0);
        } else {
            for key in grid_range(data.min, data.max) {
                if let Some(list) = self.grid.get_mut(&key) {
                    list.retain(|&h| h != handle.0);
                    if list.is_empty() {
                        self.grid.remove(&key);
                    }
                }
            }
        }
        if let Some(key) = data.source
            && let Some(list) = self.by_source.get_mut(&key)
        {
            list.retain(|&h| h != handle.0);
            if list.is_empty() {
                self.by_source.remove(&key);
            }
        }
        self.mark_dirty(data.source.map(|s| s.0), data.min, data.max);
        self.revision += 1;
        true
    }

    /// Records an area as changed, and counts it for the source that made the decal (`None`: a stage of
    /// the build).
    fn mark_dirty(&mut self, source: Option<DecalSource>, min: Vec2, max: Vec2) {
        self.dirty.push((min, max));
        self.dirty_by_source[dirty_slot(source)] += 1;
    }

    /// The areas pushed to [`Self::take_dirty`] since the layer was made, by the source of the decal,
    /// with the names of the stats.
    pub fn dirty_counters(&self) -> Vec<(&'static str, u64)> {
        DIRTY_NAMES
            .iter()
            .copied()
            .zip(self.dirty_by_source)
            .collect()
    }

    /// Replaces every decal of `source` on the hexes `cells` with `decals`, in the order of the hexes and
    /// then of the list. The type of each decal is picked by a number from the hex and the place in the
    /// list, not from a stream shared by all the decals of a batch.
    // SIMPLIFIED(SIMP-036): the original draws `rand() % number` in the order all the decals of a batch are
    // made in; ours is another order, so the variant comes from the position
    ///
    /// A hex whose new decals are the same as the ones it has (the same textures, priority, pass and
    /// triangles, in the same order) keeps them: nothing is destroyed or made, nothing is marked, and the
    /// handles returned are the old ones. The producers call this every turn with mostly the same decals,
    /// and the pages under them would be made again for nothing.
    pub fn set_object_decals(
        &mut self,
        source: DecalSource,
        cells: &[u32],
        decals: Vec<ObjectDecal>,
    ) -> Vec<DecalHandle> {
        let mut planned: BTreeMap<u32, Vec<Planned>> = BTreeMap::new();
        let mut ordinal: HashMap<u32, u32> = HashMap::new();
        let mut ordered: Vec<(usize, &ObjectDecal)> = decals.iter().enumerate().collect();
        ordered.sort_by_key(|(i, d)| (d.cell, *i));
        for (_, d) in ordered {
            if !cells.contains(&d.cell) {
                continue;
            }
            let k = ordinal.entry(d.cell).or_insert(0);
            let n = self.types.of(d.category).len();
            let mut ty = if n > 0 {
                self.types.of(d.category)[variant_hash(d.cell, *k, d.category.0) as usize % n]
                    .clone()
            } else {
                DecalType {
                    albedo: None,
                    heightmap: None,
                    priority: DEFAULT_PRIORITY,
                    name: String::new(),
                }
            };
            *k += 1;
            // A producer that names its albedo gets the type of the category that has it (its height
            // and priority: the fields of a farm), else the albedo over the type picked.
            let named = d.texture.as_ref().and_then(|t| {
                self.types
                    .of(d.category)
                    .iter()
                    .find(|ty| ty.albedo.as_ref() == Some(t))
                    .cloned()
            });
            if let Some(found) = named {
                ty = found;
            } else if let Some(t) = &d.texture {
                ty.albedo = Some(t.clone());
            } else if n == 0 {
                continue;
            }
            let (verts, tris) = mesh_of(&d.shape);
            let pass = if d.casts_shadow {
                PASS_SHADOW
            } else {
                PASS_LAYER
            };
            if fits(&verts, &tris) {
                planned.entry(d.cell).or_default().push(Planned {
                    category: d.category,
                    ty,
                    verts,
                    tris,
                    pass,
                });
            }
        }
        self.replace_cells(source, cells, planned)
    }

    /// Replaces every decal of `source` on the hexes `cells` with the mesh decals of those hexes (the
    /// others are left out). Each has its own textures and priority; the category is only a label
    /// (Crops, the one of the ground under the objects). A hex whose decals are the same as it has keeps
    /// them, as in [`Self::set_object_decals`].
    pub fn set_mesh_decals(
        &mut self,
        source: DecalSource,
        cells: &[u32],
        decals: Vec<MeshDecal>,
    ) -> Vec<DecalHandle> {
        let label = if source == DecalSource::Roads {
            "Main_Road"
        } else {
            "Crops"
        };
        let category = DecalCategory::of_name(label).unwrap_or(DecalCategory(0));
        let mut planned: BTreeMap<u32, Vec<Planned>> = BTreeMap::new();
        for d in decals {
            if !cells.contains(&d.cell) || !fits(&d.verts, &d.tris) {
                continue;
            }
            let ty = DecalType {
                albedo: Some(d.albedo),
                heightmap: d.height,
                priority: d.priority,
                name: String::new(),
            };
            planned.entry(d.cell).or_default().push(Planned {
                category,
                ty,
                verts: d.verts,
                tris: d.tris,
                pass: PASS_LAYER,
            });
        }
        self.replace_cells(source, cells, planned)
    }

    /// Brings the hexes `cells` of `source` to the decals planned for them: the ones that are the same
    /// as the hex has are kept, else the old are destroyed and the new made. The handles of the decals
    /// the hexes have now, in the order of the hexes.
    fn replace_cells(
        &mut self,
        source: DecalSource,
        cells: &[u32],
        mut planned: BTreeMap<u32, Vec<Planned>>,
    ) -> Vec<DecalHandle> {
        let mut hexes: Vec<u32> = cells.to_vec();
        hexes.sort_unstable();
        hexes.dedup();
        let mut made = Vec::new();
        for cell in hexes {
            let new = planned.remove(&cell).unwrap_or_default();
            let old = self
                .by_source
                .get(&(source, cell))
                .cloned()
                .unwrap_or_default();
            let same = old.len() == new.len()
                && old.iter().zip(&new).all(|(h, p)| {
                    self.decals.get(h).is_some_and(|d| {
                        d.category == p.category
                            && d.albedo == p.ty.albedo
                            && d.height == p.ty.heightmap
                            && d.priority == p.ty.priority
                            && d.pass == p.pass
                            && d.verts == p.verts
                            && d.tris == p.tris
                    })
                });
            if same {
                made.extend(old.into_iter().map(DecalHandle));
                continue;
            }
            for h in old {
                self.destroy(DecalHandle(h));
            }
            for p in new {
                if let Some(h) = self.insert(
                    p.category,
                    &p.ty,
                    p.verts,
                    p.tris,
                    p.pass,
                    Some((source, cell)),
                ) {
                    made.push(h);
                }
            }
        }
        made
    }

    /// The decals whose bounds overlap the rectangle (our world), sorted by priority (the lowest first)
    /// and by creation, with the ones that have the wanted pass.
    pub fn in_area(&self, min: Vec2, max: Vec2, pass: u32) -> Vec<&DecalData> {
        let mut seen: Vec<u32> = Vec::new();
        for key in grid_range(min, max) {
            if let Some(list) = self.grid.get(&key) {
                seen.extend(list);
            }
        }
        seen.extend(&self.big);
        seen.sort_unstable();
        seen.dedup();
        let mut out: Vec<&DecalData> = seen
            .into_iter()
            .filter_map(|h| self.decals.get(&h))
            .filter(|d| d.pass & pass != 0 && d.overlaps(min, max))
            .collect();
        out.sort_by(|a, b| {
            a.priority
                .total_cmp(&b.priority)
                .then(a.handle.cmp(&b.handle))
        });
        out
    }
}

/// The triangles of a shape.
fn mesh_of(shape: &DecalShape) -> (Vec<DecalVertex>, Vec<[u16; 3]>) {
    match shape {
        DecalShape::Quad(c) => {
            let uv = [
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(0.0, 1.0),
            ];
            (
                (0..4)
                    .map(|i| DecalVertex {
                        pos: c[i],
                        uv: uv[i],
                    })
                    .collect(),
                vec![[0, 1, 2], [0, 2, 3]],
            )
        }
        DecalShape::Strip(pairs) => {
            let n = pairs.len();
            let mut verts = Vec::with_capacity(n * 2);
            for (i, [l, r]) in pairs.iter().enumerate() {
                let v = if n > 1 {
                    i as f32 / (n - 1) as f32
                } else {
                    0.0
                };
                verts.push(DecalVertex {
                    pos: *l,
                    uv: Vec2::new(0.0, v),
                });
                verts.push(DecalVertex {
                    pos: *r,
                    uv: Vec2::new(1.0, v),
                });
            }
            let mut tris = Vec::new();
            for i in 0..n.saturating_sub(1) {
                let a = (i * 2) as u16;
                tris.push([a, a + 1, a + 2]);
                tris.push([a + 1, a + 3, a + 2]);
            }
            (verts, tris)
        }
        DecalShape::Quads(quads) => {
            // re: ForestVis::BuildShadows 0x086703c6 (the texture coordinates and the indices `n, n + 1,
            // n + 2, n, n + 2, n + 3` of each quad)
            let uv = [
                Vec2::new(0.0, 1.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(0.0, 0.0),
            ];
            let mut verts = Vec::with_capacity(quads.len() * 4);
            let mut tris = Vec::with_capacity(quads.len() * 2);
            for c in quads {
                let n = verts.len() as u16;
                verts.extend((0..4).map(|i| DecalVertex {
                    pos: c[i],
                    uv: uv[i],
                }));
                tris.push([n, n + 1, n + 2]);
                tris.push([n, n + 2, n + 3]);
            }
            (verts, tris)
        }
        DecalShape::Mesh { verts, tris } => (
            verts
                .iter()
                .map(|&(pos, uv)| DecalVertex { pos, uv })
                .collect(),
            tris.clone(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::Extra;

    fn def(kind: &str, name: &str, priority: Option<f32>) -> DecalDef {
        DecalDef {
            kind: kind.into(),
            name: name.into(),
            albedo: Some(LogicalId::new(format!("tx:{name}.dds"))),
            heightmap: None,
            priority,
            extra: Extra::default(),
        }
    }

    fn layer() -> DecalLayer {
        let mut l = DecalLayer::new();
        l.set_types(&[
            def("Floodplains", "Floodplains_01", Some(0.4)),
            def("Floodplains", "Floodplains_02", Some(0.4)),
            def("Floodplains", "Floodplains_03", Some(0.4)),
            def("Main_Road", "Main_Road_01", Some(0.8)),
            def("Crops", "Crops_01", None),
        ]);
        l
    }

    fn quad(min: Vec2, size: f32) -> Vec<DecalVertex> {
        [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]
            .iter()
            .map(|&(u, v)| DecalVertex {
                pos: min + Vec2::new(u, v) * size,
                uv: Vec2::new(u, v),
            })
            .collect()
    }

    const TRIS: [[u16; 3]; 2] = [[0, 1, 2], [0, 2, 3]];

    fn cat(name: &str) -> DecalCategory {
        DecalCategory::of_name(name).unwrap()
    }

    #[test]
    fn a_category_with_three_types_picks_by_rand_mod_three() {
        let mut l = layer();
        let mut rng = MsvcRand::new(1);
        let mut mirror = MsvcRand::new(1);
        for _ in 0..6 {
            let h = l
                .create(
                    cat("Floodplains"),
                    quad(Vec2::ZERO, 10.0),
                    TRIS.to_vec(),
                    PASS_LAYER,
                    &mut rng,
                )
                .unwrap();
            let want = format!("tx:Floodplains_0{}.dds", mirror.next() % 3 + 1);
            assert_eq!(l.get(h).unwrap().albedo.as_ref().unwrap().as_str(), want);
        }
        // The stream was used once a decal.
        assert_eq!(rng.calls(), 6);
        // A category with no type makes nothing and does not use the stream.
        assert!(
            l.create(
                cat("Riverbank"),
                quad(Vec2::ZERO, 10.0),
                TRIS.to_vec(),
                PASS_LAYER,
                &mut rng
            )
            .is_none()
        );
        assert_eq!(rng.calls(), 6);
    }

    #[test]
    fn create_and_create_rolled_pick_the_same_type_for_the_same_draw() {
        let mut a = layer();
        let mut b = layer();
        let mut rng = MsvcRand::new(5);
        let mut rolls = MsvcRand::new(5);
        for _ in 0..9 {
            let ha = a
                .create(
                    cat("Floodplains"),
                    quad(Vec2::ZERO, 10.0),
                    TRIS.to_vec(),
                    PASS_LAYER,
                    &mut rng,
                )
                .unwrap();
            let hb = b
                .create_rolled(
                    cat("Floodplains"),
                    rolls.next(),
                    quad(Vec2::ZERO, 10.0),
                    TRIS.to_vec(),
                    PASS_LAYER,
                )
                .unwrap();
            assert_eq!(a.get(ha).unwrap().albedo, b.get(hb).unwrap().albedo);
        }
        // No type: nothing, as `create`.
        assert!(
            b.create_rolled(
                cat("Riverbank"),
                3,
                quad(Vec2::ZERO, 10.0),
                TRIS.to_vec(),
                PASS_LAYER
            )
            .is_none()
        );
    }

    #[test]
    fn creating_and_destroying_mark_only_the_area_of_the_decal() {
        let mut l = layer();
        let h = l
            .create_typed(
                cat("Crops"),
                0,
                quad(Vec2::new(100.0, 200.0), 30.0),
                TRIS.to_vec(),
                PASS_LAYER,
            )
            .unwrap();
        assert_eq!(
            l.take_dirty(),
            vec![(Vec2::new(100.0, 200.0), Vec2::new(130.0, 230.0))]
        );
        assert!(l.take_dirty().is_empty());
        let rev = l.revision();
        assert!(l.destroy(h));
        assert!(!l.destroy(h));
        assert_eq!(l.take_dirty().len(), 1);
        assert!(l.revision() > rev);
        assert!(l.is_empty());
    }

    #[test]
    fn the_grid_finds_the_decals_of_an_area_and_the_big_ones_are_kept_apart() {
        let mut l = layer();
        let near = l
            .create_typed(
                cat("Crops"),
                0,
                quad(Vec2::new(10.0, 10.0), 20.0),
                TRIS.to_vec(),
                PASS_LAYER,
            )
            .unwrap();
        let far = l
            .create_typed(
                cat("Crops"),
                0,
                quad(Vec2::new(1000.0, 1000.0), 20.0),
                TRIS.to_vec(),
                PASS_LAYER,
            )
            .unwrap();
        let big = l
            .create_typed(
                cat("Main_Road"),
                0,
                quad(Vec2::new(-300.0, -300.0), 600.0),
                TRIS.to_vec(),
                PASS_LAYER,
            )
            .unwrap();
        assert_eq!(l.big_count(), 1);
        let found: Vec<_> = l
            .in_area(Vec2::ZERO, Vec2::new(64.0, 64.0), PASS_LAYER)
            .iter()
            .map(|d| d.handle)
            .collect();
        assert!(found.contains(&near) && found.contains(&big) && !found.contains(&far));
        // The lowest priority first: crops (0.1) under the road (0.8).
        assert_eq!(found, vec![near, big]);
        // Only the decals with the pass asked for.
        assert!(
            l.in_area(Vec2::ZERO, Vec2::new(64.0, 64.0), PASS_SHADOW)
                .is_empty()
        );
        l.destroy(big);
        assert_eq!(l.big_count(), 0);
    }

    #[test]
    fn a_bank_holds_at_most_0xffff_vertices() {
        let mut l = layer();
        let ring = |n: usize| -> (Vec<DecalVertex>, Vec<[u16; 3]>) {
            let verts = (0..n)
                .map(|i| DecalVertex {
                    pos: Vec2::new(i as f32 * 0.001, 0.0),
                    uv: Vec2::ZERO,
                })
                .collect();
            (verts, vec![[0, 1, 2]])
        };
        let (v, t) = ring(40_000);
        let a = l.create_typed(cat("Crops"), 0, v, t, PASS_LAYER).unwrap();
        let (v, t) = ring(30_000);
        let b = l.create_typed(cat("Crops"), 0, v, t, PASS_LAYER).unwrap();
        assert_eq!(l.get(a).unwrap().bank, 0);
        assert_eq!(l.get(b).unwrap().bank, 1, "does not fit the first bank");
        assert_eq!(l.banks().len(), 2);
        // A mesh larger than a bank, or with an index outside it, is refused.
        let (v, t) = ring(70_000);
        assert!(l.create_typed(cat("Crops"), 0, v, t, PASS_LAYER).is_none());
        assert!(
            l.create_typed(
                cat("Crops"),
                0,
                quad(Vec2::ZERO, 1.0),
                vec![[0, 1, 9]],
                PASS_LAYER
            )
            .is_none()
        );
        // The room of a destroyed decal is given back.
        l.destroy(a);
        let (v, t) = ring(30_000);
        let c = l.create_typed(cat("Crops"), 0, v, t, PASS_LAYER).unwrap();
        assert_eq!(l.get(c).unwrap().bank, 0);
    }

    fn road(cell: u32, at: f32) -> ObjectDecal {
        ObjectDecal {
            category: cat("Main_Road"),
            texture: None,
            shape: DecalShape::Strip(vec![
                [Vec2::new(at, 0.0), Vec2::new(at + 4.0, 0.0)],
                [Vec2::new(at, 30.0), Vec2::new(at + 4.0, 30.0)],
            ]),
            cell,
            casts_shadow: false,
        }
    }

    #[test]
    fn a_call_replaces_the_decals_of_its_source_on_its_hexes_only() {
        let mut l = layer();
        let first = l.set_object_decals(
            DecalSource::Roads,
            &[1, 2],
            vec![road(1, 0.0), road(2, 100.0), road(2, 110.0)],
        );
        assert_eq!(first.len(), 3);
        assert_eq!(l.len(), 3);
        l.take_dirty();
        // A farm on hex 1 stays when the roads of hex 1 are replaced; the roads of hex 2 stay.
        let farm = l.set_object_decals(
            DecalSource::Farms,
            &[1],
            vec![ObjectDecal {
                category: cat("Crops"),
                texture: Some(LogicalId::new("tx:own_crops.dds")),
                shape: DecalShape::Quad([
                    Vec2::new(0.0, 0.0),
                    Vec2::new(8.0, 0.0),
                    Vec2::new(8.0, 8.0),
                    Vec2::new(0.0, 8.0),
                ]),
                cell: 1,
                casts_shadow: false,
            }],
        );
        assert_eq!(
            l.get(farm[0]).unwrap().albedo.as_ref().unwrap().as_str(),
            "tx:own_crops.dds"
        );
        l.take_dirty();
        let again = l.set_object_decals(DecalSource::Roads, &[1], vec![road(1, 50.0)]);
        assert_eq!(again.len(), 1);
        assert!(l.get(first[0]).is_none(), "the road of hex 1 was replaced");
        assert!(l.get(first[1]).is_some() && l.get(first[2]).is_some());
        assert!(l.get(farm[0]).is_some());
        assert_eq!(l.len(), 4);
        // The old road and the new one are both marked.
        assert_eq!(l.take_dirty().len(), 2);
        // An empty batch for hexes only takes the old ones away.
        l.set_object_decals(DecalSource::Roads, &[2], vec![]);
        assert_eq!(l.len(), 2);
    }

    #[test]
    fn the_variant_of_an_object_decal_follows_its_hex_and_not_the_order_of_the_batch() {
        let pick = |order: &[u32]| -> Vec<String> {
            let mut l = layer();
            let batch: Vec<ObjectDecal> = order
                .iter()
                .map(|&c| ObjectDecal {
                    category: cat("Floodplains"),
                    texture: None,
                    shape: DecalShape::Quad([Vec2::ZERO; 4]),
                    cell: c,
                    casts_shadow: false,
                })
                .collect();
            let made = l.set_object_decals(DecalSource::Other(0), &[1, 2, 3, 4], batch);
            let mut by_cell: Vec<(u32, String)> = made
                .iter()
                .map(|&h| {
                    let d = l.get(h).unwrap();
                    (
                        d.source.unwrap().1,
                        d.albedo.clone().unwrap().as_str().to_owned(),
                    )
                })
                .collect();
            by_cell.sort();
            by_cell.into_iter().map(|(_, a)| a).collect()
        };
        assert_eq!(pick(&[1, 2, 3, 4]), pick(&[4, 3, 2, 1]));
    }

    #[test]
    fn a_strip_makes_two_triangles_a_segment() {
        let (verts, tris) = mesh_of(&DecalShape::Strip(vec![
            [Vec2::ZERO, Vec2::X],
            [Vec2::Y, Vec2::ONE],
            [Vec2::Y * 2.0, Vec2::new(1.0, 2.0)],
        ]));
        assert_eq!(verts.len(), 6);
        assert_eq!(tris.len(), 4);
        assert_eq!(verts[4].uv, Vec2::new(0.0, 1.0));
    }

    /// The shadows of the trees of a hex: one decal of many quads, in the macro shadow pass only.
    #[test]
    fn the_quads_of_a_shadow_are_one_decal_of_the_shadow_pass() {
        let q = |x: f32| {
            [
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, 1.0),
                Vec2::new(x, 1.0),
            ]
        };
        let (verts, tris) = mesh_of(&DecalShape::Quads(vec![q(0.0), q(5.0)]));
        assert_eq!(verts.len(), 8);
        assert_eq!(tris, [[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]]);
        assert_eq!(verts[4].uv, Vec2::new(0.0, 1.0));
        assert_eq!(verts[6].uv, Vec2::new(1.0, 0.0));

        let mut l = layer();
        l.set_types(&[def("Tree_Shadow", "Tree_Shadow_01", None)]);
        let made = l.set_object_decals(
            DecalSource::Trees,
            &[7],
            vec![ObjectDecal {
                category: cat("Tree_Shadow"),
                texture: None,
                shape: DecalShape::Quads(vec![q(0.0), q(5.0)]),
                cell: 7,
                casts_shadow: true,
            }],
        );
        assert_eq!(made.len(), 1);
        assert_eq!(l.get(made[0]).unwrap().pass, PASS_SHADOW);
        let area = (Vec2::splat(-10.0), Vec2::splat(10.0));
        assert!(l.in_area(area.0, area.1, PASS_LAYER).is_empty());
        assert_eq!(l.in_area(area.0, area.1, PASS_SHADOW).len(), 1);
    }

    fn ground(cell: u32, at: f32) -> MeshDecal {
        MeshDecal {
            albedo: LogicalId::new("tx:ground.dds"),
            height: None,
            priority: 0.5,
            verts: quad(Vec2::new(at, 0.0), 10.0),
            tris: TRIS.to_vec(),
            cell,
        }
    }

    #[test]
    fn the_same_object_decals_again_change_nothing() {
        let mut l = layer();
        let batch = || vec![road(1, 0.0), road(2, 100.0), road(2, 110.0)];
        let first = l.set_object_decals(DecalSource::Roads, &[1, 2], batch());
        let revision = l.revision();
        l.take_dirty();
        let again = l.set_object_decals(DecalSource::Roads, &[1, 2], batch());
        assert_eq!(again, first);
        assert_eq!(l.revision(), revision);
        assert!(l.take_dirty().is_empty());
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn the_same_mesh_decals_again_change_nothing() {
        let mut l = layer();
        let batch = || vec![ground(1, 0.0), ground(2, 100.0)];
        let first = l.set_mesh_decals(DecalSource::Cities, &[1, 2], batch());
        assert_eq!(first.len(), 2);
        let revision = l.revision();
        l.take_dirty();
        let again = l.set_mesh_decals(DecalSource::Cities, &[1, 2], batch());
        assert_eq!(again, first);
        assert_eq!(l.revision(), revision);
        assert!(l.take_dirty().is_empty());
    }

    #[test]
    fn only_the_hex_that_changed_is_made_again() {
        let mut l = layer();
        let first = l.set_object_decals(
            DecalSource::Roads,
            &[1, 2],
            vec![road(1, 0.0), road(2, 100.0)],
        );
        l.take_dirty();
        let again = l.set_object_decals(
            DecalSource::Roads,
            &[1, 2],
            vec![road(1, 0.0), road(2, 200.0)],
        );
        assert_eq!(again[0], first[0], "hex 1 keeps its decal");
        assert_ne!(again[1], first[1]);
        assert!(l.get(first[1]).is_none());
        // The old and the new road of hex 2 are marked, nothing of hex 1.
        let dirty = l.take_dirty();
        assert_eq!(dirty.len(), 2);
        assert!(dirty.iter().all(|(min, _)| min.x >= 100.0));
    }

    #[test]
    fn the_dirty_areas_are_counted_by_source() {
        let mut l = layer();
        l.set_object_decals(DecalSource::Roads, &[1], vec![road(1, 0.0)]);
        l.set_object_decals(DecalSource::Roads, &[1], vec![road(1, 50.0)]);
        l.set_mesh_decals(DecalSource::Cities, &[3], vec![ground(3, 0.0)]);
        l.set_mesh_decals(DecalSource::Other(4), &[5], vec![ground(5, 0.0)]);
        l.create_typed(
            cat("Crops"),
            0,
            quad(Vec2::ZERO, 1.0),
            TRIS.to_vec(),
            PASS_LAYER,
        );
        let counters = l.dirty_counters();
        assert_eq!(counters.len(), 7);
        let get = |name: &str| counters.iter().find(|(n, _)| *n == name).unwrap().1;
        // A road made, then destroyed and a new one made.
        assert_eq!(get("terrain.decal_dirty.roads_total"), 3);
        assert_eq!(get("terrain.decal_dirty.cities_total"), 1);
        assert_eq!(get("terrain.decal_dirty.other_total"), 1);
        assert_eq!(get("terrain.decal_dirty.build_total"), 1);
        assert_eq!(get("terrain.decal_dirty.farms_total"), 0);
        assert_eq!(get("terrain.decal_dirty.trees_total"), 0);
        assert_eq!(get("terrain.decal_dirty.landmarks_total"), 0);
    }
}
