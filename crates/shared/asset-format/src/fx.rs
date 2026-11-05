//! `C5FX`: one Fork particle effect, typed from a `.psb`, and
//! `C5FI`: the index of the visual effects that name them (`fx:index`).
//!
//! The fields are the emitter property of the Fork SDK (0x1FC bytes, `civ5-re` answers-b updates
//! 6 and 10); each field's comment gives its offset. Version 1 was the raw `.psb` of stage 0, which
//! no build of this repository wrote; it is not read.
//!
//! Space: the numbers are Fork's own (y up, the editor's units). The client places an effect by
//! the visual effect's transform ([`FxVisEffect`]) times the map position.

use serde::{Deserialize, Serialize};

use crate::{FormatError, LogicalId, blob};

pub const FX_MAGIC: [u8; 4] = *b"C5FX";
pub const FX_INDEX_MAGIC: [u8; 4] = *b"C5FI";
/// Manifest id of the [`FxIndex`].
pub const FX_INDEX_ID: &str = "fx:index";

/// Flag bits of [`FxProps::flags`] (property 0x68 as one `u32`: 0x69 is bits 8..15, 0x6A bits
/// 16..31).
pub mod flag {
    /// Random start frame of the flipbook.
    pub const RANDOM_FRAME: u32 = 1 << 0;
    /// The texture assets are flipbook frames.
    pub const FLIPBOOK: u32 = 1 << 1;
    /// The random start applies at age 0.
    pub const RANDOM_START: u32 = 1 << 2;
    /// Particles follow a bezier path (0x69 b0).
    pub const BEZIER_PATH: u32 = 1 << 8;
    /// The emitter moves along its path; the emitter matrix accumulates (0x69 b1).
    pub const EMITTER_PATH: u32 = 1 << 9;
    /// Wobble of the bezier offset by phase (0x69 b2).
    pub const PATH_PHASE: u32 = 1 << 10;
    /// Bezier birth (0x69 b7).
    pub const BEZIER_BIRTH: u32 = 1 << 15;
    /// Only spawned as a sub-emitter (0x6A b0).
    pub const REFERENCED: u32 = 1 << 16;
    /// Particles follow the emitter (local space, 0x6A b1).
    pub const FOLLOW_EMITTER: u32 = 1 << 17;
    /// Spin is always negative (0x6A b5).
    pub const NEGATIVE_SPIN: u32 = 1 << 21;
    /// No random sign of the spin.
    pub const NO_SPIN_SIGN: u32 = 0x40_0000;
    /// Billboard faces the camera (`LookAt`).
    pub const LOOK_AT: u32 = 0x8_0000;
    /// Quad aligned with the velocity.
    pub const VELOCITY_ALIGNED: u32 = 0x400_0000;
    /// Age stops (the particle never dies by age).
    pub const FREEZE_AGE: u32 = 0x2000_0000;
}

/// Bits of [`FxProps::flags2`] (property 0x1F8).
pub mod flag2 {
    /// One size for the whole life (`size1 = size2 = size0`).
    pub const UNIFORM_SIZE: u32 = 1 << 0;
    /// No spin.
    pub const NO_SPIN: u32 = 1 << 1;
}

