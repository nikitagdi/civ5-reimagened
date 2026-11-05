//! `golden far` (B3 far LOD): the far-view scene S5, which has no capture of the original. The world of another
//! scene (`[far] world`) is drawn natively from the game's camera at `[far] level` three times: `FarLOD:0`,
//! `FarLOD:1` and `FarLOD:0` again. The frames are compared by mean FLIP over the class masks of the first one:
//! off against on is what the LOD changes, off against off again is the noise of the frame itself.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::image::read_raw_rgba;
use crate::metrics::{Mask, PairMaps};
use crate::render::{Backend, finish_frame};
use crate::report::{MASKS, Select};
use crate::scene::Scene;

pub struct Options {
    pub cache: PathBuf,
    pub scenes_dir: PathBuf,
    /// Frames and the summary go here (`target/golden/far`).
    pub out: PathBuf,
    /// Root of the workspace (cargo runs from here).
    pub root: PathBuf,
    /// The assets as the server serves them.
    pub assets: PathBuf,
}

/// The frames of the scene: the name and the settings on top of the preset.
pub const FRAMES: [(&str, &str); 3] = [
    ("S5-far-off", "FarLOD:0"),
    ("S5-far-on", "FarLOD:1"),
    ("S5-far-off2", "FarLOD:0"),
];

/// One mask of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub mask: &'static str,
    /// Pixels of the mask after the erosion.
    pub pixels: usize,
    pub flip: f64,
}

/// The `.golden` text of one frame of the far view.
pub fn spec_text(scene: &Scene, capture: &str, gfx: &str, level: f32) -> String {
    let r = &scene.render;
    let mut s = String::from("civ5-golden/1\n");
    let _ = writeln!(s, "capture {capture}");
    let _ = writeln!(s, "size {} {}", r.size[0], r.size[1]);
    let _ = writeln!(s, "preset {}", r.preset);
    let _ = writeln!(s, "msaa {}", r.msaa);
    let _ = writeln!(s, "disable {}", r.disable.join(","));
    let _ = writeln!(s, "grid {}", u8::from(r.grid));
    if let Some(secs) = r.settle_s {
        let _ = writeln!(s, "settle {secs}");
    }
    if scene.view.reveal == "capture-mask" {
        let _ = writeln!(s, "reveal all");
    }
    let _ = writeln!(s, "time 0");
    let _ = writeln!(s, "gfx {gfx}");
    let _ = writeln!(s, "game_camera {level}");
    s
}

/// Mean FLIP of `test` against `reference` (both `<name>-native.rgba` in `dir`) over the whole frame and over each
/// mask of the class mask of `reference`.
pub fn compare(dir: &Path, reference: &str, test: &str) -> Result<Vec<Row>> {
    let size = |name: &str| -> Result<(usize, usize)> {
        let p = dir.join(format!("{name}-native.txt"));
        let text = std::fs::read_to_string(&p).with_context(|| format!("{}", p.display()))?;
        let n: Vec<usize> = text
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .with_context(|| format!("{}: size file", p.display()))?;
        match n.as_slice() {
            [w, h] => Ok((*w, *h)),
            _ => bail!("{}: size file needs width and height", p.display()),
        }
    };
    let (w, h) = size(reference)?;
    if size(test)? != (w, h) {
        bail!("{reference} and {test} differ in size");
    }
    let read = |name: &str| read_raw_rgba(&dir.join(format!("{name}-native.rgba")), w, h);
    let maps = PairMaps::new(&read(reference)?.to_image(), &read(test)?.to_image());
    let classes = std::fs::read(dir.join(format!("{reference}-native.mask.r8")))
        .with_context(|| format!("the class mask of {reference}"))?;
    if classes.len() != w * h {
        bail!(
            "{reference}: class mask has {} bytes for {w}x{h}",
            classes.len()
        );
    }
    let mut rows = Vec::new();
    let mut push = |mask: &'static str, m: Mask| {
        let s = maps.over(&m);
        rows.push(Row {
            mask,
            pixels: s.flip.count,
            flip: s.flip.mean,
        });
    };
    push("frame", Mask::full(w, h));
    push("all", Mask::all_non_empty(w, h, &classes));
    for (name, select) in MASKS {
        push(
            name,
            match select {
                Select::Class(n) => Mask::from_class(w, h, &classes, n),
                Select::Flag(f) => Mask::from_flag(w, h, &classes, f),
            },
        );
    }
    Ok(rows)
}

