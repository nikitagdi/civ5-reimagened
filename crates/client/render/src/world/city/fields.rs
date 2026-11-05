//! The authored fields of the city layout (world-objects 4.4, WOB5): the density fields that say
//! where a house of which height class may stand, by the hex-edge pattern of the city and its
//! population, and the two tensor fields that turn the houses and bend the distance between them.
//!
//! re: `cvLayoutFieldManager::LoadAllDensityFields` (0x087db3d6), `LoadDensityFieldFromFile`
//! (0x087db7a4), `LoadAllTensorFields` (0x087dbc80), `LoadTensorFieldFromFile` (0x087dbe8a),
//! `Fun_087bb370` (the field of a pattern and a population), `TransformMaskThroughRotation` /
//! `TransformMaskThroughReflection` (0x087db248 / 0x087db30f), the matrices `HEXMatrix::REFLECTIONS`
//! and `HEXMatrix_i::ROTATIONS` (static init 0x0848c5b0).
//!
//! The files are `assets\LayoutSystem\DF_flat_A1..A10.dds`, `DF_<edge digits>_A1..A10.dds` for the
//! six base patterns, and `TF_Small_City.dds` / `TF_Large_City.dds`, 128x128 A8R8G8B8. The client
//! gets them as `tx:` textures of the set `b2-city` (level 0, the bytes of the file).

use glam::Vec2;

/// Side of a density field and of the layout square (rect-local units).
pub const FIELD_SIDE: usize = 128;

/// Side of a tensor field (the file is halved by a 2x2 box filter).
pub const TENSOR_SIDE: usize = 64;

/// Population fields per pattern (`_A1` .. `_A10`; population `p` reads slot `p`).
pub const POPULATIONS: usize = 10;

/// The class of a lot that holds no house.
pub const CLASS_NONE: u8 = 4;

/// `ms_amCityHexPatterns` (0x0920c378): the flat pattern and the six base patterns.
pub const BASE_PATTERNS: [u8; 7] = [0x00, 0x01, 0x02, 0x21, 0x22, 0x31, 0x2a];

/// The four reflections each base pattern is loaded with (0x0920c2f0).
pub const PATTERN_REFLECTIONS: [usize; 4] = [0, 2, 6, 9];

/// The class thresholds of a density texel (0x0920c2f4): the class is the last index whose
/// threshold is at most the texel, [`CLASS_NONE`] below the first.
const CLASS_THRESHOLDS: [u8; 4] = [0x31, 0x63, 0x95, 0xc7];

/// `HEXVec_i::TRANSLATIONS`: the cube steps to the six neighbours, the bit order of the edge masks.
pub const TRANSLATIONS: [[i32; 3]; 6] = [
    [0, 1, -1],
    [-1, 0, 1],
    [1, -1, 0],
    [0, -1, 1],
    [1, 0, -1],
    [-1, 1, 0],
];

/// `HEXMatrix::REFLECTIONS` (rows; a cube vector `v` becomes `v.x*row0 + v.y*row1 + v.z*row2`).
pub const REFLECTIONS: [[[i32; 3]; 3]; 12] = [
    [[1, 0, 0], [0, 1, 0], [0, 0, 1]],
    [[-1, 0, 0], [0, 0, -1], [0, -1, 0]],
    [[0, 0, -1], [0, -1, 0], [-1, 0, 0]],
    [[0, -1, 0], [-1, 0, 0], [0, 0, -1]],
    [[0, 1, 0], [1, 0, 0], [0, 0, 1]],
    [[1, 0, 0], [0, 0, 1], [0, 1, 0]],
    [[0, 0, 1], [0, 1, 0], [1, 0, 0]],
    [[0, -1, 0], [0, 0, -1], [-1, 0, 0]],
    [[0, 0, 1], [1, 0, 0], [0, 1, 0]],
    [[-1, 0, 0], [0, -1, 0], [0, 0, -1]],
    [[0, 1, 0], [0, 0, 1], [1, 0, 0]],
    [[0, 0, -1], [-1, 0, 0], [0, -1, 0]],
];

/// `HEXMatrix_i::ROTATIONS[5]`, the one the masks are turned by before their reflections.
const ROTATION_5: [[i32; 3]; 3] = [[0, -1, 0], [0, 0, -1], [-1, 0, 0]];

