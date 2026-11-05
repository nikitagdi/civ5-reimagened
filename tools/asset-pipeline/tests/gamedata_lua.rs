//! game-data, Lua part: include scan, closure resolution, the VFS index, the manifest and the
//! folder export, on tiny synthetic installs made by the tests. The real install is used only by
//! `real_install_*`, when CIV5_ROOT and CIV5_DB_SNAPSHOT are set.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{LogicalId, Manifest};
use asset_pipeline::gamedata::lua_closure::{
    self, ClosureSpec, RootDef, check_civhost_log, normalize_path, scan, stem_of,
};
use asset_pipeline::gamedata::lua_override::LuaOverride;
use asset_pipeline::gamedata::vfs_index::{INDEX_FILE, INDEX_ID, VfsIndex, lua_id};
use asset_pipeline::gamedata::{
    self, Db, GAMEPLAY_DB_FILE, GAMEPLAY_DB_ID, GameDataError, LOC_DB_FILE, LOC_EN_US_ID, Lock,
    sha256_hex,
};
use asset_pipeline::store::{BlobStore, ManifestBuilder};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-gamedata-lua-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A small install with the b1 layout: the Lekmap Pangaea script, Lekmod contexts, and name
/// conflicts between roots.
fn mini_game(game: &Path) {
    put(
        game,
        "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua",
        "include(\"HBMapGenerator\")\ninclude \"MapmakerUtilities\"\n",
    );
    put(
        game,
        "Assets/Maps/Lekmap v5.2/HBMapGenerator.lua",
        "include('Shared') -- found in Gameplay/Lua before Maps\n",
    );
    put(
        game,
        "Assets/Gameplay/Lua/MapmakerUtilities.lua",
        "-- utils\n",
    );
    put(
        game,
        "Assets/Gameplay/Lua/Shared.lua",
        "-- gameplay shared\n",
    );
    put(
        game,
        "Assets/Gameplay/Lua/sub/Shared.lua",
        "-- subfolder loses\n",
    );
    put(game, "Assets/Maps/Shared.lua", "-- maps shared loses\n");
    put(game, "Assets/Maps/Unused.lua", "-- not included\n");
    put(
        game,
        "Assets/DLC/LEKMOD_v34.15/Lua/UI/InGame.lua",
        "ContextPtr:LoadNewContext(\"CtxA\")\n-- ContextPtr:LoadNewContext(\"Off\")\n\
         ContextPtr:LoadNewContext(computed)\nContextPtr:LoadNewContext('CtxB')\n",
    );
    put(
        game,
        "Assets/DLC/LEKMOD_v34.15/Lua/CtxA.lua",
        "include(\"InstanceManager\")\ninclude(\"PlotAreaSweepIterator\")\n",
    );
    put(
        game,
        "Assets/DLC/LEKMOD_v34.15/Lua/Civilizations/CtxB.lua",
        "include(\"Shared\")\n",
    );
    put(
        game,
        "Assets/UI/InstanceManager.lua",
        "-- instance manager\n",
    );
}

fn snapshot(dir: &Path) -> (PathBuf, PathBuf) {
    std::fs::create_dir_all(dir).unwrap();
    let (g, l) = (dir.join(GAMEPLAY_DB_FILE), dir.join(LOC_DB_FILE));
    Db::create(&g)
        .unwrap()
        .exec("CREATE TABLE Units(Type TEXT); INSERT INTO Units VALUES ('UNIT_WARRIOR');")
        .unwrap();
    Db::create(&l)
        .unwrap()
        .exec(
            "CREATE TABLE LocalizedText(Language TEXT, Tag TEXT, Text TEXT, Gender TEXT, Plurality TEXT);
             INSERT INTO LocalizedText VALUES ('en_US','TXT_A','Ant','',''),('ru_RU','TXT_A','Mur','','');",
        )
        .unwrap();
    (g, l)
}

fn write_lock(path: &Path, g: &Path, l: &Path) {
    std::fs::write(
        path,
        format!(
            "gameplay_db_sha256 = {}\nloc_sha256 = {}\n",
            sha256_hex(&std::fs::read(g).unwrap()),
            sha256_hex(&std::fs::read(l).unwrap())
        ),
    )
    .unwrap();
}

struct Fixture {
    _tmp: TempDir,
    game: PathBuf,
    snapshot: PathBuf,
    lock: PathBuf,
    out: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new();
    let game = tmp.path().join("game");
    mini_game(&game);
    let snap = tmp.path().join("snap");
    let (g, l) = snapshot(&snap);
    let lock = tmp.path().join("test.lock");
    write_lock(&lock, &g, &l);
    Fixture {
        game,
        snapshot: snap,
        lock,
        out: tmp.path().join("out"),
        _tmp: tmp,
    }
}

