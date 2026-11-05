//! World soundscapes (item 82): the ambient loops of the map under the camera. The mixer and its rules are
//! `audio::SoundscapeMixer`; this feeds it the map the renderer draws and the camera, once a whole second and when the
//! camera moves to another hex (the exe's `UpdateSoundscapes` / `UpdateCamera`).
//!
//! SIMPLIFIED(SIMP-exe-1027): the terrain of a plot is what the renderer's snapshot knows: its terrain kind (hills and
//! mountains are `TERRAIN_HILL` / `TERRAIN_MOUNTAIN`, as in the game), feature and river edges. The snapshot has no
//! improvement, and no shipped row names a soundscape for one, so improvements add no sound; the table still binds them.

use audio::{HexSound, ScapeCommand, ScapeView, SoundscapeMixer};
use glam::Vec2;
use render::hexgrid::nearest_cell;
use render::terrain::{Relief, TerrainCell, TerrainKind, TerrainSnapshot};

/// The mix is looked at again after this long, camera or not.
pub const MIX_EVERY_MS: u64 = 1000;

/// What the mix listens to this frame.
#[derive(Clone, Copy)]
pub struct WorldInput<'a> {
    pub snapshot: &'a TerrainSnapshot,
    /// Ground point (X, Z) in the middle of the screen.
    pub focus: Vec2,
    /// Camera height above it.
    pub height: f32,
    /// The figures of the units near the camera and the clips they play (their sounds, re-b15 ITEM 4).
    pub figures: &'a [render::world::unit::FigureClip],
}

/// The sound a plot asks for: its row names, whether it was seen, whether a river runs along it.
pub fn hex_sound(c: &TerrainCell) -> HexSound {
    if !c.is_revealed() {
        return HexSound {
            unexplored: true,
            ..HexSound::default()
        };
    }
    let terrain = match c.relief {
        // (the game gives a hill plot the terrain TERRAIN_HILL, a mountain TERRAIN_MOUNTAIN, whatever lies under)
        Relief::Hill => Some("TERRAIN_HILL"),
        Relief::Mountain => Some("TERRAIN_MOUNTAIN"),
        _ => match c.kind {
            TerrainKind::Grass => Some("TERRAIN_GRASS"),
            TerrainKind::Plains => Some("TERRAIN_PLAINS"),
            TerrainKind::Desert => Some("TERRAIN_DESERT"),
            TerrainKind::Tundra => Some("TERRAIN_TUNDRA"),
            TerrainKind::Snow => Some("TERRAIN_SNOW"),
            TerrainKind::Coast => Some("TERRAIN_COAST"),
            TerrainKind::Ocean => Some("TERRAIN_OCEAN"),
            TerrainKind::Unknown => None,
        },
    };
    HexSound {
        unexplored: false,
        river: c.rivers & 0x3f != 0,
        terrain,
        feature: c.feat.type_name(),
        improvement: None,
    }
}

/// What a look depended on: the camera's hex and the half sizes of the rectangle it sees.
type LookKey = ((i32, i32), (i32, i32));

/// The mixer and when it last looked.
pub struct WorldSound {
    mixer: SoundscapeMixer,
    /// The camera's hex, the size of the rectangle it sees and the time of the last look.
    last: Option<(LookKey, u64)>,
    heard: Vec<audio::Heard>,
}

impl WorldSound {
    pub fn new(mixer: SoundscapeMixer) -> Self {
        Self {
            mixer,
            last: None,
            heard: Vec::new(),
        }
    }

    pub fn soundscapes(&self) -> usize {
        self.mixer.len()
    }

    /// One frame: what the output must do. The map is looked at again once a second and when the camera moves to
    /// another hex or sees another rectangle (zoom); between two looks only the random sounds are due (the exe counts
    /// their timers every frame). `sounding` says whether a random sound played earlier still sounds (its next wait
    /// starts when it ends).
    pub fn step(
        &mut self,
        now_ms: u64,
        input: &WorldInput<'_>,
        sounding: &mut dyn FnMut(&str) -> bool,
    ) -> Vec<ScapeCommand> {
        let cell = nearest_cell(input.focus);
        let view = ScapeView::at(cell.0, cell.1, input.height);
        let key = (cell, (view.half_w, view.half_h));
        let due = match self.last {
            Some((k, at)) => k != key || now_ms.saturating_sub(at) >= MIX_EVERY_MS,
            None => true,
        };
        if !due {
            return self.mixer.elements(now_ms, sounding);
        }
        self.last = Some((key, now_ms));
        let shape = input.snapshot.shape();
        self.heard = self.mixer.mix(&view, |x, y| {
            let (x, y) = shape.normalize(x, y)?;
            input.snapshot.cell(x, y).map(hex_sound)
        });
        self.mixer.update(now_ms, &self.heard, sounding)
    }