/// The emitter property (0x1FC bytes) less the block pointers and the fields no code reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxProps {
    /// 0x10, 0x20, 0x30, 0x40: colour keys RGBA.
    pub colors: [[f32; 4]; 4],
    /// 0x50, 0x54: the normalised ages of keys 1 and 2.
    pub times: [f32; 2],
    /// 0x58, 0x5C: life min and max, seconds.
    pub life: [f32; 2],
    /// 0x60, 0x64: speed min and max.
    pub speed: [f32; 2],
    /// 0x68: see [`flag`].
    pub flags: u32,
    /// 0x6C..0x74: sizes at birth, mid time and death (max of the range).
    pub size_max: [f32; 3],
    /// 0x78: normalised age of the middle size.
    pub mid_time: f32,
    /// 0x7C, 0x80: spin min and max, radians per second.
    pub spin: [f32; 2],
    /// 0x84: drag (`v += v * drag * dt`).
    pub drag: f32,
    /// 0x88..0x90: size scale x, y, z.
    pub size_scale: [f32; 3],
    /// 0x98..0xA0: rotation, degrees (x is the quad's own angle).
    pub rotation: [f32; 3],
    /// 0xA8..0xB4: tint RGBA (to the vertex `COLOR1`).
    pub tint: [f32; 4],
    /// 0xB8..0xC0: sizes, min of the range.
    pub size_min: [f32; 3],
    /// 0xC4: prewarm, seconds.
    pub prewarm: f32,
    /// 0xCC: particles per second.
    pub rate: f32,
    /// 0xD0: gravity scale.
    pub gravity: f32,
    /// 0xD4..0xE0: birth volume dimensions x, y, z and the spread of the "up" direction.
    pub volume: [f32; 4],
    /// 0xE4: most particles alive.
    pub max_live: f32,
    /// 0xE8: birth volume, 0..=11 (0 plane, 1 sphere, 2 cone, 3 cylinder, 4..7 their volumes, 8
    /// point, 9 custom, 10 arc, 11 arc volume).
    pub volume_kind: u8,
    /// 0xE9: 0 diamond, 1 rectangle, 2 string, 3 triangle.
    pub shape: u8,
    /// 0x104: particles per burst (0: no bursts).
    pub burst: i32,
    /// 0x108: flipbook frames per second.
    pub flipbook_fps: f32,
    /// 0x10C: alpha state 0..=7.
    pub alpha: u8,
    /// 0x110: time scale of the emitter.
    pub time_scale: f32,
    /// 0x188, 0x18C: bezier tube radius min and max.
    pub tube_radius: [f32; 2],
    /// 0x190: speed along the emitter path.
    pub path_speed: f32,
    /// 0x19C, 0x1A0: start delay min and max, seconds.
    pub start_delay: [f32; 2],
    /// 0x1A8..0x1E4: emitter matrix, rows (row-vector convention, translation in row 3).
    pub matrix: [f32; 16],
    /// 0x1E8, 0x1EC: burst interval min and max, seconds.
    pub burst_interval: [f32; 2],
    /// 0x1F0, 0x1F4: per-particle extra range.
    pub extra: [f32; 2],
    /// 0x1F8: see [`flag2`].
    pub flags2: u32,
}

/// One texture asset of an emitter: a flipbook frame, or the one texture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxFrame {
    /// `tx:` texture; `None` when the asset is not a texture or the install lacks the file.
    pub texture: Option<LogicalId>,
    /// u0, v0, u1, v1.
    pub uv: [f32; 4],
}

/// A force of an emitter (0x20 bytes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxForce {
    pub position: [f32; 3],
    pub strength: f32,
    pub radius: f32,
    /// 0 repel, 1 attract (1/r²), 2 random jitter, 4 constant vector.
    pub kind: u8,
}

/// The pixel shader the emitter picks by its shader asset (`ForkXShader`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FxShader {
    /// `ForkDefaultShader` (package 0x2D).
    Default,
    /// `ForkFakeVolumeShader` (package 0x31) with its gradient texture.
    FakeVolume { gradient: Option<LogicalId> },
    /// `ForkBrightnessShader` (package 0x2F; `Glow.txt` builds it too) with its float.
    Brightness(f32),
    /// `ForkMaskShader` (package 0x30) with its float4.
    Mask([f32; 4]),
}

/// The animation track of an emitter: whole-property keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxTrack {
    pub fps: f32,
    pub looped: bool,
    /// Frame and the property at it, frames ascending.
    pub keys: Vec<(u32, FxProps)>,
}

/// The baked bezier path of an emitter (property 0x170, struct at 0x17C).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxPath {
    /// Property 0x170: b0, b1 birth modes, b2 parameter in index units.
    pub mode: u32,
    /// Total length.
    pub length: f32,
    pub points: Vec<[f32; 3]>,
    /// `deltas[i]` is the step from point `i - 1` to point `i`.
    pub deltas: Vec<[f32; 3]>,
    pub origin: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxEmitter {
    /// Chunk name (`FX_Center_Glow`).
    pub name: String,
    pub props: FxProps,
    pub frames: Vec<FxFrame>,
    pub forces: Vec<FxForce>,
    pub shader: FxShader,
    pub track: Option<FxTrack>,
    pub path: Option<FxPath>,
    /// Index in [`FxEffect::emitters`] of the emitter each particle spawns (ref asset 0).
    pub sub_emitter: Option<u16>,
}

/// One `.psb`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FxEffect {
    /// File name without extension, lower case.
    pub name: String,
    pub emitters: Vec<FxEmitter>,
}

