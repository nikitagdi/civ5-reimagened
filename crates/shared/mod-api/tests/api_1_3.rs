//! Mod API 1.3: the `ui` capability, the catalogue of UI extension points, and the version.

use std::collections::BTreeSet;

use mod_api::{
    Cap, Caps, CapsError, MOD_API_VERSION, MODS_POINT, PointKind, UI_CATALOG, check_ui_points,
};
use mod_manifest::{Manifest, Version, VersionReq};

fn ui_mod(parts: &str, caps: &str, panels: &str) -> Manifest {
    let rust = if parts.contains("client-rs") {
        format!(
            "[rust]\nclient = {{ crate = \"m-client\", entry = \"m_client::register\" }}\ncaps = [{caps}]\n"
        )
    } else {
        String::new()
    };
    Manifest::parse(&format!(
        "[mod]\nid = \"mm\"\nname = \"M\"\nversion = \"1.0.0\"\napi = \"^1.3\"\nlicense = \"MIT\"\nsync = \"client\"\n\
         parts = [{parts}]\n{rust}[ui]\napi = \"^1\"\n{panels}"
    ))
    .unwrap_or_else(|e| panic!("{e}"))
}

fn panel(point: &str, mode: &str) -> String {
    format!(
        "[[ui.panels]]\nid = \"p\"\nsource = \"ui/p.html\"\npoint = \"{point}\"\nmode = \"{mode}\"\n"
    )
}

#[test]
fn the_api_is_1_3_and_only_adds() {
    assert_eq!(MOD_API_VERSION, Version::new(1, 3, 0));
    for req in ["^1", "^1.2", "^1.3", ">=1.0, <2"] {
        assert!(
            req.parse::<VersionReq>().unwrap().matches(&MOD_API_VERSION),
            "{req}"
        );
    }
    assert!(
        !"^1.4"
            .parse::<VersionReq>()
            .unwrap()
            .matches(&MOD_API_VERSION)
    );
}

#[test]
fn the_ui_capability_is_client_only() {
    assert_eq!(Cap::parse("ui"), Some(Cap::Ui));
    assert_eq!(Cap::Ui.as_str(), "ui");
    assert_eq!(Cap::Ui.to_string(), "ui");
    assert!(Cap::Ui.client_only());
    assert!(!Cap::Ui.trusted_native());
    assert_eq!(Cap::ALL.len(), 13);
    assert!(Cap::ALL.contains(&Cap::Ui));
    // The older capabilities keep their names.
    let names: BTreeSet<&str> = Cap::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(names.len(), Cap::ALL.len());
    for old in ["net.channel", "hud", "kv", "world.share_terrain"] {
        assert!(names.contains(old), "{old}");
    }

    let m = ui_mod(
        "\"client-rs\", \"ui\"",
        "\"ui\", \"hud\"",
        &panel("hud.right", "append"),
    );
    let caps = Caps::from_manifest(&m).unwrap();
    assert!(caps.has(Cap::Ui));
    assert_eq!(caps.require(Cap::Ui), Ok(()));
    assert_eq!(Caps::none().require(Cap::Ui).unwrap_err().0, Cap::Ui);
}

#[test]
fn the_ui_capability_needs_a_client_half() {
    // A server-only cosmetic mod cannot hold it.
    let m = Manifest::parse(
        "[mod]\nid = \"ss\"\nname = \"S\"\nversion = \"1.0.0\"\napi = \"^1.3\"\nlicense = \"MIT\"\nsync = \"cosmetic\"\n\
         parts = [\"server-rs\"]\n[rust]\nserver = { crate = \"s\", entry = \"s::register\" }\ncaps = [\"ui\"]\n",
    )
    .unwrap();
    assert_eq!(Caps::from_manifest(&m), Err(CapsError::ClientOnly(Cap::Ui)));
}

#[test]
fn the_catalogue_has_kinds_mods_and_unique_ids() {
    assert_eq!(UI_CATALOG.version, Version::new(1, 0, 0));
    let ids: BTreeSet<&str> = UI_CATALOG.points.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), UI_CATALOG.points.len(), "point ids are unique");
    let mods = UI_CATALOG.point(MODS_POINT).expect("the shared slot");
    assert_eq!(mods.kind, PointKind::Stack);
    assert!(mods.required);
    assert_eq!(
        UI_CATALOG.points.iter().filter(|p| p.required).count(),
        1,
        "only `mods` is required of every skin"
    );
    for (id, kind, buttons) in [
        ("hud.top.extra", PointKind::Stack, false),
        ("hud.menu", PointKind::Stack, true),
        ("hud.right", PointKind::Stack, false),
        ("hud.unit.actions", PointKind::Stack, true),
        ("window.production.detail", PointKind::Stack, false),
        ("hud.minimap", PointKind::Single, false),
        ("hud.chat", PointKind::Single, false),
    ] {
        let p = UI_CATALOG.point(id).unwrap_or_else(|| panic!("{id}"));
        assert_eq!((p.kind, p.buttons), (kind, buttons), "{id}");
    }
    assert_eq!(UI_CATALOG.points.len(), 8);
    assert!(UI_CATALOG.point("hud.nowhere").is_none());
    // Every point id has the shape a manifest accepts.
    for p in UI_CATALOG.points {
        let m = ui_mod("\"ui\"", "", &panel(p.id, "append"));
        assert_eq!(m.ui.unwrap().panels[0].point, p.id);
    }
    // Tokens: colour synonyms only, unique, all `--ui-*`.
    let tokens: BTreeSet<&str> = UI_CATALOG.tokens.iter().copied().collect();
    assert_eq!(tokens.len(), UI_CATALOG.tokens.len());
    assert_eq!(tokens.len(), 17);
    assert!(UI_CATALOG.tokens.iter().all(|t| t.starts_with("--ui-")));
    assert!(UI_CATALOG.has_token("--ui-accent") && !UI_CATALOG.has_token("--ui-font"));
}

#[test]
fn panels_are_checked_against_the_catalogue() {
    let ok = |point: &str, mode: &str| {
        let m = ui_mod("\"ui\"", "", &panel(point, mode));
        check_ui_points(m.ui.as_ref().unwrap())
    };
    assert_eq!(ok("hud.right", "append"), Ok(()));
    assert_eq!(ok("mods", "append"), Ok(()));
    assert_eq!(ok("hud.minimap", "replace"), Ok(()));
    let e = ok("hud.nowhere", "append").unwrap_err();
    assert_eq!(
        e.to_string(),
        "ui.panels[0].point: unknown UI point \"hud.nowhere\""
    );
    let e = ok("hud.right", "replace").unwrap_err();
    assert_eq!(e.path(), Some("ui.panels[0].point"));
    assert!(e.to_string().contains("is a stack"), "{e}");
    let e = ok("mods", "replace").unwrap_err();
    assert!(e.to_string().contains("cannot be replaced"), "{e}");
}