fn manifest_of(out: &Path) -> Manifest {
    let store = BlobStore::open(out).unwrap();
    ManifestBuilder::open(&store).unwrap().manifest().clone()
}

fn has(m: &Manifest, id: &str) -> bool {
    !m.variants(&LogicalId::new(id)).is_empty()
}

// ---- scan ----------------------------------------------------------------------------------

#[test]
fn scan_finds_literal_includes_and_contexts_and_skips_comments_and_computed_names() {
    let src = br#"
include("A")
include "B"
include( 'C.lua' )
-- include("Commented")
--[[ include("InBlock")
include("InBlock2") ]]
--[==[ include("InLong") ]==]
local s = [[ include("InString") ]]
local t = "include('InQuoted')"
include("Dyn" .. suffix)
include(name)
x:include("Method")
x.include("Member")
local include2 = 1
ContextPtr:LoadNewContext("Ctx1")
-- ContextPtr:LoadNewContext("Off")
ContextPtr:LoadNewContext(variable)
  ContextPtr:LoadNewContext('Ctx2')
"#;
    let s = scan(src);
    assert_eq!(s.includes, ["A", "B", "C.lua"]);
    assert_eq!(s.contexts, ["Ctx1", "Ctx2"]);
}

#[test]
fn paths_are_normalised_by_one_function() {
    assert_eq!(
        normalize_path(".\\Assets\\DLC/LEKMOD_v34.15//Lua/"),
        "assets/dlc/lekmod_v34.15/lua"
    );
    assert_eq!(stem_of("UI\\InstanceManager.LUA"), "instancemanager");
    assert_eq!(stem_of("FLuaVector"), "fluavector");
}

// ---- closure -------------------------------------------------------------------------------

fn closure_of(game: &Path) -> lua_closure::Closure {
    lua_closure::compute(game, &ClosureSpec::lekmod_b1(game).unwrap()).unwrap()
}

