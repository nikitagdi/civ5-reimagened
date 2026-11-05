//! The highlights (world-overlays WOV5, `HighlightSystem`, RW-20): the style library
//! (`library.rs`), the five kinds of shape (`contour.rs`, `spline.rs`, `filled.rs`, `texture.rs`,
//! `model.rs`) and the draw of them into the map of the overlay (`gpu.rs`).
//!
//! The input is a [`HighlightSet`]: `(style, cell, colour)`. The cells of one style and colour make
//! one draw; the shape of the style makes the triangles of it, in our ground plane (`X`, `Z`) like the
//! borders, through `VSOverlay` and the `Local` matrix of the copy.
//!
//! Colour: a contour takes the call's colour (the style's if the call gives none); a `SplineBorder`
//! and a `FilledHex` take the style's (the call only switches them on); a `StaticTexture` is never
//! tinted.
// re: HighlightSystem::OnHexHighlight 0x08a52a30 (only HexContour stores the packed call colour),
// RenderFilledHexStyle 0x08a519f4 (DrawColor = the style's m_afColor), RenderStaticTextureStyle
// 0x08a51616 (DrawColor never written) (RE-B18 item 3 HL-1)

pub mod contour;
pub mod filled;
pub mod gpu;
#[cfg(test)]
mod lekmod_table;
pub mod library;
pub mod model;
pub mod spline;
pub mod texture;

use asset_format::LogicalId;

use self::library::{HighlightLibrary, Shape, Style};
use super::grid::GridVertex;
use crate::hexgrid::GridShape;

/// One highlighted cell: the style by name, the cell (map coordinates), the colour of the call.
#[derive(Debug, Clone, PartialEq)]
pub struct HighlightCell {
    pub style: String,
    pub x: i32,
    pub y: i32,
    /// RGBA, 0 to 1.
    pub color: Option<[f32; 4]>,
}

/// What to highlight. The same set again builds nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HighlightSet {
    pub cells: Vec<HighlightCell>,
}

impl HighlightSet {
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// Which pixel shader a draw uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawKind {
    /// `PSContour`: the profile texture across a strip.
    Contour,
    /// `PSOverlay`: a strip of one colour.
    Plain,
    /// `PSNoTexture`: a flat fill.
    Filled,
    /// `PSSimpleTexture` times the colour: a texture over a box.
    Static,
}

/// One draw: a range of the indices, a shader, a texture, a colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    pub kind: DrawKind,
    pub texture: Option<LogicalId>,
    /// `DrawColor`.
    pub color: [f32; 4],
    pub first_index: u32,
    pub index_count: u32,
}

/// A model on a cell (`HexModel`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPlacement {
    pub model: String,
    pub x: i32,
    pub y: i32,
}

/// The triangles of a set and what they are drawn with.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HighlightMesh {
    pub vertices: Vec<GridVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<Draw>,
    pub models: Vec<ModelPlacement>,
}

impl HighlightMesh {
    /// Adds a draw of what `build` added since `first_index`.
    fn finish(
        &mut self,
        kind: DrawKind,
        texture: Option<LogicalId>,
        color: [f32; 4],
        first_index: usize,
    ) {
        let count = self.indices.len() - first_index;
        if count > 0 {
            self.draws.push(Draw {
                kind,
                texture,
                color,
                first_index: first_index as u32,
                index_count: count as u32,
            });
        }
    }
}

/// The colour of a draw (see the module's note).
pub fn resolve_color(style: &Style, call: Option<[f32; 4]>) -> [f32; 4] {
    let white = [1.0; 4];
    match style.shape {
        Shape::SplineBorder { .. } | Shape::FilledHex { .. } => {
            style.color.or(call).unwrap_or(white)
        }
        Shape::StaticTexture { .. } => white,
        _ => call.or(style.color).unwrap_or(white),
    }
}

/// The mesh of a set. A cell whose style the library lacks is left out; so is one off the map.
pub fn build(set: &HighlightSet, library: &HighlightLibrary, shape: &GridShape) -> HighlightMesh {
    let mut mesh = HighlightMesh::default();
    // Groups in the order of first appearance: (style, colour) -> cells.
    type Group<'a> = (&'a Style, [f32; 4], Vec<(i32, i32)>);
    let mut groups: Vec<Group> = Vec::new();
    for c in &set.cells {
        let Some(style) = library.get(&c.style) else {
            continue;
        };
        let Some(cell) = shape.normalize(c.x, c.y) else {
            continue;
        };
        let color = resolve_color(style, c.color);
        match groups
            .iter_mut()
            .find(|(s, col, _)| std::ptr::eq(*s, style) && *col == color)
        {
            Some((_, _, cells)) => {
                if !cells.contains(&cell) {
                    cells.push(cell);
                }
            }
            None => groups.push((style, color, vec![cell])),
        }
    }
    for (style, color, cells) in groups {
        let first = mesh.indices.len();
        match &style.shape {
            Shape::HexContour { width, texture } => {
                contour::append(&mut mesh, &cells, *width);
                let kind = if texture.is_some() {
                    DrawKind::Contour
                } else {
                    DrawKind::Plain
                };
                mesh.finish(kind, texture.clone(), color, first);
            }
            Shape::SplineBorder { width, texture } => {
                spline::append(&mut mesh, &cells, shape, *width);
                let kind = if texture.is_some() {
                    DrawKind::Contour
                } else {
                    DrawKind::Plain
                };
                mesh.finish(kind, texture.clone(), color, first);
            }
            Shape::FilledHex { width } => {
                filled::append(&mut mesh, &cells, *width);
                mesh.finish(DrawKind::Filled, None, color, first);
            }
            Shape::StaticTexture { texture } => {
                texture::append(&mut mesh, &cells);
                mesh.finish(DrawKind::Static, texture.clone(), color, first);
            }
            Shape::HexModel { model } => {
                model::place(&mut mesh, &cells, model);
            }
        }
    }
    mesh
}