/// `kHexRowStretch` of `ResetPopBuildingsFromPlacementsInCity`: world y back to tex y.
pub const HEX_ROW_STRETCH: f32 = 1.154_734_4;

/// `g_TerrainScale.y`: tex y to world y.
pub const ROW_SCALE: f32 = 0.866_025_4;

fn apply(m: &[[i32; 3]; 3], v: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|i| v[0] * m[0][i] + v[1] * m[1][i] + v[2] * m[2][i])
}

fn transform_mask(mask: u8, m: &[[i32; 3]; 3]) -> u8 {
    let mut out = 0;
    for (bit, t) in TRANSLATIONS.iter().enumerate() {
        if mask >> bit & 1 != 0 {
            let r = apply(m, *t);
            if let Some(k) = TRANSLATIONS.iter().position(|t| *t == r) {
                out |= 1 << k;
            }
        }
    }
    out
}

/// `TransformMaskThroughRotation(mask, 5)` then `TransformMaskThroughReflection(_, r)`.
pub fn pattern_mask(base: u8, reflection: usize) -> u8 {
    transform_mask(transform_mask(base, &ROTATION_5), &REFLECTIONS[reflection])
}

/// The file stem of a base pattern: `df_` and the digits of its set bits (`flat` for none), lower
/// case as the ids of the textures are.
pub fn pattern_stem(base: u8) -> String {
    if base == 0 {
        return "df_flat".into();
    }
    let digits: String = (0..6)
        .filter(|b| base >> b & 1 != 0)
        .map(|b| char::from(b'0' + b as u8))
        .collect();
    format!("df_{digits}")
}

/// The file names of every field the layout reads (lower case).
pub fn file_names() -> Vec<String> {
    let mut out: Vec<String> = BASE_PATTERNS
        .iter()
        .flat_map(|&b| {
            let stem = pattern_stem(b);
            (1..=POPULATIONS).map(move |p| format!("{stem}_a{p}.dds"))
        })
        .collect();
    out.extend(TENSOR_FILES.iter().map(|f| (*f).to_owned()));
    out
}

/// The tensor fields: set 0 for a population up to 6, set 1 above.
pub const TENSOR_FILES: [&str; 2] = ["tf_small_city.dds", "tf_large_city.dds"];

/// The class of a texel of a density file (`LoadDensityFieldFromFile`).
pub fn class_of_texel(v: u8) -> u8 {
    let v1 = u16::from(v) + 1;
    let mut class = CLASS_NONE;
    for (i, t) in CLASS_THRESHOLDS.iter().enumerate() {
        if u16::from(*t) < v1 {
            class = i as u8;
        }
    }
    class
}

/// One density field: 128x128 classes, row 0 the last row of the file.
#[derive(Clone)]
pub struct DensityField {
    pub cells: Box<[u8; FIELD_SIDE * FIELD_SIDE]>,
}

impl DensityField {
    /// From a 128x128 RGBA8 image (the bytes of the file in R, G, B, A order). The original reads
    /// the first byte of the A8R8G8B8 texel, the blue one.
    pub fn from_rgba(rgba: &[u8]) -> Option<Self> {
        if rgba.len() < FIELD_SIDE * FIELD_SIDE * 4 {
            return None;
        }
        let mut cells = Box::new([0u8; FIELD_SIDE * FIELD_SIDE]);
        for row in 0..FIELD_SIDE {
            for col in 0..FIELD_SIDE {
                let texel = (row * FIELD_SIDE + col) * 4;
                cells[(FIELD_SIDE - 1 - row) * FIELD_SIDE + col] = class_of_texel(rgba[texel + 2]);
            }
        }
        Some(Self { cells })
    }

    /// The class at a field point (0..128 both ways); `None` outside (`Fun_087bb370`: rect 0..128,
    /// scale 127/128).
    pub fn class_at(&self, f: Vec2) -> Option<u8> {
        let side = FIELD_SIDE as f32;
        if !(f.x < side && f.x >= 0.0 && f.y < side && f.y >= 0.0) {
            return None;
        }
        let scale = (FIELD_SIDE - 1) as f32 * 0.0078125;
        let cx = (f.x * scale) as usize;
        let cy = (f.y * scale) as usize;
        Some(self.cells[cy * FIELD_SIDE + cx])
    }
}

