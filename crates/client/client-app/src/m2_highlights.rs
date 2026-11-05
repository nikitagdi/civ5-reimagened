//! Hex highlights of the original UI (M2): Events.SerialEventHexHighlight and friends to the renderer.
//!
//! The Lua UI lights hexes with `Events.SerialEventHexHighlight(hex, bShow, color, style)`, drops one style with
//! `Events.ClearHexHighlightStyle(style)` and all with `Events.ClearHexHighlights()`; the original's engine
//! draws them. `HexHighlights` follows those events (fed by the `exe_api` event tap, [`TAPPED_EVENTS`]) and
//! gives the renderer the full current [`HighlightSet`] when it changed (`Renderer::set_highlight_set`).
//!
//! The style the Lua passes is its name in `Highlights.xml` (`"MovementRangeBorder"`, `"AMRBorder"`,
//! `"FireRangeBorder"`...; see `play/highlight_calls.rs`); the renderer's library resolves the name and draws
//! the shape (border, fill, contour). The colour is a Vector4 (RGBA, 0 to 1); the hex is a hex-space Vector2.

use exe_api::Arg;
use render::world::overlay::highlights::{HighlightCell, HighlightSet};

/// The `Events.*` this module wants from `ExeApi::set_event_tap`.
pub const TAPPED_EVENTS: [&str; 3] = [
    "SerialEventHexHighlight",
    "ClearHexHighlightStyle",
    "ClearHexHighlights",
];

/// The styles of Lekmod's `Highlights.xml` in file order: a numeric style (the engine's `HighlightStyle` id) is
/// the index here.
// SIMPLIFIED(SIMP-exe-55): the Lua of the game passes style names; a number is read as the index in the
// 34-style file order of Lekmod's Highlights.xml (our assumption for the engine's enum, no Lua file uses it).
const STYLE_ORDER: [&str; 34] = [
    "",
    "SampleFilledHex",
    "SampleStaticTexture",
    "SampleHexModel",
    "EditorHexStyle1",
    "EditorHexStyle2",
    "EditorHexStyle3",
    "TempBorder",
    "GroupBorder",
    "ValidFireTargetBorder",
    "FireRangeBorder",
    "MovementRangeBorder",
    "AMRBorder",
    "TradeRoute",
    "GUHB",
    "AirRangeFill",
    "AirRangeBorder",
    "AirSweepFill",
    "AirSweepBorder",
    "Culture",
    "CityOverlap",
    "OverlapFill",
    "OverlapOutline",
    "WorkedFill",
    "WorkedOutline",
    "OwnedFill",
    "OwnedOutline",
    "BuyFill",
    "BuyOutline",
    "VacantFill",
    "VacantOutline",
    "EnemyFill",
    "EnemyOutline",
    "CityLimits",
];

/// The style an argument names: a string as is, a number by [`STYLE_ORDER`]; `None` for anything else.
fn style_of(a: Option<&Arg>) -> Option<String> {
    match a? {
        Arg::Str(s) => Some(s.clone()),
        Arg::Num(n) if *n >= 0.0 && n.fract() == 0.0 => {
            STYLE_ORDER.get(*n as usize).map(|s| (*s).to_owned())
        }
        _ => None,
    }
}

/// The map cell of a hex-space `{x, y}` vector.
fn cell_of(a: Option<&Arg>) -> Option<(i32, i32)> {
    match a? {
        Arg::Vec(v) if v.len() >= 2 => Some(exe_api::hex::hex_to_grid(v[0] as i32, v[1] as i32)),
        _ => None,
    }
}

/// RGBA of a Vector4 (a Vector3 has alpha 1); `None` when the call gives no colour (the style's own is used).
fn color_of(a: Option<&Arg>) -> Option<[f32; 4]> {
    match a? {
        Arg::Vec(v) if v.len() >= 3 => Some([
            v[0] as f32,
            v[1] as f32,
            v[2] as f32,
            v.get(3).map_or(1.0, |w| *w as f32),
        ]),
        _ => None,
    }
}

/// The highlights the Lua UI has switched on.
#[derive(Debug, Default)]
pub struct HexHighlights {
    cells: Vec<HighlightCell>,
    changed: bool,
}

impl HexHighlights {
    pub fn new() -> Self {
        Self::default()
    }

    /// One tapped event (see [`TAPPED_EVENTS`]); other names and malformed arguments are ignored.
    pub fn on_event(&mut self, name: &str, args: &[Arg]) {
        match name {
            "SerialEventHexHighlight" => {
                let (Some((x, y)), Some(style)) = (cell_of(args.first()), style_of(args.get(3)))
                else {
                    // SIMPLIFIED(SIMP-exe-56): a call with a hex or a style we cannot read is dropped (the
                    // original's engine would index a style table with it).
                    tracing::debug!(target: "client.m2.highlights", "SerialEventHexHighlight: unreadable arguments {args:?}");
                    return;
                };
                let show = matches!(args.get(1), Some(Arg::Bool(true)));
                if show {
                    let color = color_of(args.get(2));
                    match self
                        .cells
                        .iter_mut()
                        .find(|c| c.x == x && c.y == y && c.style == style)
                    {
                        Some(c) => {
                            if c.color != color {
                                c.color = color;
                                self.changed = true;
                            }
                        }
                        None => {
                            self.cells.push(HighlightCell { style, x, y, color });
                            self.changed = true;
                        }
                    }
                } else {
                    let n = self.cells.len();
                    self.cells
                        .retain(|c| !(c.x == x && c.y == y && c.style == style));
                    self.changed |= self.cells.len() != n;
                }
            }
            "ClearHexHighlightStyle" => {
                let Some(style) = style_of(args.first()) else {
                    return;
                };
                let n = self.cells.len();
                self.cells.retain(|c| c.style != style);
                self.changed |= self.cells.len() != n;
            }
            "ClearHexHighlights" => self.clear(),
            _ => {}
        }
    }

