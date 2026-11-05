//! The style library of the highlights (world-overlays WOV5, `HighlightLibrary`, RW-20 (a)).
//!
//! `Highlights.xml` of the copy that acts (Lekmod's, 34 styles; RW-9 (b) says which copy the original
//! reads, until then Lekmod's: the mod loads last) is read by the asset pipeline into
//! [`asset_format::HighlightStyle`]s; this turns them into styles of the five shapes with numbers:
//! the colour as floats, a width where the shape has one. A colour of five numbers (`EnemyOutline`
//! has `255,0,0,0,200`) keeps the first four and is a warning; how the original reads it is RW-20 (a).

use std::collections::{BTreeSet, HashMap};

use asset_format::{HighlightKind, HighlightStyle, LogicalId};

/// Width of a `HexContour` that has none: the fraction of the hex, like the grid's.
// re: Highlights.xml (every `HexContour` of the file has width .2 or less); a missing width is ours
pub const DEFAULT_CONTOUR_WIDTH: f32 = 0.2;
/// Width of a `SplineBorder` that has none, world units.
// re: Highlights.xml:  `width="6"` to `7` on every `SplineBorder` of the file
pub const DEFAULT_SPLINE_WIDTH: f32 = 6.0;

/// The five kinds of shape and what each has of its own.
#[derive(Debug, Clone, PartialEq)]
pub enum Shape {
    /// The outline of each hex: a ring `width` (a part of the corner radius, as the grid's) wide
    /// with the profile `texture` across it.
    HexContour {
        width: f32,
        texture: Option<LogicalId>,
    },
    /// A strip `width` world units wide along the spline round the set of hexes, from the edge
    /// inwards, with the profile `texture` across it (without a texture: one colour).
    SplineBorder {
        width: f32,
        texture: Option<LogicalId>,
    },
    /// Each hex filled, `width` the part of the hex covered (1 is all of it).
    FilledHex { width: f32 },
    /// A texture over the box of each hex.
    StaticTexture { texture: Option<LogicalId> },
    /// A model on each hex (`SelectedAniOneTurn.fxsxml`), animated through `AnimService`.
    HexModel { model: String },
}

/// One style: a name, a shape and the colour the file gives it, if it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub name: String,
    pub shape: Shape,
    /// RGBA, 0 to 1.
    pub color: Option<[f32; 4]>,
}

/// The styles by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HighlightLibrary {
    styles: Vec<Style>,
    by_name: HashMap<String, usize>,
    warnings: Vec<String>,
}

/// The numbers of a `color` attribute as floats; `None` (and a warning) when there are fewer than
/// three or more than five.
fn color(name: &str, numbers: &[u8], warnings: &mut Vec<String>) -> Option<[f32; 4]> {
    let c = |i: usize| f32::from(numbers[i]) / 255.0;
    match numbers.len() {
        0 => None,
        3 => Some([c(0), c(1), c(2), 1.0]),
        4 => Some([c(0), c(1), c(2), c(3)]),
        5 => {
            warnings.push(format!(
                "Highlights.xml: style {name:?} has 5 numbers in its colour; the first four are used"
            ));
            Some([c(0), c(1), c(2), c(3)])
        }
        n => {
            warnings.push(format!(
                "Highlights.xml: style {name:?} has {n} numbers in its colour; it is left out"
            ));
            None
        }
    }
}

