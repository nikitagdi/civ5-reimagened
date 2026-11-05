//! Finds the Steam install of Sid Meier's Civilization V (the fallback when neither `--game` nor `CIV5_ROOT` names
//! one): the Steam folder (Windows: `SteamPath` of `HKCU\Software\Valve\Steam`, then the default install folders;
//! Linux: `~/.steam/steam`, `~/.local/share/Steam`, the Flatpak folder; macOS: `~/Library/Application Support/Steam`),
//! every library of its `steamapps/libraryfolders.vdf`, and in each `steamapps/common/Sid Meier's Civilization V`.

use std::path::{Path, PathBuf};

/// Folder name of the game under `steamapps/common`.
pub const GAME_FOLDER: &str = "Sid Meier's Civilization V";

/// True if `dir` looks like the game's install folder.
pub fn is_game_dir(dir: &Path) -> bool {
    dir.join("Assets").is_dir() && dir.join("Resource").is_dir()
}

/// The Steam folders of this machine that exist, most likely first.
pub fn steam_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(windows) {
        if let Some(p) = registry_steam_path() {
            out.push(p);
        }
        for var in ["ProgramFiles(x86)", "ProgramFiles"] {
            if let Some(p) = std::env::var_os(var) {
                out.push(PathBuf::from(p).join("Steam"));
            }
        }
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        if cfg!(target_os = "macos") {
            out.push(home.join("Library/Application Support/Steam"));
        } else {
            out.push(home.join(".steam/steam"));
            out.push(home.join(".local/share/Steam"));
            out.push(home.join(".var/app/com.valvesoftware.Steam/.local/share/Steam"));
        }
    }
    let mut seen = Vec::new();
    out.retain(|p| {
        let key = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let keep = p.is_dir() && !seen.contains(&key);
        seen.push(key);
        keep
    });
    out
}

/// `SteamPath` of `HKCU\Software\Valve\Steam`, through `reg query` (no registry crate needed).
fn registry_steam_path() -> Option<PathBuf> {
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_reg_value(&String::from_utf8_lossy(&out.stdout), "SteamPath").map(PathBuf::from)
}

/// The data of `name` in the output of `reg query` (`    SteamPath    REG_SZ    c:/program files (x86)/steam`).
pub fn parse_reg_value(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l.strip_prefix(name)?;
        let rest = rest.trim_start();
        let (_kind, data) = rest.split_once(char::is_whitespace)?;
        let data = data.trim();
        (!data.is_empty()).then(|| data.to_string())
    })
}

/// The library folders a `libraryfolders.vdf` lists (`"path"  "D:\\SteamLibrary"`), unescaped.
pub fn parse_library_folders(vdf: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in vdf.lines() {
        let mut quoted = Vec::new();
        let mut cur: Option<String> = None;
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            match (c, cur.as_mut()) {
                ('"', None) => cur = Some(String::new()),
                ('"', Some(_)) => quoted.push(cur.take().unwrap_or_default()),
                ('\\', Some(s)) => {
                    if let Some(n) = chars.next() {
                        s.push(n);
                    }
                }
                (c, Some(s)) => s.push(c),
                _ => {}
            }
        }
        if let [key, value] = quoted.as_slice()
            && key.eq_ignore_ascii_case("path")
        {
            out.push(PathBuf::from(value));
        }
    }
    out
}

/// Every Steam library of `root`: the root itself, then those of its `libraryfolders.vdf`.
pub fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    for vdf in [
        root.join("steamapps/libraryfolders.vdf"),
        root.join("config/libraryfolders.vdf"),
    ] {
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            for p in parse_library_folders(&text) {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// The game's install folder, if Steam has it on this machine.
pub fn find_game_dir() -> Option<PathBuf> {
    steam_roots()
        .iter()
        .flat_map(|r| libraries(r))
        .map(|lib| lib.join("steamapps/common").join(GAME_FOLDER))
        .find(|d| is_game_dir(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reg_query_output_reads() {
        let text = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\r\n    SteamPath    REG_SZ    c:/program files (x86)/steam\r\n\r\n";
        assert_eq!(
            parse_reg_value(text, "SteamPath").as_deref(),
            Some("c:/program files (x86)/steam")
        );
        assert_eq!(parse_reg_value(text, "SteamExe"), None);
    }

    #[test]
    fn library_folders_read() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"8930\"\t\t\"123\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/home/user/games/SteamLibrary\"\n\t}\n}\n";
        assert_eq!(
            parse_library_folders(vdf),
            [
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from("/home/user/games/SteamLibrary")
            ]
        );
    }

    #[test]
    fn a_library_with_the_game_is_found() {
        let root = std::env::temp_dir().join(format!("civ5-steam-test-{}", std::process::id()));
        let lib = root.join("lib2");
        let game = lib.join("steamapps/common").join(GAME_FOLDER);
        std::fs::create_dir_all(game.join("Assets")).unwrap();
        std::fs::create_dir_all(game.join("Resource")).unwrap();
        std::fs::create_dir_all(root.join("steam/steamapps")).unwrap();
        let escaped = lib.to_string_lossy().replace('\\', "\\\\");
        // One key/value pair per line, as Steam writes it.
        std::fs::write(
            root.join("steam/steamapps/libraryfolders.vdf"),
            format!("\"libraryfolders\"\n{{\n\"1\"\n{{\n\"path\" \"{escaped}\"\n}}\n}}\n"),
        )
        .unwrap();
        let libs = libraries(&root.join("steam"));
        assert_eq!(libs.len(), 2, "{libs:?}");
        let found = libs
            .iter()
            .map(|l| l.join("steamapps/common").join(GAME_FOLDER))
            .find(|d| is_game_dir(d));
        assert_eq!(found, Some(game));
        let _ = std::fs::remove_dir_all(&root);
    }
}
