//! `golden mesh <scene>`: the vertices of the water and river draws of a capture as OBJ files
//! Water (`WaterShader::VSWaterOcean`, `VSWaterRiver`) is the Must; any
//! other vertex shader goes through `--match`.
//!
//! The OBJ has the POSITION of the draw's vertices in the order the draw reads them, with the faces
//! of its topology. A river drawn as a triangle strip also gets `centreline`: the midpoints of the
//! vertex pairs `(2k, 2k + 1)`, in the summary file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::captures::{DrawsFile, parsed_dir};
use crate::replay::inputs::{CaptureData, fetch_vertices};
use crate::scene::Scene;

pub const WATER_ENTRIES: [&str; 2] = ["WaterShader::VSWaterOcean", "WaterShader::VSWaterRiver"];

pub struct Options {
    pub cache: PathBuf,
    pub scenes_dir: PathBuf,
    pub out: PathBuf,
    /// Substrings of the vertex entry names; empty: the water entries.
    pub matches: Vec<String>,
    /// Only these captures; empty: all of the scene's.
    pub captures: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct MeshInfo {
    pub capture: String,
    pub event: u32,
    pub entry: String,
    pub topology: String,
    pub vertices: usize,
    pub triangles: usize,
    pub bbox_min: [f32; 3],
    pub bbox_max: [f32; 3],
    pub obj: String,
    /// River strips only.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub centreline: Vec<[f32; 3]>,
}

/// Triangles (vertex numbers) of a topology over `n` vertices.
pub fn triangles(topology: &str, n: usize) -> Vec<[usize; 3]> {
    match topology {
        "TriangleList" => (0..n / 3).map(|t| [3 * t, 3 * t + 1, 3 * t + 2]).collect(),
        "TriangleStrip" => (0..n.saturating_sub(2))
            .map(|t| {
                if t % 2 == 0 {
                    [t, t + 1, t + 2]
                } else {
                    [t + 1, t, t + 2]
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn obj_text(pos: &[[f32; 4]], tris: &[[usize; 3]]) -> String {
    let mut s = String::new();
    for p in pos {
        s.push_str(&format!("v {} {} {}\n", p[0], p[1], p[2]));
    }
    for t in tris {
        s.push_str(&format!("f {} {} {}\n", t[0] + 1, t[1] + 1, t[2] + 1));
    }
    s
}

pub fn run(opts: &Options, scene_id: &str) -> Result<Vec<MeshInfo>> {
    let scene = Scene::load_by_id(&opts.scenes_dir, scene_id)?;
    let wanted: Vec<String> = if opts.matches.is_empty() {
        WATER_ENTRIES.iter().map(|s| (*s).to_string()).collect()
    } else {
        opts.matches.clone()
    };
    let mut out = Vec::new();
    for capture in scene.required_captures() {
        if !opts.captures.is_empty() && !opts.captures.iter().any(|c| c == capture) {
            continue;
        }
        let dir = parsed_dir(&opts.cache, scene_id, capture);
        let draws = DrawsFile::load(&dir).with_context(|| format!("capture {capture}"))?;
        let data = CaptureData::load(&dir)?;
        for d in &draws.draws {
            let Some(entry) = d.vs_entry.as_deref() else {
                continue;
            };
            if !wanted.iter().any(|w| entry.contains(w.as_str())) {
                continue;
            }
            let Some(call) = data.calls.get(&d.event) else {
                continue;
            };
            let want = [("position".to_string(), 0u32)];
            let v = match fetch_vertices(&data, call, &want, crate::replay::VERTEX_LIMIT) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("{capture} event {}: {e:#}", d.event);
                    continue;
                }
            };
            let pos: Vec<[f32; 4]> = v.data.iter().filter_map(|r| r.first().copied()).collect();
            if pos.is_empty() {
                continue;
            }
            let tris = triangles(&call.topology, pos.len());
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &pos {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let dir_out = opts.out.join(scene_id).join(capture);
            std::fs::create_dir_all(&dir_out)?;
            let name = format!(
                "{}-{}.obj",
                d.event,
                entry.replace("::", "_").replace(':', "_")
            );
            std::fs::write(dir_out.join(&name), obj_text(&pos, &tris))?;
            let centreline = if call.topology == "TriangleStrip" && entry.contains("River") {
                pos.chunks_exact(2)
                    .map(|p| {
                        [
                            (p[0][0] + p[1][0]) / 2.0,
                            (p[0][1] + p[1][1]) / 2.0,
                            (p[0][2] + p[1][2]) / 2.0,
                        ]
                    })
                    .collect()
            } else {
                Vec::new()
            };
            out.push(MeshInfo {
                capture: capture.to_string(),
                event: d.event,
                entry: entry.to_string(),
                topology: call.topology.clone(),
                vertices: pos.len(),
                triangles: tris.len(),
                bbox_min: lo,
                bbox_max: hi,
                obj: format!("{scene_id}/{capture}/{name}"),
                centreline,
            });
        }
    }
    std::fs::create_dir_all(&opts.out)?;
    std::fs::write(
        opts.out.join(format!("{scene_id}-meshes.json")),
        serde_json::to_string_pretty(&out)? + "\n",
    )?;
    Ok(out)
}

/// Where the summary of `run` is.
pub fn summary_path(out: &Path, scene_id: &str) -> PathBuf {
    out.join(format!("{scene_id}-meshes.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_and_lists_make_triangles() {
        assert_eq!(triangles("TriangleList", 6), vec![[0, 1, 2], [3, 4, 5]]);
        assert_eq!(triangles("TriangleStrip", 4), vec![[0, 1, 2], [2, 1, 3]]);
        assert!(triangles("LineList", 4).is_empty());
    }

    #[test]
    fn obj_faces_are_one_based() {
        let t = obj_text(
            &[[0.0; 4], [1.0, 0.0, 0.0, 1.0], [0.0, 1.0, 0.0, 1.0]],
            &[[0, 1, 2]],
        );
        assert!(t.ends_with("f 1 2 3\n"));
    }
}