impl HighlightLibrary {
    /// The library of the styles of the file, in file order. A name that comes twice keeps the last.
    pub fn from_styles(styles: &[HighlightStyle]) -> Self {
        let mut lib = Self::default();
        for s in styles {
            let shape = match s.kind {
                HighlightKind::HexContour => Shape::HexContour {
                    width: s.width.unwrap_or(DEFAULT_CONTOUR_WIDTH),
                    texture: s.texture.clone(),
                },
                HighlightKind::SplineBorder => Shape::SplineBorder {
                    width: s.width.unwrap_or(DEFAULT_SPLINE_WIDTH),
                    texture: s.texture.clone(),
                },
                HighlightKind::FilledHex => Shape::FilledHex {
                    width: s.width.unwrap_or(1.0),
                },
                HighlightKind::StaticTexture => Shape::StaticTexture {
                    texture: s.texture.clone(),
                },
                HighlightKind::HexModel => Shape::HexModel {
                    model: s.model.clone().unwrap_or_default(),
                },
            };
            let style = Style {
                name: s.name.clone(),
                shape,
                color: color(&s.name, &s.color, &mut lib.warnings),
            };
            match lib.by_name.get(&s.name) {
                Some(&i) => lib.styles[i] = style,
                None => {
                    lib.by_name.insert(s.name.clone(), lib.styles.len());
                    lib.styles.push(style);
                }
            }
        }
        lib
    }

    pub fn get(&self, name: &str) -> Option<&Style> {
        self.by_name.get(name).map(|&i| &self.styles[i])
    }

    pub fn len(&self) -> usize {
        self.styles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.styles.is_empty()
    }

    /// The styles in file order.
    pub fn iter(&self) -> impl Iterator<Item = &Style> {
        self.styles.iter()
    }

