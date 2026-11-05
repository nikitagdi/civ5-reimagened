//! The map scripts of the lobby (game-host-mp 4.13, gc-abi-v2 4.16): the scripts of the game data index that
//! the smoke run `map-scripts.json` lists as passed. Without the report only the default script is offered.

use std::path::Path;

use protocol::LobbyChoice;
use serde::Deserialize;

/// Index key of the script every game can use (it passes the B1 smoke test).
pub const DEFAULT_SCRIPT: &str = "lekmap_pangaea";
/// Its path when the index does not list it.
const DEFAULT_PATH: &str = "assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua";

#[derive(Deserialize)]
struct Index {
    #[serde(default)]
    map_scripts: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct Report {
    #[serde(default)]
    passed: Vec<String>,
}

/// "lekmap_pangaea" -> "Lekmap Pangaea".
fn title(key: &str) -> String {
    key.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or_else(String::new, |f| {
                f.to_uppercase().chain(c).collect::<String>()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lobby choices from the bytes of `vfs-index.json` and, if there is one, `map-scripts.json`. The default
/// script comes first; `type_name` is the VFS path the core loads.
pub fn map_script_choices(index: &[u8], report: Option<&[u8]>) -> Vec<LobbyChoice> {
    let index: Index = serde_json::from_slice(index).unwrap_or(Index {
        map_scripts: Default::default(),
    });
    let path_of = |key: &str| index.map_scripts.get(key).cloned();
    let choice = |key: &str, path: String| LobbyChoice {
        type_name: path,
        name: title(key),
    };
    let default = choice(
        DEFAULT_SCRIPT,
        path_of(DEFAULT_SCRIPT).unwrap_or_else(|| DEFAULT_PATH.to_owned()),
    );
    let mut out = vec![default];
    let passed = report
        .and_then(|r| serde_json::from_slice::<Report>(r).ok())
        .map(|r| r.passed)
        .unwrap_or_default();
    for key in passed {
        if key == DEFAULT_SCRIPT {
            continue;
        }
        if let Some(path) = path_of(&key) {
            out.push(choice(&key, path));
        }
    }
    out
}

/// [`map_script_choices`] for the game data directory of the server.
pub fn map_script_choices_in(dir: &Path) -> Vec<LobbyChoice> {
    let index = std::fs::read(dir.join("vfs-index.json")).unwrap_or_default();
    let report = std::fs::read(dir.join("map-scripts.json")).ok();
    map_script_choices(&index, report.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"{"map_scripts":{"lekmap_pangaea":"assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua","continents":"assets/maps/continents.lua","oval":"assets/maps/oval.lua","bad":"assets/maps/bad.lua"}}"#;

    #[test]
    fn passed_scripts_follow_the_default() {
        let report = br#"{"passed":["continents","lekmap_pangaea","oval","unknown"],"failed":[{"name":"bad"}]}"#;
        let c = map_script_choices(INDEX.as_bytes(), Some(report));
        let names: Vec<_> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Lekmap Pangaea", "Continents", "Oval"]);
        assert_eq!(
            c[0].type_name,
            "assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua"
        );
    }

    #[test]
    fn no_report_offers_only_the_default() {
        let c = map_script_choices(INDEX.as_bytes(), None);
        assert_eq!(c.len(), 1);
        let c = map_script_choices(b"", None);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].type_name, DEFAULT_PATH);
    }
}
