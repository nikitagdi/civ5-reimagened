//! The resolver (D4): one test per rule and per kind of error, plus the permutation-invariance property.

use mod_manifest::{
    ContentHash, Installed, Manifest, ModId, ModSet, Pick, ResolveError, Version, VersionReq,
    Warning, resolve, write_lock,
};

const HOST_API: Version = Version::new(1, 0, 0);

/// An installed gameplay mod; `extra` is appended after the `[mod]` table (dependency tables and so on).
fn inst_api(id: &str, ver: &str, api: &str, extra: &str) -> Installed {
    let src = format!(
        "[mod]\nid=\"{id}\"\nname=\"{id}\"\nversion=\"{ver}\"\napi=\"{api}\"\nlicense=\"MIT\"\nsync=\"gameplay\"\nparts=[\"data\"]\n{extra}\n"
    );
    let manifest =
        Manifest::parse(&src).unwrap_or_else(|e| panic!("fixture {id} {ver}: {e}\n{src}"));
    let mut h = [0u8; 32];
    h[0] = id.bytes().next().unwrap();
    h[1] = ver
        .bytes()
        .fold(0u8, |a, b| a.wrapping_mul(31).wrapping_add(b));
    Installed {
        manifest,
        content_hash: ContentHash(h),
    }
}

fn inst(id: &str, ver: &str, extra: &str) -> Installed {
    inst_api(id, ver, "^1", extra)
}

fn pick(id: &str, req: &str) -> Pick {
    Pick {
        id: id.parse::<ModId>().unwrap(),
        req: req.parse::<VersionReq>().unwrap(),
    }
}

fn names(set: &ModSet) -> Vec<String> {
    set.mods()
        .iter()
        .map(|m| format!("{} {}", m.id, m.version))
        .collect()
}

fn ok(picks: &[Pick], installed: &[Installed]) -> Vec<String> {
    let r = resolve(picks, installed, &HOST_API).unwrap_or_else(|e| panic!("resolve failed: {e}"));
    names(&r.set)
}

fn err(picks: &[Pick], installed: &[Installed]) -> ResolveError {
    match resolve(picks, installed, &HOST_API) {
        Ok(r) => panic!("expected an error, got {:?}", names(&r.set)),
        Err(e) => e,
    }
}

#[test]
fn empty_request_gives_empty_set() {
    let r = resolve(&[], &[inst("aa", "1.0.0", "")], &HOST_API).unwrap();
    assert!(r.set.is_empty() && r.warnings.is_empty());
}

#[test]
fn highest_satisfying_version_wins() {
    let installed = [
        inst("aa", "1.0.0", ""),
        inst("aa", "1.4.2", ""),
        inst("aa", "2.0.0", ""),
        inst("aa", "0.9.0", ""),
    ];
    assert_eq!(ok(&[pick("aa", "^1")], &installed), ["aa 1.4.2"]);
    assert_eq!(ok(&[pick("aa", "*")], &installed), ["aa 2.0.0"]);
    assert_eq!(ok(&[pick("aa", "<1.4")], &installed), ["aa 1.0.0"]);
    assert_eq!(ok(&[pick("aa", "=0.9.0")], &installed), ["aa 0.9.0"]);
}

#[test]
fn hard_deps_are_added_transitively_and_loaded_first() {
    let installed = [
        inst("app", "1.0.0", "[deps]\nmid = \"^1\""),
        inst("mid", "1.2.0", "[deps]\nbase = \"^1\""),
        inst("base", "1.0.0", ""),
        inst("unrelated", "1.0.0", ""),
    ];
    let r = resolve(&[pick("app", "*")], &installed, &HOST_API).unwrap();
    assert_eq!(names(&r.set), ["base 1.0.0", "mid 1.2.0", "app 1.0.0"]);
    let idx: Vec<u16> = r.set.mods().iter().map(|m| m.load_idx).collect();
    assert_eq!(idx, [0, 1, 2]);
    assert!(r.warnings.is_empty());
}

#[test]
fn one_version_per_id_satisfying_every_requirement() {
    let installed = [
        inst("aa", "1.0.0", "[deps]\nlib = \"^1\""),
        inst("cc", "1.0.0", "[deps]\nlib = \">=1.2, <1.5\""),
        inst("lib", "1.1.0", ""),
        inst("lib", "1.3.0", ""),
        inst("lib", "1.4.0", ""),
        inst("lib", "1.9.0", ""),
    ];
    // ^1 allows 1.9.0, but c caps it at <1.5.
    assert_eq!(
        ok(&[pick("aa", "*"), pick("cc", "*")], &installed),
        ["lib 1.4.0", "aa 1.0.0", "cc 1.0.0"]
    );
}

