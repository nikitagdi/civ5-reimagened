//! Shared helpers of the integration tests: the layered files of the game install.
#![allow(dead_code)]

pub mod ctx;
pub mod harness;

use std::path::{Path, PathBuf};

/// `Assets` of the game install, or `None` (with a message) when it is not available.
pub fn game_assets() -> Option<PathBuf> {
    let candidates = [std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from)];
    for dir in candidates.into_iter().flatten() {
        let assets = dir.join("Assets");
        if assets.join("UI/InstanceManager.lua").is_file() {
            return Some(assets);
        }
    }
    eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
    None
}

/// The UI layers in override order, lowest first: base, BNW, Lekmod, EUI.
pub const UI_LAYERS: [&str; 5] = [
    "UI",
    "Gameplay/Lua",
    "DLC/Expansion2/UI",
    "DLC/LEKMOD_v34.15/Lua/UI",
    "DLC/UI_bc1",
];

pub fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}