/// A vertex in our ground plane: `x` and `y` are `X` and `Z`.
pub(crate) fn vertex(p: glam::Vec2, u: f32, v: f32) -> GridVertex {
    GridVertex {
        position: [p.x, p.y, 0.0, 1.0],
        color: [1.0; 4],
        uv: [u, v],
    }
}

#[cfg(test)]
mod tests {
    use super::library::HighlightLibrary;
    use super::*;
    use asset_format::{HighlightKind, HighlightStyle};

    fn style(
        name: &str,
        kind: HighlightKind,
        width: Option<f32>,
        tex: Option<&str>,
        color: &[u8],
    ) -> HighlightStyle {
        HighlightStyle {
            name: name.into(),
            kind,
            width,
            texture: tex.map(LogicalId::new),
            model: None,
            color: color.to_vec(),
            extra: Default::default(),
        }
    }

    pub(crate) fn lib() -> HighlightLibrary {
        HighlightLibrary::from_styles(&[
            style(
                "Contour",
                HighlightKind::HexContour,
                Some(0.2),
                Some("tx:c.dds"),
                &[],
            ),
            style(
                "Spline",
                HighlightKind::SplineBorder,
                Some(6.0),
                Some("tx:s.dds"),
                &[10, 20, 30, 200],
            ),
            style(
                "Fill",
                HighlightKind::FilledHex,
                Some(1.0),
                None,
                &[255, 0, 0, 30],
            ),
            style(
                "Static",
                HighlightKind::StaticTexture,
                None,
                Some("tx:t.dds"),
                &[],
            ),
            style(
                "Plain",
                HighlightKind::SplineBorder,
                Some(7.0),
                None,
                &[0, 0, 255, 164],
            ),
        ])
    }

    pub(crate) fn flat(w: i32, h: i32) -> GridShape {
        GridShape {
            width: w,
            height: h,
            wrap_x: false,
        }
    }

    fn set(style: &str, cells: &[(i32, i32)], color: Option<[f32; 4]>) -> HighlightSet {
        HighlightSet {
            cells: cells
                .iter()
                .map(|&(x, y)| HighlightCell {
                    style: style.into(),
                    x,
                    y,
                    color,
                })
                .collect(),
        }
    }

    #[test]
    fn each_shape_makes_the_draw_of_its_kind() {
        let l = lib();
        let s = flat(8, 8);
        let cells = [(3, 3), (4, 3)];
        for (name, kind) in [
            ("Contour", DrawKind::Contour),
            ("Spline", DrawKind::Contour),
            ("Fill", DrawKind::Filled),
            ("Static", DrawKind::Static),
            ("Plain", DrawKind::Plain),
        ] {
            let m = build(&set(name, &cells, None), &l, &s);
            assert_eq!(m.draws.len(), 1, "{name}");
            assert_eq!(m.draws[0].kind, kind, "{name}");
            assert!(m.indices.len().is_multiple_of(3));
            assert!(
                m.indices.iter().all(|&i| (i as usize) < m.vertices.len()),
                "{name}"
            );
            assert_eq!(m.draws[0].index_count as usize, m.indices.len());
        }
    }

    #[test]
    fn the_call_colour_is_used_by_a_contour_only() {
        let l = lib();
        let s = flat(8, 8);
        let call = Some([0.0, 1.0, 0.0, 0.25]);
        let contour = build(&set("Contour", &[(2, 2)], call), &l, &s);
        assert_eq!(contour.draws[0].color, [0.0, 1.0, 0.0, 0.25]);
        // A fill takes its style's colour, call or not.
        for c in [call, None] {
            let fill = build(&set("Fill", &[(2, 2)], c), &l, &s);
            assert_eq!(
                fill.draws[0].color,
                [1.0, 0.0, 0.0, 30.0 / 255.0],
                "the style's"
            );
        }
        // A texture is never tinted.
        let tex = build(&set("Static", &[(2, 2)], call), &l, &s);
        assert_eq!(tex.draws[0].color, [1.0; 4]);
        let spline = build(&set("Spline", &[(2, 2)], call), &l, &s);
        assert_eq!(
            spline.draws[0].color[0],
            10.0 / 255.0,
            "the style's, not the call's"
        );
    }

    #[test]
    fn cells_of_one_style_and_colour_are_one_draw_and_a_new_colour_is_another() {
        let l = lib();
        let s = flat(8, 8);
        let mut set = set("Contour", &[(1, 1), (2, 1), (2, 1)], None);
        set.cells.push(HighlightCell {
            style: "Contour".into(),
            x: 5,
            y: 5,
            color: Some([0.0, 0.0, 1.0, 1.0]),
        });
        let m = build(&set, &l, &s);
        assert_eq!(m.draws.len(), 2);
        // A cell twice is drawn once: two rings of twelve triangles, then the other colour's one.
        assert_eq!(m.draws[0].index_count, 2 * 36);
        assert_eq!(m.draws[1].index_count, 36);
        assert_eq!(m.draws[1].first_index, 72);
    }

    #[test]
    fn an_unknown_style_and_a_cell_off_the_map_are_left_out() {
        let l = lib();
        let s = flat(4, 4);
        assert!(
            build(&set("Nope", &[(1, 1)], None), &l, &s)
                .draws
                .is_empty()
        );
        assert!(
            build(&set("Fill", &[(9, 9)], None), &l, &s)
                .draws
                .is_empty()
        );
        assert!(build(&HighlightSet::default(), &l, &s).vertices.is_empty());
    }
}
