//! The mods of a new game (T15): the host's picks are resolved once, at `Start`, and the game keeps the frozen
//! set. A game without picks is untouched (same settings, no lock), so nothing about it changes.

use mod_host_server::ModStore;
use protocol::{GameSetup, ModPick, ModPlan};

/// Freezes `setup.settings.mods` against the installed mods. On success the settings list the resolved mods
/// pinned to their versions (dependencies included, in load order) and the `mods.lock` text is returned; it goes
/// to `meta.json` and later to the `MODS` section of the saves. No picks: `Ok(None)`, `setup` unchanged.
///
/// # Errors
/// The picks cannot be resolved (a mod is not installed, a conflict, a version out of range): the sentence for
/// the host, with the chain.
pub fn freeze(store: &ModStore, setup: &mut GameSetup) -> Result<Option<String>, String> {
    if setup.settings.mods.is_empty() {
        return Ok(None);
    }
    let frozen = store
        .freeze(&setup.settings.mods)
        .map_err(|e| e.to_string())?;
    if frozen.set.is_empty() {
        return Ok(None);
    }
    let conflicts = store.ui_conflicts(&frozen.set);
    if !conflicts.is_empty() {
        let text: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
        return Err(text.join("; "));
    }
    setup.settings.mods = frozen.exact_picks();
    Ok(Some(frozen.lock_text()))
}

