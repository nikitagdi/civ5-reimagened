//! The coverage report on a small fixture (no game files): what is counted, what is not, how the names are
//! classified, the JSON, and the reader of a built set on a set made in a temporary folder.
use asset_format::{
    ContentHash, LogicalId, Manifest, PrefetchGroup, UI_INDEX_ID, UiFileInfo, UiFileKind, UiIndex,
    UiLayerInfo, Variant, VariantFormat,
};
use exe_api::coverage::{self, Contexts, Owner, Sources, Verdict};

const ROOT: &str = r#"
include("Helper")
include "NotInTheSet"
-- UI.CommentOnly()
--[[ UI.InBlockComment() ]]
local s = "UI.InString" .. [[UI.InLongString]]
local OptionsManager = OptionsManager
OptionsManager.GetGridOn()
OptionsManager.GetGridOn()
OptionsManager.NoSuchThing()
OptionsManager.SetGridOn_Cached(true)
Steam.SaveGameToCloud()
Modding.OpenUserData("Enhanced User Interface Options", 1)
local hex = ToHexFromGrid{ x = 1, y = 2 }
local ok = Mouse.eLClick + Keys.VK_ESCAPE
UI.NoSuchFunctionAtAll(1, 2)
Events.NoSuchEventAtAll.Add(function() end)
Game.GetActivePlayer()
Controls.B:SetText("x")
UIManager:QueuePopup(ContextPtr, PopupPriority.GenericPopup)
InStrategicView()
MapModData.UI_bc1 = {}
MapModData.Other = 1
Path.GetExtension("a.lua")
Color(1, 2, 3, 4)
FooBar.Baz()
"#;

const HELPER: &str = r#"
function Color(r, g, b, a) return { x = r, y = g, z = b, w = a } end
ContentManager.IsActive("x", ContentType.GAMEPLAY)
OptionsManager.NoSuchThing()
"#;

const TOP: &str = r#"
include("Helper")
OptionsManager.IsNoBasicHelp()
Locale.ConvertTextKey("TXT_KEY_X")
GetEverythingAtOnce()
"#;

fn sources() -> Sources {
    let mut s = Sources::new();
    s.add("Assets/UI/InGame/Root.lua", ROOT.as_bytes().to_vec());
    s.add("Assets/UI/Core/Helper.lua", HELPER.as_bytes().to_vec());
    s.add("Assets/UI/InGame/Top.lua", TOP.as_bytes().to_vec());
    s
}

fn contexts() -> Contexts {
    Contexts::parse("[K1]\nRoot\nXmlOnly\n[K2]\nTop\n").expect("contexts")
}

fn entry<'a>(r: &'a coverage::Report, name: &str) -> &'a coverage::Entry {
    r.entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} not in the report: {:?}",
                r.entries.iter().map(|e| &e.name).collect::<Vec<_>>()
            )
        })
}

#[test]
fn report_classifies_the_names_of_the_fixture() {
    let r = coverage::run(&sources(), &contexts());
    assert_eq!(r.files_scanned, 3);
    assert_eq!(r.no_lua, ["XmlOnly"]);
    assert!(
        r.unknown_includes.contains("NotInTheSet"),
        "{:?}",
        r.unknown_includes
    );

    // Comments and strings are not uses.
    for ghost in [
        "UI.CommentOnly",
        "UI.InBlockComment",
        "UI.InString",
        "UI.InLongString",
    ] {
        assert!(r.entries.iter().all(|e| e.name != ghost), "{ghost}");
    }

    let real = entry(&r, "OptionsManager.GetGridOn");
    assert_eq!(
        (real.owner, &real.verdict, real.uses),
        (Owner::ExeApi, &Verdict::Real, 2)
    );
    assert_eq!(
        entry(&r, "OptionsManager.NoSuchThing").verdict,
        Verdict::Missing
    );
    // Used in the root and in the helper both clusters reach: counted for K1 and K2, 3 times.
    let missing = entry(&r, "OptionsManager.NoSuchThing");
    assert_eq!(
        missing
            .clusters
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["K1", "K2"]
    );
    assert_eq!(missing.uses, 3);
    assert_eq!(
        entry(&r, "OptionsManager.SetGridOn_Cached").verdict,
        Verdict::Real
    );
    assert_eq!(
        entry(&r, "Steam.SaveGameToCloud").verdict,
        Verdict::Simp("SIMP-exe-12".into())
    );
    assert_eq!(entry(&r, "Modding.OpenUserData").verdict, Verdict::Real);
    assert_eq!(entry(&r, "ToHexFromGrid").verdict, Verdict::Real);
    assert_eq!(entry(&r, "InStrategicView").verdict, Verdict::Real);
    assert_eq!(entry(&r, "Mouse.eLClick").verdict, Verdict::Real);
    assert_eq!(entry(&r, "Keys.VK_ESCAPE").verdict, Verdict::Real);
    assert_eq!(entry(&r, "ContentType.GAMEPLAY").verdict, Verdict::Real);
    assert_eq!(entry(&r, "Locale.ConvertTextKey").verdict, Verdict::Real);
    assert_eq!(entry(&r, "Path.GetExtension").verdict, Verdict::Real);
    // Data tables count once, however many members the scripts set.
    assert_eq!(entry(&r, "MapModData").verdict, Verdict::Real);
    assert_eq!(entry(&r, "MapModData").uses, 2);
    // Names of the other modules are counted, not looked up.
    assert_eq!(entry(&r, "Controls.B").owner, Owner::Ui);
    assert_eq!(
        entry(&r, "UIManager:QueuePopup").verdict,
        Verdict::NotChecked
    );
    assert_eq!(entry(&r, "Game.GetActivePlayer").owner, Owner::Replica);
    // WO-4 names: looked up like the others.
    let ev = entry(&r, "Events.NoSuchEventAtAll");
    assert_eq!(ev.owner, Owner::Events);
    assert_eq!(
        entry(&r, "UI.NoSuchFunctionAtAll").verdict,
        Verdict::Missing
    );

    // `Color` is defined by the helper; `FooBar` and `GetEverythingAtOnce` by nobody.
    assert!(!r.unresolved.contains_key("Color"));
    assert!(
        r.unresolved.contains_key("FooBar.Baz"),
        "{:?}",
        r.unresolved
    );
    assert!(
        r.unresolved.contains_key("GetEverythingAtOnce"),
        "{:?}",
        r.unresolved
    );
}

