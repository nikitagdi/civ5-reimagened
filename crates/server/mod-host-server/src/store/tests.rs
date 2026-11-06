use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// A scratch directory removed on drop.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "c5-modstore-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
    fn write(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn manifest(id: &str, ver: &str, extra: &str) -> String {
    format!(
        "[mod]\nid=\"{id}\"\nname=\"{id} title\"\nversion=\"{ver}\"\napi=\"^1\"\nlicense=\"MIT\"\nsync=\"gameplay\"\nparts=[\"data\"]\n{extra}\n"
    )
}

fn install(t: &Tmp, id: &str, ver: &str, extra: &str) {
    t.write(&format!("{id}/mod.toml"), &manifest(id, ver, extra));
}

fn pick(id: &str, req: &str) -> ModPick {
    ModPick {
        id: id.into(),
        req: req.into(),
    }
}

#[test]
fn scan_missing_root_is_empty() {
    let t = Tmp::new();
    let s = ModStore::scan(&t.0.join("nope"));
    assert!(s.installed().is_empty() && s.problems().is_empty());
}

#[test]
fn scan_and_freeze_success_orders_by_dependency() {
    let t = Tmp::new();
    install(&t, "base", "1.2.0", "");
    install(&t, "addon", "0.3.0", "[deps]\nbase = \"^1\"");
    install(&t, "unused", "1.0.0", "");
    t.write("stray-file.txt", "not a mod");
    let s = ModStore::scan(&t.0);
    assert!(s.problems().is_empty(), "{:?}", s.problems());
    assert_eq!(s.installed().len(), 3);
    assert!(s.dir_of(&ModId::new("base").unwrap()).is_some());

    let f = s.freeze(&[pick("addon", "*")]).unwrap();
    let ids: Vec<&str> = f.set.mods().iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["base", "addon"]);
    let view = f.view();
    assert_eq!(view.mods.len(), 2);
    assert_eq!(view.mods[0].id, "base");
    assert_eq!(view.mods[0].version, "1.2.0");
    assert_eq!(view.mods[1].load_idx, 1);
    assert_eq!(view.mods[0].sync, ModSync::Gameplay);
    assert!(view.mods.iter().all(|m| m.available && m.client_required));
    assert_eq!(view.modset_hash, modset_hash64(&f.set.modset_hash()));

    // The same picks give the same frozen set.
    assert_eq!(f, s.freeze(&[pick("addon", "*")]).unwrap());
}

#[test]
fn freeze_empty_picks_is_empty_set() {
    let s = ModStore::empty();
    let f = s.freeze(&[]).unwrap();
    assert!(f.set.is_empty() && f.view().mods.is_empty());
}

#[test]
fn freeze_missing_mod_names_it() {
    let t = Tmp::new();
    install(&t, "base", "1.0.0", "");
    let s = ModStore::scan(&t.0);
    let e = s.freeze(&[pick("ghost", "*")]).unwrap_err();
    assert!(matches!(
        e,
        FreezeError::Resolve(ResolveError::Missing { .. })
    ));
    let text = e.to_string();
    assert!(
        text.contains("ghost") && text.contains("not installed"),
        "{text}"
    );
    let p = e.problems();
    assert_eq!(p.len(), 1);
    assert!(p[0].text.contains("ghost"));
}

#[test]
fn freeze_conflict_names_both() {
    let t = Tmp::new();
    install(&t, "one", "1.0.0", "[conflicts]\ntwo = \"*\"");
    install(&t, "two", "1.0.0", "");
    let s = ModStore::scan(&t.0);
    let e = s.freeze(&[pick("one", "*"), pick("two", "*")]).unwrap_err();
    assert!(matches!(e, FreezeError::Resolve(ResolveError::Conflict(_))));
    let text = e.to_string();
    assert!(text.contains("one") && text.contains("two"), "{text}");
}

#[test]
fn freeze_version_out_of_range() {
    let t = Tmp::new();
    install(&t, "base", "1.0.0", "");
    let s = ModStore::scan(&t.0);
    let e = s.freeze(&[pick("base", "^2")]).unwrap_err();
    assert!(matches!(
        e,
        FreezeError::Resolve(ResolveError::NoMatchingVersion { .. })
    ));
}

