//! Every resource of the game DB has an icon the UI can draw (player report: "In trade, not every resource has an
//! icon, e.g. coconuts"). The trade screen lists resources as `[ICON_RES_X]` text, so the font icon of each
//! `Resources` row must resolve: `IconFontMapping` -> `IconFontTextures` -> a `.ggxml` in the set -> its texture; the
//! `IconAtlas` + `PortraitIndex` of the row (map and tooltips) must name a texture in the set at size 45.
//!
//! Runs only when `CIV5_ROOT` (the game) and `CIV5_DB_SNAPSHOT` (a `Civ5*Database.db` file, or the folder that holds
//! `Civ5DebugDatabase.db`) are set.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use asset_format::UiTables;
use asset_pipeline::db::SnapDb;
use asset_pipeline::ui::{UiOptions, icon_font_entries, open_vfs};

fn snapshot_file() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("CIV5_DB_SNAPSHOT")?);
    Some(if p.is_dir() {
        p.join("Civ5DebugDatabase.db")
    } else {
        p
    })
}

fn texture_names(ggxml: &str) -> Vec<String> {
    ggxml
        .split("src=\"")
        .skip(1)
        .filter_map(|s| s.split('"').next())
        .filter(|s| s.to_ascii_lowercase().ends_with(".dds"))
        .map(|s| {
            s.rsplit(['/', '\\'])
                .next()
                .unwrap_or(s)
                .to_ascii_lowercase()
        })
        .collect()
}

#[test]
fn every_resource_has_a_font_icon_and_an_atlas_icon_in_the_set() {
    let (Some(root), Some(snap)) = (std::env::var_os("CIV5_ROOT"), snapshot_file()) else {
        eprintln!("CIV5_ROOT / CIV5_DB_SNAPSHOT not set: skipped");
        return;
    };
    let vfs = open_vfs(Path::new(&root), &UiOptions::default()).unwrap();
    let db = SnapDb::open_read_only(&snap).unwrap();

    // The tables of the set as the DB states them.
    let mut tables = UiTables::default();
    for r in db
        .query(
            "SELECT IconFontTexture, IconFontTextureFile FROM IconFontTextures",
            2,
        )
        .unwrap()
    {
        tables.icon_textures.push((
            r[0].clone().unwrap_or_default(),
            r[1].clone().unwrap_or_default(),
        ));
    }
    let mapping: BTreeMap<String, String> = db
        .query("SELECT IconName, IconFontTexture FROM IconFontMapping", 2)
        .unwrap()
        .into_iter()
        .filter_map(|r| Some((r[0].clone()?, r[1].clone()?)))
        .collect();
    let font_file: BTreeMap<String, String> = tables.icon_textures.iter().cloned().collect();
    // The fonts the build puts into the set: what the tables name, wherever the file sits.
    let in_set: Vec<String> = icon_font_entries(&vfs, &tables)
        .iter()
        .map(|e| e.name.clone())
        .collect();

    let mut problems: Vec<String> = Vec::new();
    let mut checked = 0;
    for r in db
        .query(
            "SELECT Type, IconString, PortraitIndex, IconAtlas FROM Resources",
            4,
        )
        .unwrap()
    {
        let ty = r[0].clone().unwrap_or_default();
        checked += 1;
        // Font icon.
        let name = r[1].clone().unwrap_or_default();
        let name = name.trim_matches(['[', ']']).to_owned();
        match mapping.get(&name).and_then(|k| font_file.get(k)) {
            None => problems.push(format!(
                "{ty}: {name} has no IconFontMapping/IconFontTextures row"
            )),
            Some(file) => {
                let ggxml = format!("{}.ggxml", file.to_ascii_lowercase());
                match vfs.lookup(&ggxml) {
                    None => problems.push(format!("{ty}: font {ggxml} is not in the VFS")),
                    Some(e) => {
                        if !in_set.contains(&ggxml) && !e.is_packed() {
                            // A font outside the UI roots must still be picked up by `icon_font_entries`.
                            problems.push(format!("{ty}: font {ggxml} is not taken into the set"));
                        }
                        let text = String::from_utf8_lossy(&vfs.read(e).unwrap()).into_owned();
                        for t in texture_names(&text) {
                            match vfs.lookup(&t) {
                                None => problems.push(format!(
                                    "{ty}: font {ggxml} names texture {t} that is not in the VFS"
                                )),
                                Some(te) => {
                                    // The build must be able to convert it.
                                    if let Err(err) = asset_pipeline::ui::tex::ui_texture(
                                        &vfs.read(te).unwrap(),
                                        None,
                                    ) {
                                        problems.push(format!("{ty}: font texture {t}: {err}"));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        // Atlas icon at size 45.
        let (Some(index), Some(atlas)) = (r[2].clone(), r[3].clone()) else {
            problems.push(format!("{ty}: no IconAtlas/PortraitIndex"));
            continue;
        };
        let rows = db
            .query(
                &format!(
                    "SELECT Filename, IconsPerRow, IconsPerColumn FROM IconTextureAtlases WHERE Atlas = {} AND IconSize = 45",
                    asset_pipeline::db::quote(&atlas)
                ),
                3,
            )
            .unwrap();
        let Some(a) = rows.first() else {
            problems.push(format!("{ty}: atlas {atlas} has no 45 px row"));
            continue;
        };
        let file = a[0].clone().unwrap_or_default();
        let bare = file
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&file)
            .to_ascii_lowercase();
        if vfs.lookup(&bare).is_none() {
            problems.push(format!(
                "{ty}: atlas texture {bare} ({atlas}) is not in the VFS"
            ));
        }
        let per: i64 = a[1].as_deref().unwrap_or("1").parse().unwrap_or(1);
        let cols: i64 = a[2].as_deref().unwrap_or("1").parse().unwrap_or(1);
        let idx: i64 = index.parse().unwrap_or(-1);
        if idx < 0 || idx >= per * cols {
            problems.push(format!(
                "{ty}: PortraitIndex {idx} outside {atlas} ({per}x{cols})"
            ));
        }
    }
    assert!(checked > 40, "only {checked} resources read");
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