#[test]
fn closure_resolves_by_stem_with_root_order_and_files_before_subfolders() {
    let f = fixture();
    let c = closure_of(&f.game);
    let paths: Vec<&str> = c.files.keys().map(String::as_str).collect();
    assert_eq!(
        paths,
        [
            "assets/dlc/lekmod_v34.15/lua/civilizations/ctxb.lua",
            "assets/dlc/lekmod_v34.15/lua/ctxa.lua",
            "assets/dlc/lekmod_v34.15/lua/ui/ingame.lua",
            // Gameplay/Lua comes before Maps, and its own file before its subfolder.
            "assets/gameplay/lua/mapmakerutilities.lua",
            "assets/gameplay/lua/shared.lua",
            "assets/maps/lekmap v5.2/hbmapgenerator.lua",
            "assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua",
            "assets/ui/instancemanager.lua",
        ]
    );
    assert_eq!(
        c.contexts,
        [
            "assets/dlc/lekmod_v34.15/lua/ctxa.lua",
            "assets/dlc/lekmod_v34.15/lua/civilizations/ctxb.lua"
        ]
    );
    assert_eq!(
        c.context_entry,
        "assets/dlc/lekmod_v34.15/lua/ui/ingame.lua"
    );
    assert_eq!(
        c.map_scripts["lekmap_pangaea"],
        "assets/maps/lekmap v5.2/lekmappangaeafractalv5.2.lua"
    );
    assert_eq!(
        c.known_missing.iter().collect::<Vec<_>>(),
        ["plotareasweepiterator.lua"]
    );
    assert_eq!(c.include_roots[0], "assets/maps/lekmap v5.2");
    assert_eq!(c.include_roots.len(), 9);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

#[test]
fn script_folder_wins_over_the_other_roots() {
    let f = fixture();
    put(
        &f.game,
        "Assets/Maps/Lekmap v5.2/MapmakerUtilities.lua",
        "-- local copy\n",
    );
    let c = closure_of(&f.game);
    assert!(
        c.files
            .contains_key("assets/maps/lekmap v5.2/mapmakerutilities.lua")
    );
    assert!(
        !c.files
            .contains_key("assets/gameplay/lua/mapmakerutilities.lua")
    );
}

#[test]
fn the_same_stem_in_two_roots_goes_to_the_earlier_root_of_each_side() {
    let f = fixture();
    // Map side: Gameplay/Lua before Maps. Context side: the Lekmod DLC and UI before Gameplay.
    put(&f.game, "Assets/UI/Shared.lua", "-- ui shared\n");
    let c = closure_of(&f.game);
    assert!(c.files.contains_key("assets/gameplay/lua/shared.lua"));
    assert!(c.files.contains_key("assets/ui/shared.lua"));
    // The index order (Gameplay first) resolves the context include differently from civhost.
    assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
    assert!(
        c.warnings[0].contains("assets/ui/shared.lua"),
        "{:?}",
        c.warnings
    );
}

#[test]
fn unknown_unresolved_include_is_an_error_naming_the_file() {
    let f = fixture();
    put(
        &f.game,
        "Assets/Gameplay/Lua/MapmakerUtilities.lua",
        "include(\"NoSuchLib\")\n",
    );
    let spec = ClosureSpec::lekmod_b1(&f.game).unwrap();
    let err = lua_closure::compute(&f.game, &spec)
        .unwrap_err()
        .to_string();
    assert!(err.contains("NoSuchLib"), "{err}");
    assert!(
        err.contains("assets/gameplay/lua/mapmakerutilities.lua"),
        "{err}"
    );
    assert!(!err.contains("PlotAreaSweepIterator"), "{err}");
}

#[test]
fn missing_map_script_and_missing_context_are_errors() {
    let f = fixture();
    std::fs::remove_file(f.game.join("Assets/Maps/Lekmap v5.2/HBMapGenerator.lua")).unwrap();
    let spec = ClosureSpec::lekmod_b1(&f.game).unwrap();
    assert!(lua_closure::compute(&f.game, &spec).is_err());

    let f = fixture();
    std::fs::remove_file(f.game.join("Assets/DLC/LEKMOD_v34.15/Lua/CtxA.lua")).unwrap();
    let spec = ClosureSpec::lekmod_b1(&f.game).unwrap();
    let err = lua_closure::compute(&f.game, &spec)
        .unwrap_err()
        .to_string();
    assert!(err.contains("CtxA"), "{err}");

    let f = fixture();
    let mut spec = ClosureSpec::lekmod_b1(&f.game).unwrap();
    spec.map_scripts[0].1 = "Assets/Maps/Nope.lua".into();
    assert!(lua_closure::compute(&f.game, &spec).is_err());
}

#[test]
fn closure_follows_includes_transitively_and_ignores_cycles() {
    let f = fixture();
    put(
        &f.game,
        "Assets/Gameplay/Lua/MapmakerUtilities.lua",
        "include(\"Shared\")\n",
    );
    put(
        &f.game,
        "Assets/Gameplay/Lua/Shared.lua",
        "include(\"MapmakerUtilities\")\n",
    );
    let c = closure_of(&f.game);
    assert!(c.files.contains_key("assets/gameplay/lua/shared.lua"));
}

#[test]
fn custom_spec_roots_are_searched_in_order() {
    let tmp = TempDir::new();
    let g = tmp.path();
    put(g, "a/x.lua", "-- a\n");
    put(g, "b/x.lua", "-- b\n");
    put(g, "b/y.lua", "include(\"x\")\n");
    put(g, "main/entry.lua", "include(\"y\")\n");
    put(g, "ctx/InGame.lua", "\n");
    let spec = |order: &[&str]| ClosureSpec {
        map_scripts: vec![("m".into(), "main/entry.lua".into())],
        map_roots: order.iter().map(|d| RootDef::recursive(*d)).collect(),
        context_entry: "InGame".into(),
        context_roots: vec![RootDef::recursive("ctx")],
        index_roots: order.iter().map(|d| RootDef::recursive(*d)).collect(),
        known_missing: vec![],
        extra_entries: vec![],
        default_script: None,
    };
    let ab = lua_closure::compute(g, &spec(&["a", "b"])).unwrap();
    assert!(ab.files.contains_key("a/x.lua") && !ab.files.contains_key("b/x.lua"));
    let ba = lua_closure::compute(g, &spec(&["b", "a"])).unwrap();
    assert!(ba.files.contains_key("b/x.lua") && !ba.files.contains_key("a/x.lua"));
}

// ---- civhost log ---------------------------------------------------------------------------

const LOG: &str = "lua: LoadFile Assets\\Maps\\Lekmap v5.2\\LekmapPangaeaFractalv5.2.lua -> D:\\G\\Assets\\Maps\\Lekmap v5.2\\LekmapPangaeaFractalv5.2.lua\n\
lua: WARNING include: file not found: PlotAreaSweepIterator.lua\n\
lua: 2/2 gameplay context(s) loaded from D:\\G\\Assets\\DLC\\LEKMOD_v34.15\\Lua\\UI\\InGame.lua (1.0 ms, 0 Lua error(s))\n";

#[test]
fn civhost_log_agreement_and_disagreements() {
    let f = fixture();
    let c = closure_of(&f.game);
    assert_eq!(check_civhost_log(LOG, &c), Vec::<String>::new());

    let w = check_civhost_log(&LOG.replace("2/2", "3/3"), &c);
    assert!(w.iter().any(|m| m.contains("3 gameplay contexts")), "{w:?}");
    let w = check_civhost_log(&LOG.replace("PlotAreaSweepIterator", "Other"), &c);
    assert!(w.iter().any(|m| m.contains("other.lua")), "{w:?}");
    assert!(
        w.iter().any(|m| m.contains("plotareasweepiterator.lua")),
        "{w:?}"
    );
    let w = check_civhost_log("nothing\n", &c);
    assert!(w.iter().any(|m| m.contains("no `N/N")), "{w:?}");
}

// ---- index and manifest --------------------------------------------------------------------

#[test]
fn package_puts_every_lua_file_the_index_and_both_dbs_into_one_manifest() {
    let f = fixture();
    let s = gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
    assert_eq!(s.lua.files, 8);
    let m = manifest_of(&f.out);
    assert!(has(&m, GAMEPLAY_DB_ID) && has(&m, LOC_EN_US_ID) && has(&m, INDEX_ID));

    let store = BlobStore::open(&f.out).unwrap();
    let v = &m.variants(&LogicalId::new(INDEX_ID))[0];
    let index = VfsIndex::parse(&std::fs::read(store.blob_path(&v.hash)).unwrap()).unwrap();
    index.check_against(&m).unwrap();
    assert_eq!(index.files.len(), 8);
    assert_eq!(index.snapshot.lekmod, "v34.15");
    assert_eq!(index.known_missing, ["plotareasweepiterator.lua"]);
    for file in &index.files {
        assert_eq!(file.id, lua_id(&file.path));
        let v = &m.variants(&LogicalId::new(&file.id))[0];
        assert_eq!(v.size, file.size);
        // The blob is the file as it is on disk.
        assert_eq!(
            std::fs::read(store.blob_path(&v.hash)).unwrap(),
            std::fs::read(f.game.join(&file.path)).unwrap_or_else(|_| {
                // The on-disk casing differs from the normalised path on case-sensitive systems.
                let c = closure_of(&f.game);
                std::fs::read(&c.files[&file.path].abs).unwrap()
            })
        );
    }
}

#[test]
fn package_publishes_the_replica_group_with_the_context_side_and_no_map_script() {
    use asset_format::Trigger;
    use asset_pipeline::world::groups;

    let f = fixture();
    gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
    let store = BlobStore::open(&f.out).unwrap();
    let mb = ManifestBuilder::open(&store).unwrap();
    let table = groups::load(&store, &mb).unwrap();
    let g = table.get(gamedata::REPLICA_GROUP).expect("the group");
    assert_eq!(g.trigger, Trigger::Preset(gamedata::REPLICA_TRIGGER.into()));
    let ids: Vec<&str> = g.members.iter().map(LogicalId::as_str).collect();
    for id in [GAMEPLAY_DB_ID, LOC_EN_US_ID, INDEX_ID] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    let c = closure_of(&f.game);
    // Every context and what the contexts include is in; the map scripts and what only they include are not.
    for p in c.contexts.iter().chain([&c.context_entry]) {
        assert!(ids.contains(&lua_id(p).as_str()), "{p} in {ids:?}");
    }
    assert!(
        ids.iter().any(|i| i.ends_with("/ui/instancemanager.lua")),
        "{ids:?}"
    );
    for p in c.map_scripts.values() {
        assert!(!ids.contains(&lua_id(p).as_str()), "{p} is a map script");
    }
    assert!(
        !ids.iter()
            .any(|i| i.contains("hbmapgenerator") || i.contains("mapmakerutilities")),
        "{ids:?}"
    );
    // The sum of the group is in the table, and every member is in the manifest.
    let m = manifest_of(&f.out);
    assert!(g.members.iter().all(|id| has(&m, id.as_str())));
    assert!(g.bytes > 0);
    assert_eq!(c.context_files.len() + 3, g.members.len());
}

#[test]
fn building_the_package_twice_gives_the_same_manifest() {
    let f = fixture();
    gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
    let first = std::fs::read_to_string(f.out.join("manifest")).unwrap();
    gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.out.join("manifest")).unwrap(),
        first
    );
}