#[test]
fn counts_and_json_follow_the_entries() {
    let r = coverage::run(&sources(), &contexts());
    // WO-5 names missing in K1: only OptionsManager.NoSuchThing. (UI/Events names are WO-4's.)
    let k1 = r.missing(&["K1"], &[Owner::ExeApi]);
    assert_eq!(
        k1.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        ["OptionsManager.NoSuchThing"]
    );
    let (real, simp, missing) = r.counts(&["K1"], &[Owner::ExeApi]);
    assert_eq!(simp, 1);
    assert_eq!(missing, 1);
    assert!(real >= 12, "{real}");

    let j = r.to_json();
    assert_eq!(j["version"], 1);
    assert_eq!(j["files_scanned"], 3);
    assert_eq!(j["contexts"]["K1"][0], "Root");
    assert_eq!(j["summary"]["per_cluster"]["K1"]["wo5"]["missing"], 1);
    assert_eq!(
        j["summary"]["per_cluster"]["K2"]["wo5"]["missing"], 1,
        "the helper's use reaches K2 too"
    );
    assert_eq!(j["summary"]["all"]["wo5"]["simp"], 1);
    let first = &j["entries"][0];
    for key in [
        "name", "owner", "status", "simp", "clusters", "uses", "files",
    ] {
        assert!(first.get(key).is_some(), "{key} in {first}");
    }
    assert_eq!(j["simp"][0]["id"], "SIMP-exe-12");
    assert!(j["unresolved_globals"]["FooBar.Baz"].is_number());
    let table = r.table();
    assert!(table.contains("OptionsManager.NoSuchThing"), "{table}");
    assert!(table.contains("SIMP-exe-12"), "{table}");
}

/// A built set in a temporary folder: the pointer, the manifest blob, the index and two Lua files.
#[test]
fn the_reader_loads_the_lua_of_a_built_set() {
    let dir = std::env::temp_dir().join(format!("exe-api-set-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let hash = |n: u8| ContentHash([n; 32]);
    let put = |h: ContentHash, bytes: &[u8]| {
        std::fs::write(dir.join(h.to_string()), bytes).expect("blob")
    };

    let layer = UiLayerInfo {
        name: "base".into(),
        dir: String::new(),
        priority: 0,
        order: 0,
        packages: vec![],
        priority_defaulted: false,
    };
    let file = |name: &str, kind: UiFileKind, id: &str, path: &str| UiFileInfo {
        name: name.into(),
        id: id.into(),
        kind,
        layer: 0,
        size: 1,
        path: path.into(),
        hidden: 0,
        index_of: None,
    };
    let index = UiIndex {
        layers: vec![layer],
        files: vec![
            file(
                "helper.lua",
                UiFileKind::Lua,
                "ui:f:helper.lua",
                "UI/Helper.lua",
            ),
            file("root.lua", UiFileKind::Lua, "ui:f:root.lua", "UI/Root.lua"),
            file("root.xml", UiFileKind::Xml, "ui:f:root.xml", "UI/Root.xml"),
        ],
        ..UiIndex::default()
    };
    let mut m = Manifest::new();
    let variant = |n: u8| Variant {
        format: VariantFormat::Generic,
        hash: hash(n),
        size: 1,
        prefetch: PrefetchGroup(0),
    };
    m.insert(LogicalId::new(UI_INDEX_ID), variant(2));
    m.insert(LogicalId::new("ui:f:helper.lua"), variant(3));
    m.insert(LogicalId::new("ui:f:root.lua"), variant(4));
    put(hash(1), &m.encode());
    put(hash(2), &index.encode());
    put(hash(3), HELPER.as_bytes());
    put(hash(4), ROOT.as_bytes());
    std::fs::write(dir.join("manifest"), format!("{}\n", hash(1))).expect("pointer");

    let (sources, loaded) = coverage::set::load_lua(&dir).expect("load");
    assert_eq!(sources.len(), 2, "the XML file is not Lua");
    assert_eq!(loaded.files.len(), 3);
    let r = coverage::run(
        &sources,
        &Contexts::parse("[K1]\nRoot\n").expect("contexts"),
    );
    assert_eq!(r.files_scanned, 2);
    assert_eq!(entry(&r, "OptionsManager.GetGridOn").verdict, Verdict::Real);
    std::fs::remove_dir_all(&dir).ok();

    assert!(coverage::set::load_lua(&dir).is_err(), "no set, no report");
}

/// The list in the crate names the three clusters of M2 and the core.
#[test]
fn the_m2_list_has_the_clusters_of_the_milestone() {
    let c = Contexts::parse(include_str!("../m2-contexts.txt")).expect("list");
    let names: Vec<&str> = c.clusters.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["K1", "K2", "K3", "core"]);
    assert!(c.names("K2").iter().any(|n| n == "TopPanel"));
    assert!(c.names("K3").iter().any(|n| n == "UnitPanel"));
    assert!(c.names("K1").iter().any(|n| n == "WorldView"));
}
