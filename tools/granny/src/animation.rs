//! Typed animations (`granny_animation`, `granny_track_group`, `granny_transform_track`).

use crate::curve::Curve;
use crate::{GrannyError, Object, Transform};

/// Local pose of one bone at one time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub translation: [f32; 3],
    /// Normalised quaternion (x, y, z, w).
    pub rotation: [f32; 4],
    /// Row-major 3x3.
    pub scale_shear: [f32; 9],
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransformTrack {
    /// `None` in packed game files.
    pub name: Option<String>,
    /// Raw ID stored in place of the name in packed game files.
    pub name_id: Option<u32>,
    pub flags: u32,
    pub position: Curve,
    pub orientation: Curve,
    pub scale_shear: Curve,
}

impl TransformTrack {
    /// Pose at `t`. The quaternion is normalised; a degenerate one becomes identity.
    pub fn sample(&self, t: f32) -> Pose {
        let rotation = self.sample_raw(t).rotation;
        Pose {
            rotation: normalize(rotation),
            ..self.sample_raw(t)
        }
    }

    /// Like `sample` but with the quaternion as evaluated (for validation).
    pub fn sample_raw(&self, t: f32) -> Pose {
        Pose {
            translation: self.position.sample::<3>(t),
            rotation: self.orientation.sample::<4>(t),
            scale_shear: self.scale_shear.sample::<9>(t),
        }
    }
}

pub fn normalize(q: [f32; 4]) -> [f32; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if n.is_finite() && n > 1e-6 {
        q.map(|x| x / n)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackGroup {
    pub name: Option<String>,
    pub transform_tracks: Vec<TransformTrack>,
    pub initial_placement: Transform,
    pub accumulation_flags: u32,
    pub loop_translation: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub name: Option<String>,
    pub duration: f32,
    pub time_step: f32,
    pub oversampling: f32,
    pub track_groups: Vec<TrackGroup>,
}

fn check_dim(c: &Curve, dim: usize, what: &str) -> Result<(), GrannyError> {
    if c.dim == dim {
        Ok(())
    } else {
        Err(GrannyError::Invalid(format!(
            "{what} curve has {} components, expected {dim}",
            c.dim
        )))
    }
}

fn read_curve(
    track: &Object<'_>,
    field: &str,
    dim: usize,
    duration: f32,
) -> Result<Curve, GrannyError> {
    let holder = track.inline(field)?;
    let data = holder
        .reference("CurveData")?
        .ok_or_else(|| GrannyError::NullReference(format!("{field}.CurveData")))?;
    let c = Curve::read(&data, duration)?;
    check_dim(&c, dim, field)?;
    Ok(c)
}

impl TransformTrack {
    pub fn read(o: &Object<'_>, duration: f32) -> Result<TransformTrack, GrannyError> {
        let (name, name_id) = o.name_and_id("Name")?;
        Ok(TransformTrack {
            name,
            name_id,
            flags: o.u32("Flags").unwrap_or(0),
            position: read_curve(o, "PositionCurve", 3, duration)?,
            orientation: read_curve(o, "OrientationCurve", 4, duration)?,
            scale_shear: read_curve(o, "ScaleShearCurve", 9, duration)?,
        })
    }
}

impl TrackGroup {
    pub fn read(o: &Object<'_>, duration: f32) -> Result<TrackGroup, GrannyError> {
        let mut transform_tracks = Vec::new();
        for t in o.array("TransformTracks")?.iter() {
            transform_tracks.push(TransformTrack::read(&t, duration)?);
        }
        let loop_translation = match o.get("LoopTranslation")? {
            crate::Value::Fixed(f) => {
                let mut v = [0.0f32; 3];
                for (d, x) in v.iter_mut().zip(f.iter()) {
                    if let crate::Value::Real32(x) = x {
                        *d = x;
                    }
                }
                v
            }
            _ => [0.0; 3],
        };
        Ok(TrackGroup {
            name: o.string("Name")?,
            transform_tracks,
            initial_placement: o.transform("InitialPlacement")?,
            accumulation_flags: o.u32("AccumulationFlags").unwrap_or(0),
            loop_translation,
        })
    }
}

impl Animation {
    pub fn read(o: &Object<'_>) -> Result<Animation, GrannyError> {
        let duration = o.f32("Duration")?;
        if !duration.is_finite() || duration < 0.0 {
            return Err(GrannyError::Invalid(format!("bad duration {duration}")));
        }
        let mut track_groups = Vec::new();
        for g in o.references("TrackGroups")?.iter()? {
            if let Some(g) = g? {
                track_groups.push(TrackGroup::read(&g, duration)?);
            }
        }
        Ok(Animation {
            name: o.string("Name")?,
            duration,
            time_step: o.f32("TimeStep")?,
            oversampling: o.f32("Oversampling")?,
            track_groups,
        })
    }

    /// Total number of transform tracks over all groups.
    pub fn track_count(&self) -> usize {
        self.track_groups
            .iter()
            .map(|g| g.transform_tracks.len())
            .sum()
    }
}