#[test]
fn index_check_rejects_duplicate_paths_foreign_ids_and_unlisted_files() {
    let f = fixture();
    let lock = Lock::read(&f.lock).unwrap();
    let c = closure_of(&f.game);
    let good = VfsIndex::from_closure(&c, &lock, "v34.15");
    good.check().unwrap();
    // Deterministic JSON that parses back to the same index.
    assert_eq!(good.to_json().unwrap(), good.to_json().unwrap());
    assert_eq!(VfsIndex::parse(&good.to_json().unwrap()).unwrap(), good);

    let mut dup = good.clone();
    dup.files.push(dup.files[0].clone());
    assert!(
        dup.check()
            .unwrap_err()
            .to_string()
            .contains("duplicate path")
    );

    let mut foreign = good.clone();
    foreign.files[0].id = "lua:elsewhere".into();
    assert!(foreign.check().is_err());

    let mut unlisted = good.clone();
    unlisted.contexts.push("assets/nowhere.lua".into());
    assert!(unlisted.check().is_err());

    let mut m = Manifest::new();
    assert!(good.check_against(&m).is_err());
    m = manifest_of(&{
        gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
        f.out.clone()
    });
    good.check_against(&m).unwrap();
}

#[test]
fn snapshot_hash_mismatch_stores_nothing_and_names_the_file_and_the_pinned_hash() {
    let f = fixture();
    let pinned = Lock::read(&f.lock).unwrap().gameplay_db_sha256;
    let mut bytes = std::fs::read(f.snapshot.join(GAMEPLAY_DB_FILE)).unwrap();
    let n = bytes.len();
    bytes[n - 1] ^= 0xff;
    std::fs::write(f.snapshot.join(GAMEPLAY_DB_FILE), bytes).unwrap();

    let err = gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap_err();
    assert!(matches!(err, GameDataError::HashMismatch { .. }), "{err}");
    let text = err.to_string();
    assert!(
        text.contains(GAMEPLAY_DB_FILE) && text.contains(&pinned),
        "{text}"
    );
    assert!(!f.out.join("manifest").exists());
    assert!(manifest_of(&f.out).is_empty());

    let dir = f.out.join("export");
    let err = gamedata::export(&f.game, &f.snapshot, &f.lock, &dir, None).unwrap_err();
    assert!(matches!(err, GameDataError::HashMismatch { .. }), "{err}");
    assert!(!dir.exists());
}