#[test]
fn backtracking_falls_back_to_an_older_version() {
    // a 1.1.0 needs c ^2, but d needs c ^1: the search must settle on a 1.0.0 (c ^1).
    let installed = [
        inst("aa", "1.1.0", "[deps]\ncc = \"^2\""),
        inst("aa", "1.0.0", "[deps]\ncc = \"^1\""),
        inst("cc", "2.0.0", ""),
        inst("cc", "1.5.0", ""),
        inst("dd", "1.0.0", "[deps]\ncc = \"^1\""),
    ];
    assert_eq!(
        ok(&[pick("aa", "^1"), pick("dd", "*")], &installed),
        ["cc 1.5.0", "aa 1.0.0", "dd 1.0.0"]
    );
}

#[test]
fn version_that_conflicts_is_skipped_when_an_older_one_does_not() {
    let installed = [
        inst("aa", "2.0.0", "[conflicts]\nbb = \"*\""),
        inst("aa", "1.0.0", ""),
        inst("bb", "1.0.0", ""),
    ];
    assert_eq!(
        ok(&[pick("aa", "*"), pick("bb", "*")], &installed),
        ["aa 1.0.0", "bb 1.0.0"]
    );
}

#[test]
fn optional_dependency_is_ordered_before_when_present_and_in_range() {
    // By id `a` would load first; the optional edge puts `z` before it.
    let installed = [
        inst("aa", "1.0.0", "[optional]\nzz = \"^1\""),
        inst("zz", "1.2.0", ""),
    ];
    assert_eq!(
        ok(&[pick("aa", "*"), pick("zz", "*")], &installed),
        ["zz 1.2.0", "aa 1.0.0"]
    );
    // Absent optional: not pulled in, not an error.
    assert_eq!(ok(&[pick("aa", "*")], &installed), ["aa 1.0.0"]);
}

#[test]
fn optional_out_of_range_is_ignored_with_a_warning() {
    let installed = [
        inst("aa", "1.0.0", "[optional]\nzz = \"^2\""),
        inst("zz", "1.2.0", ""),
    ];
    let r = resolve(&[pick("aa", "*"), pick("zz", "*")], &installed, &HOST_API).unwrap();
    assert_eq!(names(&r.set), ["aa 1.0.0", "zz 1.2.0"]);
    assert_eq!(r.warnings.len(), 1);
    let Warning::OptionalOutOfRange {
        by,
        optional,
        found,
        ..
    } = &r.warnings[0];
    assert_eq!(
        (by.as_str(), optional.as_str(), *found),
        ("aa", "zz", Version::new(1, 2, 0))
    );
    let text = r.warnings[0].to_string();
    assert!(
        text.contains("optional `zz` ^2") && text.contains("1.2.0"),
        "{text}"
    );
}

#[test]
fn order_after_and_before() {
    let installed = [
        inst("aa", "1.0.0", "[order]\nafter = [\"cc\"]"),
        inst("bb", "1.0.0", "[order]\nbefore = [\"aa\"]"),
        inst("cc", "1.0.0", ""),
    ];
    let all = [pick("aa", "*"), pick("bb", "*"), pick("cc", "*")];
    // b before a, c before a; b and c tie, resolved by id.
    assert_eq!(ok(&all, &installed), ["bb 1.0.0", "cc 1.0.0", "aa 1.0.0"]);
    // Ordering against an absent mod is ignored.
    assert_eq!(ok(&[pick("aa", "*")], &installed), ["aa 1.0.0"]);
}

#[test]
fn ties_are_broken_by_id() {
    let installed = [
        inst("m3", "1.0.0", ""),
        inst("m1", "1.0.0", ""),
        inst("m2", "1.0.0", ""),
        inst("m0", "1.0.0", ""),
    ];
    let picks = [
        pick("m3", "*"),
        pick("m1", "*"),
        pick("m2", "*"),
        pick("m0", "*"),
    ];
    assert_eq!(
        ok(&picks, &installed),
        ["m0 1.0.0", "m1 1.0.0", "m2 1.0.0", "m3 1.0.0"]
    );
}