    /// The game is left (or the loading screen is up): every bed ends and the next frame looks afresh.
    pub fn stop(&mut self) -> Vec<ScapeCommand> {
        self.last = None;
        self.heard.clear();
        self.mixer.stop_all()
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{ScapeBinding, ScapeBindingKind, ScapeElement, Soundscape, SoundscapeTable};
    use render::hexgrid::cell_center;
    use render::terrain::{FeatureKind, Fog};

    use super::*;

    fn cell(kind: TerrainKind) -> TerrainCell {
        TerrainCell {
            relief: Relief::Flat,
            kind,
            fog: Fog::Visible,
            ..TerrainCell::default()
        }
    }

    #[test]
    fn a_plot_gives_its_row_names_its_river_and_its_fog() {
        let g = hex_sound(&cell(TerrainKind::Grass));
        assert_eq!(g.terrain, Some("TERRAIN_GRASS"));
        assert!(!g.river && !g.unexplored && g.feature.is_none());
        let forest = TerrainCell {
            feat: FeatureKind::Forest,
            rivers: 0b000100,
            ..cell(TerrainKind::Plains)
        };
        let f = hex_sound(&forest);
        assert_eq!(
            (f.terrain, f.feature, f.river),
            (Some("TERRAIN_PLAINS"), Some("FEATURE_FOREST"), true)
        );
        // Hills and mountains are terrains of their own in the game.
        let hill = TerrainCell {
            relief: Relief::Hill,
            ..cell(TerrainKind::Desert)
        };
        assert_eq!(hex_sound(&hill).terrain, Some("TERRAIN_HILL"));
        let mountain = TerrainCell {
            relief: Relief::Mountain,
            ..cell(TerrainKind::Unknown)
        };
        assert_eq!(hex_sound(&mountain).terrain, Some("TERRAIN_MOUNTAIN"));
        assert_eq!(hex_sound(&cell(TerrainKind::Unknown)).terrain, None);
        // Never seen: the fog and nothing else, whatever the cell holds.
        let black = TerrainCell {
            fog: Fog::Unrevealed,
            rivers: 1,
            ..cell(TerrainKind::Ocean)
        };
        let b = hex_sound(&black);
        assert!(b.unexplored && b.terrain.is_none() && !b.river);
        // Seen once but out of sight is a revealed hex.
        let hidden = TerrainCell {
            fog: Fog::Hidden,
            ..cell(TerrainKind::Coast)
        };
        assert_eq!(hex_sound(&hidden).terrain, Some("TERRAIN_COAST"));
    }

    fn table() -> SoundscapeTable {
        let scape = |id: &str, bed: &str| Soundscape {
            id: id.into(),
            min_distance: 3.0,
            max_distance: 5.0,
            min_volume: 30,
            max_volume: 30,
            priority: 1.0,
            elements: vec![ScapeElement {
                scripts: vec![bed.into()],
                bed: true,
                min_gap_ms: 0,
                max_gap_ms: 0,
            }],
        };
        SoundscapeTable {
            scapes: vec![
                scape("ASSS_AMBIENCE_DEEP_OCEAN", "BED_OCEAN"),
                scape("ASSS_AMBIENCE_GRASSLANDS", "BED_GRASS"),
            ],
            bindings: vec![
                ScapeBinding {
                    kind: ScapeBindingKind::Terrain,
                    type_name: "TERRAIN_GRASS".into(),
                    scape: "ASSS_AMBIENCE_GRASSLANDS".into(),
                },
                ScapeBinding {
                    kind: ScapeBindingKind::Terrain,
                    type_name: "TERRAIN_OCEAN".into(),
                    scape: "ASSS_AMBIENCE_DEEP_OCEAN".into(),
                },
            ],
        }
    }

    /// 16 x 8 map, grass in the west half and ocean in the east half.
    fn snapshot(wrap: bool) -> TerrainSnapshot {
        let cells = (0..8)
            .flat_map(|_| 0..16)
            .map(|x| {
                cell(if x < 8 {
                    TerrainKind::Grass
                } else {
                    TerrainKind::Ocean
                })
            })
            .collect();
        TerrainSnapshot::new(16, 8, wrap, cells).unwrap()
    }

    fn beds(cmds: &[ScapeCommand]) -> Vec<(String, f32)> {
        cmds.iter()
            .filter_map(|c| match c {
                ScapeCommand::Bed { script, level, .. } => Some((script.clone(), *level)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn it_looks_once_a_second_and_when_the_camera_changes_hex() {
        let snap = snapshot(false);
        let mut w = WorldSound::new(SoundscapeMixer::new(&table(), 1));
        assert_eq!(w.soundscapes(), 2);
        let at = |x: i32, y: i32| WorldInput {
            snapshot: &snap,
            focus: cell_center(x, y),
            height: 424.0,
            figures: &[],
        };
        // In the middle of the grass: the grass bed at the soundscape's 0.3, heard from the listener's lift (0.55 of
        // the height, 3.64 hexes) between the soundscape's 3 and 5.
        let first = w.step(0, &at(3, 4), &mut |_| false);
        assert_eq!(beds(&first).len(), 1);
        let (script, level) = beds(&first).remove(0);
        assert_eq!(script, "BED_GRASS");
        let lift = 0.55 * 424.0 / 64.0;
        let heard = 0.3 * (1.0 - (lift - 3.0) / 2.0);
        assert!((level - heard).abs() < 1e-5, "{level} {heard}");
        // The same hex a moment later: nothing to do; a second later: told again.
        assert!(w.step(400, &at(3, 4), &mut |_| false).is_empty());
        assert_eq!(beds(&w.step(1000, &at(3, 4), &mut |_| false)).len(), 1);
        // Another hex looks at once.
        assert!(!w.step(1100, &at(4, 4), &mut |_| false).is_empty());
        // Far enough into the ocean the grass ends and the ocean bed starts.
        let cmds = w.step(1200, &at(14, 4), &mut |_| false);
        assert!(
            cmds.contains(&ScapeCommand::EndBed {
                script: "BED_GRASS".into()
            }),
            "{cmds:?}"
        );
        assert_eq!(beds(&cmds)[0].0, "BED_OCEAN");
    }

    #[test]
    fn a_zoom_to_another_rectangle_looks_at_once_and_random_sounds_are_due_between_looks() {
        let snap = snapshot(false);
        let mut t = table();
        t.scapes[1].elements.push(ScapeElement {
            scripts: vec!["BIRD".into()],
            bed: false,
            min_gap_ms: 500,
            max_gap_ms: 500,
        });
        let mut w = WorldSound::new(SoundscapeMixer::new(&t, 1));
        let at = |height: f32| WorldInput {
            snapshot: &snap,
            focus: cell_center(3, 4),
            height,
            figures: &[],
        };
        w.step(0, &at(424.0), &mut |_| false);
        // The same hex and rectangle, well before the next look: no bed told again, but the bird is due at 500 ms.
        assert!(w.step(300, &at(424.0), &mut |_| false).is_empty());
        let between = w.step(600, &at(424.0), &mut |_| false);
        assert!(
            matches!(between.as_slice(), [ScapeCommand::Element { script, .. }] if script == "BIRD"),
            "{between:?}"
        );
        // Zooming out to a bigger rectangle looks again without waiting for the second.
        let zoomed = w.step(700, &at(900.0), &mut |_| false);
        assert_eq!(beds(&zoomed).len(), 1, "{zoomed:?}");
    }

    #[test]
    fn the_map_wraps_in_x_for_the_hexes_around_the_camera() {
        let snap = snapshot(true);
        let mut w = WorldSound::new(SoundscapeMixer::new(&table(), 1));
        // At the west edge the hexes past it are the east end of the map (ocean): both are heard.
        let cmds = w.step(
            0,
            &WorldInput {
                snapshot: &snap,
                focus: cell_center(0, 4),
                height: 424.0,
                figures: &[],
            },
            &mut |_| false,
        );
        let b = beds(&cmds);
        assert!(b.iter().any(|(s, _)| s == "BED_GRASS"), "{b:?}");
        assert!(b.iter().any(|(s, _)| s == "BED_OCEAN"), "{b:?}");
    }

    #[test]
    fn stop_ends_the_beds_and_the_next_frame_looks_again() {
        let snap = snapshot(false);
        let mut w = WorldSound::new(SoundscapeMixer::new(&table(), 1));
        let input = WorldInput {
            snapshot: &snap,
            focus: cell_center(3, 4),
            height: 424.0,
            figures: &[],
        };
        w.step(0, &input, &mut |_| false);
        let stopped = w.stop();
        assert!(stopped.contains(&ScapeCommand::EndBed {
            script: "BED_GRASS".into()
        }));
        assert!(w.stop().is_empty());
        assert_eq!(
            beds(&w.step(10, &input, &mut |_| false)).len(),
            1,
            "looks at once"
        );
    }
}