#[test]
fn unresolved_include_stops_the_build_before_anything_is_stored() {
    let f = fixture();
    put(
        &f.game,
        "Assets/Gameplay/Lua/Shared.lua",
        "include(\"Missing1\")\n",
    );
    let err = gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap_err();
    assert!(err.to_string().contains("Missing1"), "{err}");
    assert!(manifest_of(&f.out).is_empty());
}

#[test]
fn package_keeps_entries_of_earlier_builds() {
    use asset_format::{PrefetchGroup, Variant, VariantFormat};
    let f = fixture();
    let store = BlobStore::open(&f.out).unwrap();
    let mut mb = ManifestBuilder::open(&store).unwrap();
    let (hash, size) = store.put(b"earlier").unwrap();
    mb.add(
        LogicalId::new("tex:earlier"),
        Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: PrefetchGroup(0),
        },
    );
    mb.finish(&store).unwrap();
    gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
    let m = manifest_of(&f.out);
    assert!(has(&m, "tex:earlier") && has(&m, INDEX_ID));
}

// ---- folder export -------------------------------------------------------------------------

#[test]
fn export_writes_index_dbs_and_lua_by_the_paths_of_the_index() {
    let f = fixture();
    let dir = f.out.join("fs");
    let (s, warnings) = gamedata::export(&f.game, &f.snapshot, &f.lock, &dir, None).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(s.lua_files, 8);
    assert_eq!(s.files, 11);
    assert_eq!(s.loc_rows, 1);

    let index = VfsIndex::parse(&std::fs::read(dir.join(INDEX_FILE)).unwrap()).unwrap();
    index.check().unwrap();
    for file in &index.files {
        assert_eq!(
            std::fs::metadata(dir.join(&file.path)).unwrap().len(),
            file.size
        );
    }
    assert_eq!(
        std::fs::read(dir.join(gamedata::EXPORT_GAMEPLAY_DB)).unwrap(),
        std::fs::read(f.snapshot.join(GAMEPLAY_DB_FILE)).unwrap()
    );
    let loc = Db::open_read_only(&dir.join(gamedata::EXPORT_LOC_DB)).unwrap();
    assert_eq!(
        loc.scalar_i64("SELECT count(*) FROM Language_en_US")
            .unwrap(),
        1
    );

    // The blobs of the manifest and the folder hold the same index.
    gamedata::build_package(&f.game, &f.snapshot, &f.lock, &f.out, None).unwrap();
    let store = BlobStore::open(&f.out).unwrap();
    let m = manifest_of(&f.out);
    let v = &m.variants(&LogicalId::new(INDEX_ID))[0];
    assert_eq!(
        std::fs::read(store.blob_path(&v.hash)).unwrap(),
        std::fs::read(dir.join(INDEX_FILE)).unwrap()
    );
}