/// Draws the three frames and compares them; returns the summary (also `<out>/S5-far.md`).
pub fn run(o: &Options) -> Result<String> {
    let scene = Scene::load_by_id(&o.scenes_dir, "S5")?;
    let far = scene.far.clone().context("S5 has no [far] section")?;
    let world_dir = o.cache.join("golden").join(&far.world);
    for f in ["mapbasis.wv3", "world.wv3"] {
        if !world_dir.join(f).is_file() {
            bail!(
                "far: {} is missing: run `cargo xtask golden world {}` first",
                world_dir.join(f).display(),
                far.world
            );
        }
    }
    for (name, gfx) in FRAMES {
        std::fs::write(
            world_dir.join(format!("{name}.golden")),
            spec_text(&scene, name, gfx, far.level),
        )?;
    }
    std::fs::create_dir_all(&o.out)?;
    let mut c = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    c.args(["run", "-p", "app-desktop", "--release", "--", "--golden"])
        .arg(&world_dir)
        .arg("--out")
        .arg(&o.out)
        .arg("--assets")
        .arg(&o.assets);
    for (name, _) in FRAMES {
        c.args(["--capture", name]);
    }
    c.current_dir(&o.root);
    let status = c.status().context("cannot start app-desktop --golden")?;
    if !status.success() {
        bail!("app-desktop --golden failed ({status})");
    }
    for (name, _) in FRAMES {
        finish_frame(&o.out, name, Backend::Native)?;
    }
    let mut text = format!(
        "# Far view S5: the world of {} at zoom level {}\n\n| mask | pixels | FLIP off vs on | FLIP off vs off again |\n|---|---:|---:|---:|\n",
        far.world, far.level
    );
    let lod = compare(&o.out, FRAMES[0].0, FRAMES[1].0)?;
    let noise = compare(&o.out, FRAMES[0].0, FRAMES[2].0)?;
    for (a, b) in lod.iter().zip(&noise) {
        let _ = writeln!(
            text,
            "| {} | {} | {:.4} | {:.4} |",
            a.mask, a.pixels, a.flip, b.flip
        );
    }
    std::fs::write(o.out.join("S5-far.md"), &text)?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_of_a_far_frame_has_the_game_camera_and_the_setting() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes");
        let scene = Scene::load_by_id(&dir, "S5").unwrap();
        let text = spec_text(&scene, "S5-far-on", "FarLOD:1", 20.5);
        assert!(text.starts_with("civ5-golden/1\ncapture S5-far-on\nsize 1920 1080\n"));
        assert!(text.contains("\ngfx FarLOD:1\n") && text.contains("\ngame_camera 20.5\n"));
        assert!(text.contains("\nreveal all\n") && text.contains("\ngrid 0\n"));
        assert!(!text.contains("view_proj_orig"));
    }

    #[test]
    fn equal_frames_do_not_differ() {
        let dir = std::env::temp_dir().join(format!("golden-far-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (w, h) = (40usize, 30usize);
        let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 13 % 251) as u8).collect();
        for name in ["A", "B"] {
            std::fs::write(dir.join(format!("{name}-native.rgba")), &rgba).unwrap();
            std::fs::write(dir.join(format!("{name}-native.txt")), format!("{w} {h}")).unwrap();
        }
        std::fs::write(dir.join("A-native.mask.r8"), vec![4u8; w * h]).unwrap();
        let rows = compare(&dir, "A", "B").unwrap();
        let forest = rows.iter().find(|r| r.mask == "forest").unwrap();
        assert!(forest.pixels > 0 && forest.flip < 1e-6);
        let land = rows.iter().find(|r| r.mask == "land").unwrap();
        assert_eq!(land.pixels, 0);
        std::fs::remove_dir_all(&dir).ok();
    }
}