#[test]
fn api_must_admit_the_host_version() {
    let installed = [
        inst_api("aa", "2.0.0", "^2", ""),
        inst_api("aa", "1.0.0", "^1", ""),
    ];
    assert_eq!(ok(&[pick("aa", "*")], &installed), ["aa 1.0.0"]);
}

// ---- errors -------------------------------------------------------------------------------------------------

#[test]
fn error_host_pick_not_installed() {
    let e = err(&[pick("ghost", "*")], &[inst("aa", "1.0.0", "")]);
    assert!(matches!(e, ResolveError::Missing { ref id, .. } if id.as_str() == "ghost"));
    let t = e.to_string();
    assert!(
        t.contains("`ghost` is not installed")
            && t.contains("the host needs `ghost` *")
            && t.contains("host -> ghost"),
        "{t}"
    );
}

#[test]
fn error_missing_transitive_dependency_names_the_chain() {
    let installed = [
        inst("aa", "1.0.0", "[deps]\nbb = \"^1\""),
        inst("bb", "1.0.0", "[deps]\ndd = \">=2\""),
    ];
    let e = err(&[pick("aa", "*")], &installed);
    let t = e.to_string();
    assert!(matches!(e, ResolveError::Missing { ref id, .. } if id.as_str() == "dd"));
    assert!(t.contains("bb 1.0.0 needs `dd` >=2"), "{t}");
    assert!(
        t.contains("chain: host -> aa 1.0.0 -> bb 1.0.0 -> dd"),
        "{t}"
    );
}

#[test]
fn error_no_version_in_range_names_every_requirer() {
    // The example from the task: a needs b ^2, c pins b =1.4.0.
    let installed = [
        inst("aa", "1.0.0", "[deps]\nbb = \"^2\""),
        inst("cc", "1.2.0", "[deps]\nbb = \"=1.4.0\""),
        inst("bb", "1.4.0", ""),
        inst("bb", "2.0.0", ""),
    ];
    let e = err(&[pick("aa", "*"), pick("cc", "*")], &installed);
    let t = e.to_string();
    assert!(
        matches!(e, ResolveError::NoMatchingVersion { ref id, .. } if id.as_str() == "bb"),
        "{t}"
    );
    assert!(t.contains("aa 1.0.0 needs `bb` ^2"), "{t}");
    assert!(t.contains("cc 1.2.0 needs `bb` =1.4.0"), "{t}");
    assert!(t.contains("installed versions: 1.4.0, 2.0.0"), "{t}");
}

#[test]
fn error_host_range_has_no_match() {
    let e = err(
        &[pick("aa", "^3")],
        &[inst("aa", "1.0.0", ""), inst("aa", "2.1.0", "")],
    );
    let t = e.to_string();
    assert!(
        t.contains("the host needs `aa` ^3") && t.contains("installed versions: 1.0.0, 2.1.0"),
        "{t}"
    );
}

#[test]
fn error_conflict() {
    let installed = [
        inst("aa", "1.0.0", "[conflicts]\nbb = \"<2\""),
        inst("bb", "1.5.0", ""),
        inst("cc", "1.0.0", "[deps]\nbb = \"*\""),
    ];
    // a declares the conflict; b is pulled in through c.
    let e = err(&[pick("aa", "*"), pick("cc", "*")], &installed);
    let t = e.to_string();
    assert!(
        matches!(e, ResolveError::Conflict(ref c) if c.a.as_str() == "aa" && c.b.as_str() == "bb"),
        "{t}"
    );
    assert!(
        t.contains("aa 1.0.0 conflicts with bb 1.5.0") && t.contains("conflicts.bb = \"<2\""),
        "{t}"
    );
    assert!(
        t.contains("chain of aa: host -> aa 1.0.0")
            && t.contains("chain of bb: host -> cc 1.0.0 -> bb 1.5.0"),
        "{t}"
    );
    // Order of the picks does not matter, and an out-of-range conflict is harmless.
    let e2 = err(&[pick("cc", "*"), pick("aa", "*")], &installed);
    assert_eq!(e, e2);
    let fine = [
        inst("aa", "1.0.0", "[conflicts]\nbb = \">=2\""),
        inst("bb", "1.5.0", ""),
    ];
    assert_eq!(
        ok(&[pick("aa", "*"), pick("bb", "*")], &fine),
        ["aa 1.0.0", "bb 1.5.0"]
    );
}