    /// Drops every highlight.
    pub fn clear(&mut self) {
        if !self.cells.is_empty() {
            self.cells.clear();
            self.changed = true;
        }
    }

    /// The full current set if something changed since the last call.
    pub fn take_changed(&mut self) -> Option<HighlightSet> {
        if !std::mem::take(&mut self.changed) {
            return None;
        }
        Some(HighlightSet {
            cells: self.cells.clone(),
        })
    }

    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show(h: &mut HexHighlights, hx: f64, hy: f64, on: bool, style: &str) {
        h.on_event(
            "SerialEventHexHighlight",
            &[
                Arg::Vec(vec![hx, hy]),
                Arg::Bool(on),
                Arg::Vec(vec![0.0, 1.0, 0.0, 0.25]),
                Arg::Str(style.into()),
            ],
        );
    }

    #[test]
    fn show_converts_hex_to_grid_and_keeps_the_colour() {
        let mut h = HexHighlights::new();
        show(&mut h, 4.0, 3.0, true, "MovementRangeBorder");
        let set = h.take_changed().unwrap();
        assert_eq!(set.cells.len(), 1);
        let c = &set.cells[0];
        // hex (4, 3) is map (5, 3)
        assert_eq!((c.x, c.y), (5, 3));
        assert_eq!(c.style, "MovementRangeBorder");
        assert_eq!(c.color, Some([0.0, 1.0, 0.0, 0.25]));
    }

    #[test]
    fn hide_removes_only_that_cell_and_style() {
        let mut h = HexHighlights::new();
        show(&mut h, 1.0, 0.0, true, "AMRBorder");
        show(&mut h, 1.0, 0.0, true, "MovementRangeBorder");
        show(&mut h, 2.0, 0.0, true, "AMRBorder");
        h.take_changed();
        show(&mut h, 1.0, 0.0, false, "AMRBorder");
        assert_eq!(h.take_changed().unwrap().cells.len(), 2);
    }

    #[test]
    fn clear_style_and_clear_all() {
        let mut h = HexHighlights::new();
        show(&mut h, 1.0, 0.0, true, "AMRBorder");
        show(&mut h, 2.0, 0.0, true, "MovementRangeBorder");
        show(&mut h, 3.0, 0.0, true, "MovementRangeBorder");
        h.take_changed();
        h.on_event(
            "ClearHexHighlightStyle",
            &[Arg::Str("MovementRangeBorder".into())],
        );
        let set = h.take_changed().unwrap();
        assert_eq!(set.cells.len(), 1);
        assert_eq!(set.cells[0].style, "AMRBorder");
        h.on_event("ClearHexHighlights", &[]);
        assert!(h.take_changed().unwrap().is_empty());
    }

    #[test]
    fn change_detection() {
        let mut h = HexHighlights::new();
        assert!(h.take_changed().is_none());
        show(&mut h, 1.0, 1.0, true, "AMRBorder");
        assert!(h.take_changed().is_some());
        assert!(h.take_changed().is_none());
        // the same call again, a clear of nothing, a hide of nothing: no change
        show(&mut h, 1.0, 1.0, true, "AMRBorder");
        h.on_event("ClearHexHighlightStyle", &[Arg::Str("GUHB".into())]);
        show(&mut h, 9.0, 9.0, false, "AMRBorder");
        assert!(h.take_changed().is_none());
        // a new colour is a change
        h.on_event(
            "SerialEventHexHighlight",
            &[
                Arg::Vec(vec![1.0, 1.0]),
                Arg::Bool(true),
                Arg::Vec(vec![1.0, 0.0, 0.0, 1.0]),
                Arg::Str("AMRBorder".into()),
            ],
        );
        assert!(h.take_changed().is_some());
        h.clear();
        assert!(h.take_changed().is_some());
        h.clear();
        assert!(h.take_changed().is_none());
    }

    #[test]
    fn numeric_styles_follow_the_file_order_and_junk_is_ignored() {
        let mut h = HexHighlights::new();
        h.on_event(
            "SerialEventHexHighlight",
            &[
                Arg::Vec(vec![0.0, 0.0]),
                Arg::Bool(true),
                Arg::Vec(vec![1.0, 1.0, 1.0]),
                Arg::Num(12.0),
            ],
        );
        let set = h.take_changed().unwrap();
        assert_eq!(set.cells[0].style, "AMRBorder");
        assert_eq!(set.cells[0].color, Some([1.0, 1.0, 1.0, 1.0]));
        h.on_event("SerialEventHexHighlight", &[Arg::Nil, Arg::Bool(true)]);
        h.on_event("SomethingElse", &[]);
        assert!(h.take_changed().is_none());
    }
}