impl FxEffect {
    pub const VERSION: u16 = 2;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(FX_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, fx): (u16, FxEffect) =
            blob::decode_range(FX_MAGIC, Self::VERSION, Self::VERSION, bytes)?;
        fx.validate()?;
        Ok(fx)
    }

    /// The numbers a player indexes by or divides with.
    pub fn validate(&self) -> Result<(), FormatError> {
        let n = self.emitters.len();
        for (i, e) in self.emitters.iter().enumerate() {
            let bad = |why: &str| FormatError::invalid(format!("emitter {i} ({}): {why}", e.name));
            check_props(&e.props).map_err(bad)?;
            if let Some(s) = e.sub_emitter
                && usize::from(s) >= n
            {
                return Err(bad("sub-emitter out of range"));
            }
            if let Some(t) = &e.track {
                if !(t.fps.is_finite() && t.fps > 0.0) || t.keys.is_empty() {
                    return Err(bad("track without keys or fps"));
                }
                if t.keys.windows(2).any(|w| w[1].0 < w[0].0) {
                    return Err(bad("track frames not ascending"));
                }
                for (_, k) in &t.keys {
                    check_props(k).map_err(bad)?;
                }
            }
            if let Some(p) = &e.path
                && (p.points.len() != p.deltas.len() || p.points.len() < 2 || !p.length.is_finite())
            {
                return Err(bad("path arrays"));
            }
            for f in &e.frames {
                if f.uv.iter().any(|v| !v.is_finite()) {
                    return Err(bad("frame uv not finite"));
                }
            }
        }
        Ok(())
    }
}

fn check_props(p: &FxProps) -> Result<(), &'static str> {
    if p.alpha > 7 {
        return Err("alpha state above 7");
    }
    if p.volume_kind > 11 {
        return Err("birth volume above 11");
    }
    if p.shape > 3 {
        return Err("shape above 3");
    }
    let floats = p
        .colors
        .iter()
        .flatten()
        .chain(&p.times)
        .chain(&p.life)
        .chain(&p.speed)
        .chain(&p.size_max)
        .chain(&p.spin)
        .chain(&p.size_scale)
        .chain(&p.rotation)
        .chain(&p.tint)
        .chain(&p.size_min)
        .chain(&p.volume)
        .chain(&p.tube_radius)
        .chain(&p.start_delay)
        .chain(&p.matrix)
        .chain(&p.burst_interval)
        .chain(&p.extra)
        .chain([
            &p.mid_time,
            &p.drag,
            &p.prewarm,
            &p.rate,
            &p.gravity,
            &p.max_live,
            &p.flipbook_fps,
            &p.time_scale,
            &p.path_speed,
        ]);
    if floats.into_iter().any(|v| !v.is_finite()) {
        return Err("a number is not finite");
    }
    Ok(())
}

/// A `VisEffectArtInfo` that names particle effects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FxVisEffect {
    /// `ART_DEF_VEFFECT_*`.
    pub name: String,
    pub translation: [f32; 3],
    /// Degrees, applied x then y then z.
    pub rotation: [f32; 3],
    pub scale: f32,
    /// `fBaseDuration * fBaseDurationScale`, seconds.
    pub duration: f32,
    /// `FOWMask & 3` (default 2: only while visible).
    pub fow_mask: u8,
    /// The `fx:` effects, in the order of the file; a name without a `.psb` is left out.
    pub particles: Vec<LogicalId>,
}

/// `fx:index`: the visual effects with particles, by name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FxIndex {
    /// Sorted by name.
    pub effects: Vec<FxVisEffect>,
}