#[test]
fn export_replaces_an_earlier_export_but_not_a_foreign_folder() {
    let f = fixture();
    let dir = f.out.join("fs");
    gamedata::export(&f.game, &f.snapshot, &f.lock, &dir, None).unwrap();
    put(&dir, "stale.txt", "old");
    gamedata::export(&f.game, &f.snapshot, &f.lock, &dir, None).unwrap();
    assert!(!dir.join("stale.txt").exists());

    let foreign = f.out.join("foreign");
    put(&foreign, "mine.txt", "keep");
    let err = gamedata::export(&f.game, &f.snapshot, &f.lock, &foreign, None).unwrap_err();
    assert!(err.to_string().contains("refusing"), "{err}");
    assert!(foreign.join("mine.txt").exists());
}

// ---- server package (AB-16) ----------------------------------------------------------------

/// The b1 mini install plus a second map script, the world-builder entry file with an include of
/// its own, two maps and a context that includes `Lekmod_policies`; the DB has `MapScripts`.
fn server_fixture() -> Fixture {
    let f = fixture();
    put(
        &f.game,
        "Assets/Maps/Second.lua",
        "include(\"Shared\")\ninclude(\"Other\")\n",
    );
    put(&f.game, "Assets/Maps/Other.lua", "-- only Second uses it\n");
    put(
        &f.game,
        "Assets/DLC/Expansion2/Gameplay/Lua/WorldBuilderRandomItems.lua",
        "include(\"Extra\")\n",
    );
    put(&f.game, "Assets/Gameplay/Lua/Extra.lua", "-- extra\n");
    put(&f.game, "Assets/Maps/A.Civ5Map", "map a");
    put(&f.game, "Tutorials/Maps/B.civ5map", "map b!");
    put(
        &f.game,
        "Assets/DLC/LEKMOD_v34.15/Lua/CtxA.lua",
        "include(\"InstanceManager\")\ninclude(\"PlotAreaSweepIterator\")\ninclude(\"Lekmod_policies\")\n",
    );
    put(
        &f.game,
        "Assets/DLC/LEKMOD_v34.15/Lua/Lekmod_policies.lua",
        "-- installed\n",
    );
    let gameplay = f.snapshot.join(GAMEPLAY_DB_FILE);
    Db::create(&gameplay)
        .unwrap()
        .exec(
            "CREATE TABLE MapScripts(FileName TEXT);
             INSERT INTO MapScripts VALUES ('Assets\\Maps\\Second.lua'),
               ('Assets\\Maps\\Lekmap v5.2\\LekmapPangaeaFractalv5.2.lua');",
        )
        .unwrap();
    write_lock(&f.lock, &gameplay, &f.snapshot.join(LOC_DB_FILE));
    f
}

fn server_opts(over: Vec<LuaOverride>) -> gamedata::ServerOptions<'static> {
    gamedata::ServerOptions {
        civhost_log: None,
        fix_0004: !over.is_empty(),
        overrides: over,
    }
}