/// A tensor field: 64x64 signed byte pairs, row 0 the last row of the file.
#[derive(Clone)]
pub struct TensorField {
    pub cells: Vec<[i8; 2]>,
}

impl TensorField {
    /// From a 128x128 RGBA8 image: x = G ^ 0x80, y = R ^ 0x80, the rows turned over, then the mean
    /// of each 2x2 block (truncated, `LoadTensorFieldFromFile`).
    pub fn from_rgba(rgba: &[u8]) -> Option<Self> {
        let side = FIELD_SIDE;
        if rgba.len() < side * side * 4 {
            return None;
        }
        let at = |x: usize, y: usize| {
            // `y` is the turned-over row.
            let texel = ((side - 1 - y) * side + x) * 4;
            [(rgba[texel + 1] ^ 0x80) as i8, (rgba[texel] ^ 0x80) as i8]
        };
        let mut cells = Vec::with_capacity(TENSOR_SIDE * TENSOR_SIDE);
        for j in 0..TENSOR_SIDE {
            for i in 0..TENSOR_SIDE {
                let mut sum = [0.0f32; 2];
                for (dx, dy) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let c = at(2 * i + dx, 2 * j + dy);
                    sum[0] += f32::from(c[0]);
                    sum[1] += f32::from(c[1]);
                }
                cells.push([(sum[0] * 0.25) as i32 as i8, (sum[1] * 0.25) as i32 as i8]);
            }
        }
        Some(Self { cells })
    }

    /// A field of one direction everywhere (tests).
    pub fn uniform(x: i8, y: i8) -> Self {
        Self {
            cells: vec![[x, y]; TENSOR_SIDE * TENSOR_SIDE],
        }
    }

    pub fn cell(&self, col: usize, row: usize) -> [i8; 2] {
        self.cells[row.min(TENSOR_SIDE - 1) * TENSOR_SIDE + col.min(TENSOR_SIDE - 1)]
    }
}

/// One entry of the pattern list: which field set, the edge mask it serves, its reflection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pattern {
    pub field_set: usize,
    pub edge_mask: u8,
    pub reflection: usize,
}

/// Every field of the layout.
#[derive(Clone)]
pub struct LayoutFields {
    pub patterns: Vec<Pattern>,
    /// `[set][population]`.
    pub sets: Vec<Vec<DensityField>>,
    pub tensors: [TensorField; 2],
}