/// The plan of a room (T43): the picks of the host resolved against the installed mods. `Ok` lists every mod of
/// the set in load order (dependencies the host did not pick are added, `client_required: false` marks the mods
/// players may lack, `pack` the asset pack to fetch); `Err` carries the sentence with the dependency chain, or one
/// sentence per UI point two mods both replace (mod API 1.3), naming both mods. No picks: the empty plan, as before
/// mods existed.
pub fn plan(store: &ModStore, picks: &[ModPick]) -> ModPlan {
    if picks.is_empty() {
        return ModPlan::default();
    }
    match store.freeze(picks) {
        Ok(frozen) => {
            let ui = store.ui_problems(&frozen.set);
            if ui.is_empty() {
                ModPlan::Ok(store.set_view(&frozen.set).mods)
            } else {
                ModPlan::Err(ui)
            }
        }
        Err(e) => ModPlan::Err(e.problems()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use protocol::{AutoChoices, GameSettings, ModPick, TimerMode, TurnMode};

    use super::*;

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("c5-gamemods-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn install(&self, id: &str, extra: &str) {
            let dir: &Path = &self.0;
            std::fs::create_dir_all(dir.join(id)).unwrap();
            std::fs::write(
                dir.join(id).join("mod.toml"),
                format!(
                    "[mod]\nid=\"{id}\"\nname=\"{id}\"\nversion=\"1.0.0\"\napi=\"^1\"\nlicense=\"MIT\"\nsync=\"gameplay\"\nparts=[\"data\"]\n{extra}\n"
                ),
            )
            .unwrap();
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(picks: &[(&str, &str)]) -> GameSetup {
        GameSetup {
            settings: GameSettings {
                name: "g".into(),
                world_size: "DUEL".into(),
                map_script: "m".into(),
                map_options: vec![],
                speed: "s".into(),
                start_era: "e".into(),
                handicap: "h".into(),
                minor_civs: 0,
                turn_mode: TurnMode::Hybrid,
                timer: TimerMode::Off,
                game_options: vec![],
                seed: Some(1),
                map_seed: None,
                auto_choices: AutoChoices::default(),
                diplo_auto_decline: true,
                mods: picks
                    .iter()
                    .map(|(id, req)| ModPick {
                        id: (*id).into(),
                        req: (*req).into(),
                    })
                    .collect(),
            },
            slots: vec![],
        }
    }

    #[test]
    fn no_picks_changes_nothing() {
        let mut s = setup(&[]);
        let before = s.clone();
        assert_eq!(freeze(&ModStore::empty(), &mut s), Ok(None));
        assert_eq!(s, before);
    }

    #[test]
    fn picks_are_frozen_to_exact_versions_with_a_lock() {
        let t = Tmp::new("ok");
        t.install("base", "");
        t.install("addon", "[deps]\nbase = \"*\"");
        let store = ModStore::scan(&t.0);
        let mut s = setup(&[("addon", "^1")]);
        let lock = freeze(&store, &mut s).unwrap().unwrap();
        let picks: Vec<(&str, &str)> = s
            .settings
            .mods
            .iter()
            .map(|p| (p.id.as_str(), p.req.as_str()))
            .collect();
        assert_eq!(picks, [("base", "=1.0.0"), ("addon", "=1.0.0")]);
        // The game comes back from the lock while the mods are unchanged...
        let n = s.settings.mods.len();
        assert!(store.restore_set(Some(&lock), n).is_ok());
        // ...and freezing the frozen picks again is a fixed point.
        let mut again = s.clone();
        assert_eq!(freeze(&store, &mut again).unwrap().as_deref(), Some(&*lock));
        assert_eq!(again, s);
    }

    #[test]
    fn a_missing_mod_refuses_the_start_with_a_sentence() {
        let t = Tmp::new("missing");
        t.install("base", "");
        let store = ModStore::scan(&t.0);
        let mut s = setup(&[("ghost", "*")]);
        let before = s.clone();
        let e = freeze(&store, &mut s).unwrap_err();
        assert!(e.contains("ghost") && e.contains("not installed"), "{e}");
        assert_eq!(s, before, "a refused start leaves the settings alone");
        // A server with no mods directory refuses any pick the same way.
        assert!(freeze(&ModStore::empty(), &mut setup(&[("base", "*")])).is_err());
    }

    /// A client-only mod with one panel at `point` in `mode`.
    fn ui_mod(t: &Tmp, id: &str, point: &str, mode: &str) {
        let dir = t.0.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mod.toml"),
            format!(
                "[mod]\nid=\"{id}\"\nname=\"{id}\"\nversion=\"1.0.0\"\napi=\"^1.3\"\nlicense=\"MIT\"\nsync=\"client\"\nparts=[\"ui\"]\n\
                 [ui]\napi=\"^1\"\n[[ui.panels]]\nid=\"p\"\nsource=\"ui/p.html\"\npoint=\"{point}\"\nmode=\"{mode}\"\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn a_panel_at_an_unknown_point_keeps_the_mod_out_of_the_store() {
        let t = Tmp::new("uipoint");
        ui_mod(&t, "lost", "hud.nowhere", "append");
        ui_mod(&t, "stack", "hud.right", "replace");
        ui_mod(&t, "fine", "hud.right", "append");
        let store = ModStore::scan(&t.0);
        let ids: Vec<&str> = store
            .installed()
            .iter()
            .map(|i| i.manifest.id.as_str())
            .collect();
        assert_eq!(ids, ["fine"]);
        let texts: Vec<(&str, &str)> = store
            .problems()
            .iter()
            .map(|p| (p.dir.as_str(), p.message.as_str()))
            .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(
            texts[0],
            (
                "lost",
                "ui.panels[0].point: unknown UI point \"hud.nowhere\""
            )
        );
        assert!(
            texts[1].0 == "stack" && texts[1].1.contains("cannot be replaced"),
            "{texts:?}"
        );
    }

    #[test]
    fn two_mods_replacing_one_point_cannot_be_planned_or_frozen() {
        let t = Tmp::new("uiconflict");
        ui_mod(&t, "map-a", "hud.minimap", "replace");
        ui_mod(&t, "map-b", "hud.minimap", "replace");
        ui_mod(&t, "side", "hud.right", "append");
        let store = ModStore::scan(&t.0);
        let pick = |id: &str| ModPick {
            id: id.into(),
            req: "*".into(),
        };
        // One replacer and an append: a plan.
        assert!(matches!(
            plan(&store, &[pick("map-a"), pick("side")]),
            ModPlan::Ok(v) if v.len() == 2
        ));
        let ModPlan::Err(problems) = plan(&store, &[pick("map-a"), pick("map-b")]) else {
            panic!("a conflict must not be a plan");
        };
        assert_eq!(problems.len(), 1);
        assert_eq!(
            problems[0].text,
            "mods `map-a` and `map-b` both replace the interface element `hud.minimap`; keep one of them"
        );
        let e = freeze(&store, &mut setup(&[("map-a", "*"), ("map-b", "*")])).unwrap_err();
        assert!(e.contains("map-a") && e.contains("map-b"), "{e}");
    }
}