impl FxIndex {
    pub const VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(FX_INDEX_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, idx): (u16, FxIndex) =
            blob::decode_range(FX_INDEX_MAGIC, Self::VERSION, Self::VERSION, bytes)?;
        if idx.effects.windows(2).any(|w| w[1].name <= w[0].name) {
            return Err(FormatError::invalid("fx index not sorted by name"));
        }
        if idx
            .effects
            .iter()
            .any(|e| !(e.scale.is_finite() && e.duration.is_finite()) || e.fow_mask > 3)
        {
            return Err(FormatError::invalid("fx index: scale, duration or mask"));
        }
        Ok(idx)
    }

    pub fn get(&self, name: &str) -> Option<&FxVisEffect> {
        self.effects
            .binary_search_by(|e| e.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.effects[i])
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn props() -> FxProps {
        FxProps {
            colors: [[0.0, 0.0, 0.0, 0.4], [1.0; 4], [0.5; 4], [0.0; 4]],
            times: [0.3, 0.6],
            life: [1.0, 2.0],
            speed: [0.0, 1.0],
            flags: flag::LOOK_AT,
            size_max: [10.0, 4.0, 0.0],
            mid_time: 0.5,
            spin: [0.0, 0.5],
            drag: 0.5,
            size_scale: [1.0; 3],
            rotation: [0.0; 3],
            tint: [1.0; 4],
            size_min: [10.0, 0.0, 0.0],
            prewarm: 0.0,
            rate: 1.0,
            gravity: 0.0,
            volume: [1.0, 1.0, 1.0, 2.5],
            max_live: 250.0,
            volume_kind: 8,
            shape: 1,
            burst: 0,
            flipbook_fps: 0.0,
            alpha: 1,
            time_scale: 1.0,
            tube_radius: [0.0; 2],
            path_speed: 10.0,
            start_delay: [0.0; 2],
            matrix: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.5, 0.0, 1.0,
            ],
            burst_interval: [0.0; 2],
            extra: [0.0; 2],
            flags2: flag2::UNIFORM_SIZE,
        }
    }

    pub fn effect() -> FxEffect {
        FxEffect {
            name: "fx_look_at_me".into(),
            emitters: vec![
                FxEmitter {
                    name: "FX_Center_Glow".into(),
                    props: props(),
                    frames: vec![FxFrame {
                        texture: Some(LogicalId::new("tx:fx_flash_01.dds")),
                        uv: [0.0, 0.0, 1.0, 1.0],
                    }],
                    forces: vec![FxForce {
                        position: [0.0; 3],
                        strength: 1.0,
                        radius: 2.0,
                        kind: 4,
                    }],
                    shader: FxShader::FakeVolume {
                        gradient: Some(LogicalId::new("tx:fx_gradient_wtorYl.dds")),
                    },
                    track: Some(FxTrack {
                        fps: 60.0,
                        looped: true,
                        keys: vec![(0, props()), (30, props())],
                    }),
                    path: None,
                    sub_emitter: Some(1),
                },
                FxEmitter {
                    name: "child".into(),
                    props: props(),
                    frames: vec![],
                    forces: vec![],
                    shader: FxShader::Brightness(0.25),
                    track: None,
                    path: Some(FxPath {
                        mode: 1,
                        length: 2.0,
                        points: vec![[0.0; 3], [1.0, 0.0, 0.0]],
                        deltas: vec![[0.0; 3], [1.0, 0.0, 0.0]],
                        origin: [0.0; 3],
                    }),
                    sub_emitter: None,
                },
            ],
        }
    }

    #[test]
    fn roundtrip_is_deterministic() {
        let e = effect();
        let b = e.encode();
        assert_eq!(b, e.encode());
        assert_eq!(FxEffect::decode(&b).unwrap(), e);
        let idx = FxIndex {
            effects: vec![
                FxVisEffect {
                    name: "A".into(),
                    translation: [0.0; 3],
                    rotation: [90.0, 0.0, 0.0],
                    scale: 4.2,
                    duration: 1.1,
                    fow_mask: 3,
                    particles: vec![LogicalId::new("fx:fx_look_at_me")],
                },
                FxVisEffect {
                    name: "B".into(),
                    translation: [0.0; 3],
                    rotation: [0.0; 3],
                    scale: 1.0,
                    duration: 1.0,
                    fow_mask: 2,
                    particles: vec![],
                },
            ],
        };
        let back = FxIndex::decode(&idx.encode()).unwrap();
        assert_eq!(back, idx);
        assert_eq!(back.get("B").unwrap().fow_mask, 2);
        assert!(back.get("C").is_none());
    }

    fn bad(f: impl FnOnce(&mut FxEffect)) -> bool {
        let mut e = effect();
        f(&mut e);
        FxEffect::decode(&e.encode()).is_err()
    }

    #[test]
    fn hostile_effects_are_errors() {
        assert!(bad(|e| e.emitters[0].props.alpha = 8));
        assert!(bad(|e| e.emitters[0].props.volume_kind = 12));
        assert!(bad(|e| e.emitters[0].props.shape = 4));
        assert!(bad(|e| e.emitters[0].props.rate = f32::NAN));
        assert!(bad(|e| e.emitters[0].sub_emitter = Some(2)));
        assert!(bad(|e| e.emitters[0]
            .track
            .as_mut()
            .unwrap()
            .keys
            .reverse()));
        assert!(bad(|e| e.emitters[0].track.as_mut().unwrap().fps = 0.0));
        assert!(bad(|e| e.emitters[1]
            .path
            .as_mut()
            .unwrap()
            .deltas
            .pop()
            .map_or((), |_| ())));
        let good = effect().encode();
        for cut in 0..good.len() {
            assert!(FxEffect::decode(&good[..cut]).is_err(), "cut {cut}");
        }
        let mut idx = FxIndex::default();
        idx.effects.push(FxVisEffect {
            name: "B".into(),
            translation: [0.0; 3],
            rotation: [0.0; 3],
            scale: 1.0,
            duration: 1.0,
            fow_mask: 2,
            particles: vec![],
        });
        idx.effects.push(idx.effects[0].clone());
        assert!(FxIndex::decode(&idx.encode()).is_err(), "duplicate names");
    }
}