impl LayoutFields {
    /// The fields from the images by file name (lower case, see [`file_names`]); `None` without
    /// the flat set or a tensor field. A base pattern whose files are not all there is left out,
    /// as the original does when its set does not load.
    pub fn from_images(get: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<Self> {
        let load_set = |base: u8| -> Option<Vec<DensityField>> {
            let stem = pattern_stem(base);
            (1..=POPULATIONS)
                .map(|p| DensityField::from_rgba(&get(&format!("{stem}_a{p}.dds"))?))
                .collect()
        };
        let mut sets = vec![load_set(0)?];
        let mut patterns = vec![Pattern {
            field_set: 0,
            edge_mask: 0,
            reflection: 0,
        }];
        for &base in &BASE_PATTERNS[1..] {
            let Some(set) = load_set(base) else { continue };
            let field_set = sets.len();
            sets.push(set);
            for r in PATTERN_REFLECTIONS {
                patterns.push(Pattern {
                    field_set,
                    edge_mask: pattern_mask(base, r),
                    reflection: r,
                });
            }
        }
        let tensor = |i: usize| TensorField::from_rgba(&get(TENSOR_FILES[i])?);
        Some(Self {
            patterns,
            sets,
            tensors: [tensor(0)?, tensor(1)?],
        })
    }

    /// The pattern of an edge mask: the first whose mask it is, else the first of all (flat).
    pub fn pattern_of(&self, edges: u8) -> Pattern {
        self.patterns
            .iter()
            .find(|p| p.edge_mask == edges)
            .copied()
            .unwrap_or(self.patterns[0])
    }

    /// The density field of a pattern and a population. A population past the last field reads
    /// the last one (the original reads past the block, a latent overflow).
    pub fn density(&self, pattern: Pattern, population: i32) -> &DensityField {
        let set = &self.sets[pattern.field_set];
        &set[(population.max(0) as usize).min(set.len() - 1)]
    }

    /// The tensor field of a population (`pop > 6` takes the large city).
    pub fn tensor(&self, population: i32) -> &TensorField {
        &self.tensors[usize::from(population > 6)]
    }
}

/// A point of the tex space relative to the city (the hex is 64 wide) to the field space of a
/// pattern: tex to hex, the reflection, back to tex, and the centre at 64.
pub fn tex_to_field(tex: Vec2, reflection: usize) -> Vec2 {
    let hy = tex.y / 64.0;
    let hx = tex.x / 64.0 - tex.y / 128.0;
    let hz = -hx - hy;
    let m = &REFLECTIONS[reflection];
    let r: [f32; 3] =
        std::array::from_fn(|i| hx * m[0][i] as f32 + hy * m[1][i] as f32 + hz * m[2][i] as f32);
    Vec2::new(64.0 * r[0] + 32.0 * r[1] + 64.0, 64.0 * r[1] + 64.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_texel_classes_follow_the_thresholds() {
        assert_eq!(class_of_texel(0), CLASS_NONE);
        assert_eq!(class_of_texel(48), CLASS_NONE);
        assert_eq!(class_of_texel(49), 0);
        assert_eq!(class_of_texel(50), 0);
        assert_eq!(class_of_texel(100), 1);
        assert_eq!(class_of_texel(150), 2);
        assert_eq!(class_of_texel(200), 3);
        assert_eq!(class_of_texel(255), 3);
    }

    #[test]
    fn the_masks_turn_with_the_hex() {
        // Rotation 5 sends bit 0 to 4, 1 to 5, 2 to 3.
        assert_eq!(pattern_mask(0x01, 0), 0x10);
        assert_eq!(pattern_mask(0x02, 0), 0x20);
        assert_eq!(pattern_mask(0x21, 0), 0x11);
        // The point reflection swaps each direction with its opposite.
        assert_eq!(pattern_mask(0x01, 9), 0x02);
        assert_eq!(pattern_stem(0x21), "df_05");
        assert_eq!(pattern_stem(0x31), "df_045");
        assert_eq!(pattern_stem(0x2a), "df_135");
        assert_eq!(file_names().len(), 72);
    }

    #[test]
    fn the_city_is_the_middle_of_the_field_and_a_hex_is_64_wide() {
        for r in PATTERN_REFLECTIONS {
            assert_eq!(tex_to_field(Vec2::ZERO, r), Vec2::splat(64.0));
        }
        assert_eq!(
            tex_to_field(Vec2::new(64.0, 0.0), 0),
            Vec2::new(128.0, 64.0)
        );
        // The point reflection turns the field half round.
        assert_eq!(
            tex_to_field(Vec2::new(10.0, 20.0), 9),
            Vec2::new(54.0, 44.0)
        );
    }

    #[test]
    fn a_density_file_is_turned_over_and_a_tensor_file_halved() {
        let mut rgba = vec![0u8; 128 * 128 * 4];
        // The second row of the file (row 126 of the field; row 127 is never read), blue 200.
        for x in 0..128 {
            rgba[(128 + x) * 4 + 2] = 200;
        }
        let d = DensityField::from_rgba(&rgba).unwrap();
        assert_eq!(d.class_at(Vec2::new(5.0, 127.5)), Some(3));
        assert_eq!(d.class_at(Vec2::new(5.0, 3.0)), Some(CLASS_NONE));
        assert_eq!(d.class_at(Vec2::new(128.0, 3.0)), None);
        let mut t = vec![0x80u8; 128 * 128 * 4];
        t[0] = 0x80 + 40; // R of the first texel: y
        t[1] = 0x80 - 20; // G: x
        let tf = TensorField::from_rgba(&t).unwrap();
        assert_eq!(tf.cell(0, 63), [-5, 10]);
        assert_eq!(tf.cell(0, 0), [0, 0]);
    }
}