#[test]
fn server_export_has_all_map_scripts_the_entry_closure_the_maps_and_a_valid_seal() {
    let f = server_fixture();
    let fix = f.out.join("repo/Lekmod_policies.lua");
    put(&f.out, "repo/Lekmod_policies.lua", "-- fixed, longer\n");
    let dir = f.out.join("srv");
    let opts = server_opts(vec![LuaOverride {
        file: "Lekmod_policies.lua".into(),
        source: fix.clone(),
    }]);
    let (s, warnings) =
        gamedata::export_server(&f.game, &f.snapshot, &f.lock, &dir, &opts).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!((s.map_scripts, s.maps), (2, 2));

    let index = VfsIndex::parse(&std::fs::read(dir.join(INDEX_FILE)).unwrap()).unwrap();
    index.check().unwrap();
    assert_eq!(
        index.map_scripts.keys().collect::<Vec<_>>(),
        ["lekmap_pangaea", "second"]
    );
    let paths: Vec<&str> = index.files.iter().map(|f| f.path.as_str()).collect();
    // The second script's own include, and the entry file with its include.
    for p in [
        "assets/maps/other.lua",
        "assets/dlc/expansion2/gameplay/lua/worldbuilderrandomitems.lua",
        "assets/gameplay/lua/extra.lua",
    ] {
        assert!(paths.contains(&p), "{p} missing from {paths:?}");
    }
    assert_eq!(
        index
            .civ5_maps
            .iter()
            .map(|m| (m.path.as_str(), m.size))
            .collect::<Vec<_>>(),
        [
            ("maps/Assets/Maps/A.Civ5Map", 5),
            ("maps/Tutorials/Maps/B.civ5map", 6)
        ]
    );
    assert_eq!(
        std::fs::read(dir.join("maps/Tutorials/Maps/B.civ5map")).unwrap(),
        b"map b!"
    );

    // FIX-0004: the replacement is in the file and in the index, and the lock says so.
    let policies = "assets/dlc/lekmod_v34.15/lua/lekmod_policies.lua";
    assert_eq!(
        std::fs::read(dir.join(policies)).unwrap(),
        b"-- fixed, longer\n"
    );
    let entry = index.files.iter().find(|f| f.path == policies).unwrap();
    assert_eq!(entry.size, 17);
    assert_eq!(s.applied.len(), 1);
    assert!(!s.applied[0].is_noop());
    let lock_text = std::fs::read_to_string(dir.join(gamedata::LOCK_FILE)).unwrap();
    assert!(lock_text.contains("lua_override = on"), "{lock_text}");
    assert!(
        lock_text.contains(&s.applied[0].replacement_sha256),
        "{lock_text}"
    );
    // The export's own lock still reads as a lock of the snapshot.
    let pinned = Lock::read(&dir.join(gamedata::LOCK_FILE)).unwrap();
    assert_eq!(
        pinned.gameplay_db_sha256,
        Lock::read(&f.lock).unwrap().gameplay_db_sha256
    );

    // The seal covers every file except itself, in `sha256sum` format.
    assert_eq!(gamedata::check_seal(&dir).unwrap(), s.sealed);
    let seal = std::fs::read_to_string(dir.join(gamedata::MANIFEST_FILE)).unwrap();
    assert!(seal.contains("  vfs-index.json\n") && seal.contains("  maps/Assets/Maps/A.Civ5Map\n"));
    assert!(!seal.contains(gamedata::MANIFEST_FILE));

    // A file added later (map-scripts.json) or changed: the check fails until the seal is
    // written again.
    put(&dir, "map-scripts.json", "[]");
    let err = gamedata::check_seal(&dir).unwrap_err().to_string();
    assert!(err.contains("map-scripts.json: not listed"), "{err}");
    gamedata::seal(&dir).unwrap();
    gamedata::check_seal(&dir).unwrap();
    put(&dir, "maps/Assets/Maps/A.Civ5Map", "tampered");
    let err = gamedata::check_seal(&dir).unwrap_err().to_string();
    assert!(err.contains("A.Civ5Map: changed"), "{err}");
}

#[test]
fn server_export_without_override_keeps_the_installed_file_and_says_off() {
    let f = server_fixture();
    let dir = f.out.join("srv");
    let (s, _) =
        gamedata::export_server(&f.game, &f.snapshot, &f.lock, &dir, &server_opts(vec![])).unwrap();
    assert!(s.applied.is_empty());
    assert_eq!(
        std::fs::read(dir.join("assets/dlc/lekmod_v34.15/lua/lekmod_policies.lua")).unwrap(),
        b"-- installed\n"
    );
    let lock_text = std::fs::read_to_string(dir.join(gamedata::LOCK_FILE)).unwrap();
    assert!(lock_text.contains("lua_override = off"), "{lock_text}");
    assert!(!lock_text.contains("lua_override_file"), "{lock_text}");
}

#[test]
fn an_override_that_matches_no_file_is_an_error_and_nothing_is_written() {
    let f = server_fixture();
    let dir = f.out.join("srv");
    let opts = server_opts(vec![LuaOverride {
        file: "NoSuchFile.lua".into(),
        source: f.game.join("Assets/Maps/Other.lua"),
    }]);
    let err = gamedata::export_server(&f.game, &f.snapshot, &f.lock, &dir, &opts)
        .unwrap_err()
        .to_string();
    assert!(err.contains("NoSuchFile.lua"), "{err}");
    assert!(!dir.exists());
}