#[test]
fn error_conflict_is_symmetric() {
    // The mod with the lower id is chosen first; the declaration sits on the later one.
    let installed = [
        inst("aa", "1.0.0", ""),
        inst("bb", "1.0.0", "[conflicts]\naa = \"*\""),
    ];
    let e = err(&[pick("aa", "*"), pick("bb", "*")], &installed);
    assert!(
        matches!(e, ResolveError::Conflict(ref c) if c.a.as_str() == "bb" && c.b.as_str() == "aa"),
        "{e}"
    );
}

#[test]
fn error_api_mismatch() {
    let installed = [
        inst_api("aa", "2.0.0", "^2", ""),
        inst_api("aa", "1.0.0", "^0.9", ""),
    ];
    let e = err(&[pick("aa", "*")], &installed);
    let t = e.to_string();
    assert!(matches!(e, ResolveError::ApiMismatch { .. }), "{t}");
    assert!(
        t.contains("host provides API 1.0.0") && t.contains("`aa`"),
        "{t}"
    );
    assert!(
        t.contains("2.0.0 wants api ^2") && t.contains("1.0.0 wants api ^0.9"),
        "{t}"
    );
    assert!(t.contains("chain: host -> aa"), "{t}");
}

#[test]
fn error_api_mismatch_of_a_transitive_dependency() {
    let installed = [
        inst("aa", "1.0.0", "[deps]\nbb = \"*\""),
        inst_api("bb", "1.0.0", "^7", ""),
    ];
    let t = err(&[pick("aa", "*")], &installed).to_string();
    assert!(
        t.contains("`bb`") && t.contains("chain: host -> aa 1.0.0 -> bb"),
        "{t}"
    );
}

#[test]
fn error_cycle_in_dependencies() {
    let installed = [
        inst("aa", "1.0.0", "[deps]\nbb = \"*\""),
        inst("bb", "1.0.0", "[deps]\ncc = \"*\""),
        inst("cc", "1.0.0", "[deps]\naa = \"*\""),
    ];
    let e = err(&[pick("aa", "*")], &installed);
    let t = e.to_string();
    assert!(matches!(e, ResolveError::Cycle { .. }), "{t}");
    for part in ["`aa` needs `bb`", "`bb` needs `cc`", "`cc` needs `aa`"] {
        assert!(t.contains(part), "{part} missing in: {t}");
    }
}

#[test]
fn error_cycle_through_order_constraints() {
    let installed = [
        inst("aa", "1.0.0", "[order]\nafter = [\"bb\"]"),
        inst("bb", "1.0.0", "[order]\nafter = [\"aa\"]"),
        inst("tail", "1.0.0", "[deps]\naa = \"*\""),
    ];
    let t = err(&[pick("tail", "*"), pick("bb", "*")], &installed).to_string();
    assert!(t.starts_with("load order cycle:"), "{t}");
    assert!(
        t.contains("`aa` is ordered after `bb`") && t.contains("`bb` is ordered after `aa`"),
        "{t}"
    );
    assert!(
        !t.contains("tail"),
        "a mod downstream of the cycle is not part of it: {t}"
    );
}

#[test]
fn error_duplicate_installed() {
    let e = err(&[], &[inst("aa", "1.0.0", ""), inst("aa", "1.0.0", "")]);
    assert_eq!(
        e.to_string(),
        "installed mod aa 1.0.0 appears more than once"
    );
}

#[test]
fn resolution_is_reproducible_through_the_lock() {
    let installed = [
        inst("aa", "1.0.0", "[deps]\nbb = \"*\""),
        inst("bb", "1.0.0", ""),
    ];
    let r1 = resolve(&[pick("aa", "*")], &installed, &HOST_API).unwrap();
    let r2 = resolve(&[pick("aa", "*")], &installed, &HOST_API).unwrap();
    assert_eq!(write_lock(&r1.set), write_lock(&r2.set));
}

// ---- property: permuting the input never changes the output ---------------------------------------------------

/// Deterministic xorshift, so the test needs no `rand` and always runs the same permutations.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, (self.next() % (i as u64 + 1)) as usize);
        }
    }
}