    /// What the reading found odd.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The textures the styles name (the profiles and the static texture).
    pub fn texture_ids(&self) -> BTreeSet<LogicalId> {
        self.styles
            .iter()
            .filter_map(|s| match &s.shape {
                Shape::HexContour { texture, .. }
                | Shape::SplineBorder { texture, .. }
                | Shape::StaticTexture { texture } => texture.clone(),
                _ => None,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::overlay::highlights::lekmod_table::STYLES;

    /// The styles of the table as the pipeline reads them: the texture is its `tx:` id.
    fn from_table() -> Vec<HighlightStyle> {
        STYLES
            .iter()
            .map(
                |&(name, kind, width, texture, model, colour)| HighlightStyle {
                    name: name.into(),
                    kind: match kind {
                        "HexContour" => HighlightKind::HexContour,
                        "FilledHex" => HighlightKind::FilledHex,
                        "StaticTexture" => HighlightKind::StaticTexture,
                        "HexModel" => HighlightKind::HexModel,
                        "SplineBorder" => HighlightKind::SplineBorder,
                        other => panic!("{other}"),
                    },
                    width,
                    texture: texture.map(|t| LogicalId::new(format!("tx:{}", t.to_lowercase()))),
                    model: model.map(str::to_string),
                    color: colour.to_vec(),
                    extra: Default::default(),
                },
            )
            .collect()
    }

    #[test]
    fn all_34_styles_of_lekmod_are_read() {
        assert_eq!(STYLES.len(), 34);
        let lib = HighlightLibrary::from_styles(&from_table());
        assert_eq!(lib.len(), 34);
        for &(name, kind, width, texture, model, _) in STYLES.iter() {
            let s = lib.get(name).unwrap_or_else(|| panic!("style {name:?}"));
            match (&s.shape, kind) {
                (
                    Shape::HexContour {
                        width: w,
                        texture: t,
                    },
                    "HexContour",
                ) => {
                    assert_eq!(*w, width.unwrap(), "{name}");
                    assert_eq!(t.is_some(), texture.is_some(), "{name}");
                }
                (
                    Shape::SplineBorder {
                        width: w,
                        texture: t,
                    },
                    "SplineBorder",
                ) => {
                    assert_eq!(*w, width.unwrap(), "{name}");
                    assert_eq!(t.is_some(), texture.is_some(), "{name}");
                }
                (Shape::FilledHex { width: w }, "FilledHex") => {
                    assert_eq!(*w, width.unwrap(), "{name}")
                }
                (Shape::StaticTexture { texture: t }, "StaticTexture") => {
                    assert_eq!(t.is_some(), texture.is_some(), "{name}")
                }
                (Shape::HexModel { model: m }, "HexModel") => {
                    assert_eq!(Some(m.as_str()), model, "{name}")
                }
                (shape, kind) => panic!("{name}: {kind} read as {shape:?}"),
            }
        }
    }

    #[test]
    fn the_kinds_of_the_file_are_the_five_shapes() {
        let lib = HighlightLibrary::from_styles(&from_table());
        let count = |f: fn(&Shape) -> bool| lib.iter().filter(|s| f(&s.shape)).count();
        assert_eq!(count(|s| matches!(s, Shape::HexContour { .. })), 8);
        assert_eq!(count(|s| matches!(s, Shape::SplineBorder { .. })), 12);
        assert_eq!(count(|s| matches!(s, Shape::FilledHex { .. })), 10);
        assert_eq!(count(|s| matches!(s, Shape::StaticTexture { .. })), 3);
        assert_eq!(count(|s| matches!(s, Shape::HexModel { .. })), 1);
    }

    #[test]
    fn the_numbers_the_calls_of_lua_rely_on_are_in() {
        let lib = HighlightLibrary::from_styles(&from_table());
        let c = |n: &str| lib.get(n).unwrap().color;
        // `MovementRangeBorder`: 100,185,245,200; `FireRangeBorder`: 255,0,0,200.
        assert_eq!(
            c("MovementRangeBorder"),
            Some([100.0 / 255.0, 185.0 / 255.0, 245.0 / 255.0, 200.0 / 255.0])
        );
        assert_eq!(c("FireRangeBorder"), Some([1.0, 0.0, 0.0, 200.0 / 255.0]));
        assert!(matches!(
            lib.get("MovementRangeBorder").unwrap().shape,
            Shape::SplineBorder { width, .. } if width == 6.0
        ));
        assert!(matches!(
            lib.get("AMRBorder").unwrap().shape,
            Shape::StaticTexture { .. }
        ));
        assert_eq!(c("AMRBorder"), None, "the calls give its colour");
        assert!(matches!(
            lib.get("AirRangeFill").unwrap().shape,
            Shape::FilledHex { width } if width == 1.0
        ));
    }

    #[test]
    fn a_colour_of_five_numbers_keeps_four_and_warns() {
        let lib = HighlightLibrary::from_styles(&from_table());
        let enemy = lib.get("EnemyOutline").unwrap();
        assert_eq!(
            enemy.color,
            Some([1.0, 0.0, 0.0, 0.0]),
            "255,0,0,0 of 255,0,0,0,200"
        );
        assert_eq!(lib.warnings().len(), 1);
        assert!(lib.warnings()[0].contains("EnemyOutline"));
    }

    #[test]
    fn three_numbers_are_opaque_and_two_are_left_out() {
        let mut w = Vec::new();
        assert_eq!(color("a", &[255, 0, 0], &mut w), Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(color("a", &[1, 2], &mut w), None);
        assert_eq!(color("a", &[], &mut w), None);
        assert_eq!(w.len(), 1, "an absent colour is no warning");
    }

    #[test]
    fn a_name_that_comes_twice_keeps_the_last() {
        let style = |w: f32| HighlightStyle {
            name: "X".into(),
            kind: HighlightKind::FilledHex,
            width: Some(w),
            texture: None,
            model: None,
            color: vec![],
            extra: Default::default(),
        };
        let lib = HighlightLibrary::from_styles(&[style(0.5), style(1.0)]);
        assert_eq!(lib.len(), 1);
        assert!(matches!(
            lib.get("X").unwrap().shape,
            Shape::FilledHex { width } if width == 1.0
        ));
    }

    #[test]
    fn the_textures_are_the_ones_the_styles_name() {
        let lib = HighlightLibrary::from_styles(&from_table());
        let ids: Vec<String> = lib
            .texture_ids()
            .iter()
            .map(|i| i.as_str().to_string())
            .collect();
        for want in [
            "tx:hex_contour1.dds",
            "tx:hex_contour2.dds",
            "tx:spline_border_contour2.dds",
            "tx:static_texture_highlight.dds",
            "tx:solid_contour.dds",
        ] {
            assert!(ids.iter().any(|i| i == want), "{want} in {ids:?}");
        }
    }
}