#[test]
fn freeze_rejects_bad_picks() {
    let s = ModStore::empty();
    assert!(matches!(
        s.freeze(&[pick("Bad Id", "*")]),
        Err(FreezeError::BadPick { index: 0, .. })
    ));
    assert!(matches!(
        s.freeze(&[pick("fine", "1.0")]),
        Err(FreezeError::BadPick { .. })
    ));
    assert!(matches!(
        s.freeze(&[pick("fine", "*"), pick("fine", "^1")]),
        Err(FreezeError::DuplicatePick(_))
    ));
    let many: Vec<ModPick> = (0..=MAX_MODS)
        .map(|i| pick(&format!("m{i:02}"), "*"))
        .collect();
    assert_eq!(
        s.freeze(&many),
        Err(FreezeError::TooManyPicks(MAX_MODS + 1))
    );
}

#[test]
fn problem_text_is_clamped() {
    let long = "x".repeat(1000);
    let e = FreezeError::DuplicatePick(long);
    let p = e.problems();
    assert_eq!(p[0].text.chars().count(), MAX_MOD_PROBLEM_CHARS);
    assert!(p[0].text.ends_with("..."));
}

#[test]
fn scan_reports_broken_directories_and_keeps_the_rest() {
    let t = Tmp::new();
    install(&t, "good", "1.0.0", "");
    t.write("nomanifest/readme.txt", "x");
    t.write("badtoml/mod.toml", "[mod]\nid = ");
    t.write("wrongname/mod.toml", &manifest("other", "1.0.0", ""));
    t.write(
        "unknown/mod.toml",
        &(manifest("unknown", "1.0.0", "") + "bogus = 1\n"),
    );
    let s = ModStore::scan(&t.0);
    assert_eq!(s.installed().len(), 1);
    let by_dir = |d: &str| s.problems().iter().find(|p| p.dir == d).unwrap();
    assert_eq!(by_dir("nomanifest").message, "no mod.toml");
    assert!(by_dir("wrongname").message.contains("does not match"));
    assert!(
        by_dir("unknown").message.contains("bogus"),
        "{:?}",
        by_dir("unknown")
    );
    assert_eq!(s.problems().len(), 4);
}

#[test]
fn content_hash_follows_content_not_timestamps() {
    let t = Tmp::new();
    install(&t, "m", "1.0.0", "");
    t.write("m/data/a.sql", "select 1;");
    t.write("m/data/b.sql", "select 2;");
    let h1 = hash_mod_dir(&t.0.join("m")).unwrap();
    assert_eq!(h1, hash_mod_dir(&t.0.join("m")).unwrap());

    // Touching a file with the same bytes changes nothing; hidden files are ignored.
    t.write("m/data/a.sql", "select 1;");
    t.write("m/.DS_Store", "junk");
    assert_eq!(h1, hash_mod_dir(&t.0.join("m")).unwrap());

    // Changed bytes, an added file, a renamed file: all change the hash.
    t.write("m/data/a.sql", "select 3;");
    let h2 = hash_mod_dir(&t.0.join("m")).unwrap();
    assert_ne!(h1, h2);
    t.write("m/data/c.sql", "");
    let h3 = hash_mod_dir(&t.0.join("m")).unwrap();
    assert_ne!(h2, h3);
    fs::rename(t.0.join("m/data/c.sql"), t.0.join("m/data/d.sql")).unwrap();
    assert_ne!(h3, hash_mod_dir(&t.0.join("m")).unwrap());
}

#[test]
fn content_change_changes_modset_hash() {
    let t = Tmp::new();
    install(&t, "mm", "1.0.0", "");
    let a = ModStore::scan(&t.0).freeze(&[pick("mm", "*")]).unwrap();
    t.write("mm/extra.txt", "changed");
    let b = ModStore::scan(&t.0).freeze(&[pick("mm", "*")]).unwrap();
    assert_ne!(a.view().modset_hash, b.view().modset_hash);
}

#[test]
fn lobby_views_describe_installed_mods() {
    let t = Tmp::new();
    install(&t, "hello", "0.1.0", "");
    let s = ModStore::scan(&t.0);
    let v = s.lobby_views();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].id, "hello");
    assert_eq!(v[0].title, "hello title");
    assert!(v[0].client_required);
}