fn fixture() -> Vec<Installed> {
    vec![
        inst(
            "app",
            "2.0.0",
            "[deps]\nui = \"^2\"\nnet = \">=1.1\"\n[optional]\nfx = \"^1\"\n[order]\nafter = [\"audio\"]",
        ),
        inst("app", "1.0.0", "[deps]\nui = \"^1\""),
        inst("ui", "2.1.0", "[deps]\ncore = \"^3\""),
        inst("ui", "2.0.0", "[deps]\ncore = \"^2\""),
        inst("ui", "1.0.0", "[deps]\ncore = \"^2\""),
        inst("net", "1.3.0", "[deps]\ncore = \"^2\""),
        inst("net", "1.1.0", "[deps]\ncore = \"^2\""),
        inst("core", "3.0.0", ""),
        inst("core", "2.2.0", ""),
        inst("core", "2.0.0", ""),
        inst("fx", "1.0.0", "[order]\nbefore = [\"audio\"]"),
        inst("audio", "1.0.0", ""),
        inst("extra", "0.1.0", "[conflicts]\nfx = \"^2\""),
        inst("cyc1", "1.0.0", "[order]\nafter = [\"cyc2\"]"),
        inst("cyc2", "1.0.0", "[order]\nafter = [\"cyc1\"]"),
        inst("zed", "1.0.0", "[optional]\nextra = \"^9\""),
    ]
}

fn run(picks: &[Pick], installed: &[Installed]) -> Result<String, String> {
    match resolve(picks, installed, &HOST_API) {
        Ok(r) => Ok(format!("{}\n{:?}", write_lock(&r.set), r.warnings)),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn permuting_the_input_never_changes_the_output() {
    let installed = fixture();
    let scenarios: Vec<Vec<Pick>> = vec![
        vec![pick("app", "^2"), pick("audio", "*"), pick("fx", "*")],
        vec![pick("app", "*"), pick("zed", "*"), pick("extra", "*")],
        vec![pick("app", "^1"), pick("net", "*")],
        // Needs backtracking: app 2 -> ui 2.1 -> core 3 is fine, net pins core ^2 -> falls back to ui 2.0.0.
        vec![pick("app", "^2"), pick("net", "<1.3")],
        // Failing scenarios must fail with the same message in every order.
        vec![pick("app", "^2"), pick("core", "=3.0.0")],
        vec![pick("cyc1", "*"), pick("cyc2", "*"), pick("app", "*")],
        vec![pick("ghost", "*"), pick("app", "*")],
    ];
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut seen_ok = 0;
    let mut seen_err = 0;
    for picks in scenarios {
        let baseline = run(&picks, &installed);
        match &baseline {
            Ok(_) => seen_ok += 1,
            Err(_) => seen_err += 1,
        }
        for _ in 0..200 {
            let mut inst_p = installed.clone();
            let mut pick_p = picks.clone();
            rng.shuffle(&mut inst_p);
            rng.shuffle(&mut pick_p);
            assert_eq!(run(&pick_p, &inst_p), baseline, "picks {pick_p:?}");
        }
    }
    assert!(
        seen_ok >= 4 && seen_err >= 2,
        "fixture must cover both outcomes ({seen_ok} ok, {seen_err} err)"
    );
}

#[test]
fn every_permutation_of_a_small_input_agrees() {
    // Exhaustive for 5 installed mods: 120 permutations.
    let installed = vec![
        inst("aa", "1.0.0", "[deps]\nbb = \"*\"\n[optional]\ndd = \"*\""),
        inst("bb", "1.0.0", "[deps]\ncc = \"*\""),
        inst("cc", "1.0.0", ""),
        inst("dd", "1.0.0", "[order]\nbefore = [\"cc\"]"),
        inst("ee", "1.0.0", "[order]\nafter = [\"aa\"]"),
    ];
    let picks = [pick("aa", "*"), pick("dd", "*"), pick("ee", "*")];
    let baseline = run(&picks, &installed).unwrap();
    let mut idx: Vec<usize> = (0..installed.len()).collect();
    let mut count = 0;
    permute(&mut idx, 0, &mut |order| {
        let p: Vec<Installed> = order.iter().map(|&i| installed[i].clone()).collect();
        assert_eq!(run(&picks, &p).unwrap(), baseline, "order {order:?}");
        count += 1;
    });
    assert_eq!(count, 120);
}

fn permute(v: &mut Vec<usize>, k: usize, f: &mut dyn FnMut(&[usize])) {
    if k == v.len() {
        f(v);
        return;
    }
    for i in k..v.len() {
        v.swap(k, i);
        permute(v, k + 1, f);
        v.swap(k, i);
    }
}