#[test]
fn a_missing_entry_file_stops_the_server_export() {
    let f = server_fixture();
    std::fs::remove_file(
        f.game
            .join("Assets/DLC/Expansion2/Gameplay/Lua/WorldBuilderRandomItems.lua"),
    )
    .unwrap();
    let dir = f.out.join("srv");
    let err = gamedata::export_server(&f.game, &f.snapshot, &f.lock, &dir, &server_opts(vec![]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("WorldBuilderRandomItems"), "{err}");
}

// ---- real install --------------------------------------------------------------------------

/// The server package of the real install: 48 map scripts, 91 maps, the entry closure.
#[test]
fn real_install_server_package_has_48_map_scripts_the_maps_and_the_entry_closure() {
    let (Some(game), Some(snap)) = (
        std::env::var_os("CIV5_ROOT").map(PathBuf::from),
        std::env::var_os(gamedata::SNAPSHOT_ENV).map(PathBuf::from),
    ) else {
        eprintln!(
            "skipped: CIV5_ROOT and {} are not both set",
            gamedata::SNAPSHOT_ENV
        );
        return;
    };
    let tmp = TempDir::new();
    let dir = tmp.path().join("srv");
    let (s, warnings) = gamedata::export_server(
        &game,
        &snap,
        &gamedata::default_lock_path(),
        &dir,
        &server_opts(vec![]),
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(s.map_scripts, 48);
    assert_eq!(s.maps, 91);
    let index = VfsIndex::parse(&std::fs::read(dir.join(INDEX_FILE)).unwrap()).unwrap();
    index.check().unwrap();
    for p in [
        "assets/dlc/expansion2/gameplay/lua/worldbuilderrandomitems.lua",
        "assets/dlc/expansion2/gameplay/lua/mapgenerator.lua",
        "assets/dlc/expansion2/gameplay/lua/assignstartingplots.lua",
    ] {
        assert!(
            index.files.iter().any(|f| f.path == p),
            "{p} not in the index"
        );
    }
    for m in &index.civ5_maps {
        assert_eq!(std::fs::metadata(dir.join(&m.path)).unwrap().len(), m.size);
    }
    assert!(
        index
            .civ5_maps
            .iter()
            .any(|m| m.path == "maps/Assets/Maps/Earth_Duel.Civ5Map")
    );
    gamedata::check_seal(&dir).unwrap();
}

/// Real install and snapshot; runs only when CIV5_ROOT and CIV5_DB_SNAPSHOT are set. CIV5_CIVHOST_LOG
/// (optional) is a civhost log to compare with.
#[test]
fn real_install_closure_has_about_53_files_and_only_the_known_missing_include() {
    let (Some(game), Some(snap)) = (
        std::env::var_os("CIV5_ROOT").map(PathBuf::from),
        std::env::var_os(gamedata::SNAPSHOT_ENV).map(PathBuf::from),
    ) else {
        eprintln!(
            "skipped: CIV5_ROOT and {} are not both set",
            gamedata::SNAPSHOT_ENV
        );
        return;
    };
    let log = std::env::var_os("CIV5_CIVHOST_LOG").map(PathBuf::from);
    let lock = Lock::read(&gamedata::default_lock_path()).unwrap();
    let pkg = gamedata::prepare(&game, &lock, log.as_deref()).unwrap();
    let n = pkg.index.files.len();
    assert!((50..=56).contains(&n), "closure has {n} files");
    assert_eq!(pkg.index.known_missing, ["plotareasweepiterator.lua"]);
    assert_eq!(pkg.index.map_scripts.len(), 1);
    assert!(
        pkg.index.contexts.len() >= 30,
        "{} contexts",
        pkg.index.contexts.len()
    );
    assert!(pkg.warnings.is_empty(), "{:?}", pkg.warnings);
    assert!(pkg.lua_bytes() < 2_000_000);
    // The client's replica group: the context side without any map script.
    let ctx = &pkg.closure.context_files;
    assert!(pkg.closure.contexts.iter().all(|c| ctx.contains(c)));
    assert!(pkg.closure.map_scripts.values().all(|m| !ctx.contains(m)));
    assert!(
        ctx.len() >= pkg.index.contexts.len() && ctx.len() < n,
        "{} context files",
        ctx.len()
    );
    if let Some(path) = std::env::var_os("CIV5_REPLICA_FILES") {
        std::fs::write(
            path,
            ctx.iter().cloned().collect::<Vec<_>>().join(
                "
",
            ),
        )
        .unwrap();
    }

    let tmp = TempDir::new();
    let s = gamedata::build_package(
        &game,
        &snap,
        &gamedata::default_lock_path(),
        &tmp.path().join("out"),
        log.as_deref(),
    )
    .unwrap();
    assert_eq!(s.lua.files, n);
    assert_eq!(s.db.loc_rows, 30_012);
}
