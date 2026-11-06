//! `ModSet`, `modset_hash` and the `mods.lock` format.

use std::collections::BTreeSet;

use mod_manifest::{
    ClientPolicy, ContentHash, LockError, ModId, ModSet, ModSetError, ModsetHash, Part,
    ResolvedMod, SyncClass, Version, parse_lock, write_lock,
};

fn rm(
    id: &str,
    ver: (u32, u32, u32),
    idx: u16,
    sync: SyncClass,
    parts: &[Part],
    hash: u8,
) -> ResolvedMod {
    ResolvedMod {
        id: id.parse::<ModId>().unwrap(),
        version: Version::new(ver.0, ver.1, ver.2),
        load_idx: idx,
        sync,
        client: match sync {
            SyncClass::Gameplay => ClientPolicy::Required,
            _ => ClientPolicy::Optional,
        },
        parts: parts.iter().copied().collect::<BTreeSet<_>>(),
        content_hash: ContentHash([hash; 32]),
    }
}

fn sample() -> Vec<ResolvedMod> {
    vec![
        rm(
            "base",
            (1, 2, 3),
            0,
            SyncClass::Gameplay,
            &[Part::Data, Part::LuaGameplay],
            0xAA,
        ),
        rm(
            "voxel-demo",
            (0, 1, 0),
            1,
            SyncClass::Cosmetic,
            &[Part::ServerRs, Part::ClientRs, Part::Assets],
            0x01,
        ),
    ]
}

fn set(mods: Vec<ResolvedMod>) -> ModSet {
    ModSet::new(mods).unwrap()
}

/// Golden value: changing it means the hash (and so every saved `modset_hash`) changed. Bump `HASH_FORMAT`
/// in `modset.rs` on purpose, never by accident.
const GOLDEN_HASH: &str = "5591bbc6dc9d9e63874cc818bdad758a";
const GOLDEN_EMPTY_HASH: &str = "fd69eb87e0a426fa964945b18530aac2";

#[test]
fn golden_hash() {
    assert_eq!(set(sample()).modset_hash().to_string(), GOLDEN_HASH);
    assert_eq!(
        ModSet::default().modset_hash().to_string(),
        GOLDEN_EMPTY_HASH
    );
}

#[test]
fn hash_is_stable_and_clone_independent() {
    let a = set(sample());
    let b = set(sample());
    assert_eq!(a.modset_hash(), b.modset_hash());
    assert_eq!(a.clone().modset_hash(), a.modset_hash());
}

#[test]
fn any_change_changes_the_hash() {
    let base = set(sample()).modset_hash();
    let mut seen = BTreeSet::from([base]);
    let mut check = |name: &str, mods: Vec<ResolvedMod>| {
        let h = set(mods).modset_hash();
        assert!(
            seen.insert(h),
            "{name}: hash collides with an earlier variant"
        );
    };

    let mut v = sample();
    v[0].version = Version::new(1, 2, 4);
    check("patch version", v);

    let mut v = sample();
    v[0].version = Version::new(1, 3, 3);
    check("minor version", v);

    let mut v = sample();
    v[1].content_hash = ContentHash([0x02; 32]);
    check("content hash", v);

    let mut v = sample();
    let mut h = [0xAA; 32];
    h[31] ^= 1;
    v[0].content_hash = ContentHash(h);
    check("last content-hash byte", v);

    // Load order: same mods, swapped (load_idx follows the position).
    let mut v = sample();
    v.swap(0, 1);
    v[0].load_idx = 0;
    v[1].load_idx = 1;
    check("order", v);

    let mut v = sample();
    v[0].sync = SyncClass::Cosmetic;
    check("sync class", v);

    let mut v = sample();
    v[1].client = ClientPolicy::Required;
    check("client policy", v);

    let mut v = sample();
    v[1].parts.remove(&Part::Assets);
    check("parts", v);

    let mut v = sample();
    v[1].id = "voxel-demo2".parse().unwrap();
    check("id", v);

    check(
        "fewer mods",
        sample()[..1]
            .iter()
            .cloned()
            .map(|mut m| {
                m.load_idx = 0;
                m
            })
            .collect(),
    );
    check("empty", Vec::new());
}

#[test]
fn id_boundaries_cannot_be_forged() {
    // "ab"+"c" vs "a"+"bc" style ambiguity: ids are length-prefixed in the canonical bytes.
    let one = set(vec![rm(
        "ab",
        (1, 0, 0),
        0,
        SyncClass::Gameplay,
        &[Part::Data],
        0,
    )]);
    let two = set(vec![rm(
        "abab",
        (1, 0, 0),
        0,
        SyncClass::Gameplay,
        &[Part::Data],
        0,
    )]);
    assert_ne!(one.modset_hash(), two.modset_hash());
}

#[test]
fn modset_invariants() {
    let mut dup = sample();
    dup[1].id = dup[0].id.clone();
    assert!(matches!(ModSet::new(dup), Err(ModSetError::DuplicateId(_))));

    let mut gap = sample();
    gap[1].load_idx = 5;
    let e = ModSet::new(gap).unwrap_err();
    assert_eq!(e.to_string(), "mod `voxel-demo` has load_idx 5, expected 1");

    let s = set(sample());
    assert_eq!(s.len(), 2);
    assert_eq!(s.by_idx(1).unwrap().id.as_str(), "voxel-demo");
    assert!(s.by_idx(2).is_none());
    assert_eq!(s.get(&"base".parse().unwrap()).unwrap().load_idx, 0);
    assert!(s.get(&"nope".parse().unwrap()).is_none());
}

#[test]
fn hex_round_trip() {
    let h = set(sample()).modset_hash();
    assert_eq!(ModsetHash::from_hex(&h.to_string()), Some(h));
    assert_eq!(ModsetHash::from_hex("xyz"), None);
    assert_eq!(
        ModsetHash::from_hex(&"A".repeat(32)),
        None,
        "upper case is not canonical"
    );
    let c = ContentHash([0x5A; 32]);
    assert_eq!(ContentHash::from_hex(&c.to_string()), Some(c));
    assert_eq!(ContentHash::from_hex(&"0".repeat(63)), None);
}

const GOLDEN_LOCK: &str = r#"# mods.lock - written by `cargo xtask mods build`; do not edit by hand.
version = 1
modset_hash = "HASH"

[[mod]]
id = "base"
version = "1.2.3"
load_idx = 0
sync = "gameplay"
client = "required"
parts = ["data", "lua-gameplay"]
content_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[[mod]]
id = "voxel-demo"
version = "0.1.0"
load_idx = 1
sync = "cosmetic"
client = "optional"
parts = ["assets", "server-rs", "client-rs"]
content_hash = "0101010101010101010101010101010101010101010101010101010101010101"
"#;

#[test]
fn lock_text_is_byte_stable() {
    let text = write_lock(&set(sample()));
    assert_eq!(text, GOLDEN_LOCK.replace("HASH", GOLDEN_HASH));
    assert!(text.ends_with('\n') && !text.contains('\r'));
    assert_eq!(write_lock(&set(sample())), text);
}

#[test]
fn lock_round_trips() {
    let s = set(sample());
    let text = write_lock(&s);
    let back = parse_lock(&text).unwrap();
    assert_eq!(back, s);
    assert_eq!(
        write_lock(&back),
        text,
        "parse then write must reproduce the bytes"
    );
    // Empty set.
    let empty = write_lock(&ModSet::default());
    assert_eq!(parse_lock(&empty).unwrap(), ModSet::default());
    assert_eq!(write_lock(&parse_lock(&empty).unwrap()), empty);
}

#[test]
fn lock_rejects_tampering() {
    let good = write_lock(&set(sample()));
    // Edited version: the stated modset_hash no longer matches.
    let e = parse_lock(&good.replace("version = \"1.2.3\"", "version = \"1.2.4\"")).unwrap_err();
    assert!(matches!(e, LockError::HashMismatch { .. }), "{e}");
    assert!(e.to_string().contains("modset_hash"), "{e}");
    // Edited hash line.
    let e = parse_lock(&good.replace(GOLDEN_HASH, &"0".repeat(32))).unwrap_err();
    assert!(matches!(e, LockError::HashMismatch { .. }), "{e}");
    // Swapped blocks keep the hash line but break load_idx order.
    let bad_idx = good.replace("load_idx = 1", "load_idx = 3");
    assert!(matches!(
        parse_lock(&bad_idx),
        Err(LockError::Set(ModSetError::BadLoadIdx { .. }))
    ));
}

#[test]
fn lock_rejects_malformed_files() {
    let good = write_lock(&set(sample()));
    let cases: Vec<(String, &str)> = vec![
        (
            good.replace("version = 1\n", "version = 2\n"),
            "unsupported lock version 2",
        ),
        (
            good.replace("version = 1\n", ""),
            "version: missing required key",
        ),
        (
            good.replace("sync = \"gameplay\"", "sync = \"loud\""),
            "mod[0].sync",
        ),
        (
            good.replace("client = \"required\"", "client = \"sometimes\""),
            "mod[0].client",
        ),
        (
            good.replace("\"lua-gameplay\"", "\"nope\""),
            "mod[0].parts[1]",
        ),
        (good.replace("id = \"base\"", "id = \"Base\""), "mod[0].id"),
        (
            good.replace("version = \"0.1.0\"", "version = \"0.1\""),
            "mod[1].version",
        ),
        (
            good.replace("id = \"base\"", "id = \"base\"\nextra = 1"),
            "mod[0].extra: unknown key",
        ),
        (format!("{good}\nsurprise = 1\n"), "surprise: unknown key"),
        (good.replace(&"aa".repeat(32), "zz"), "mod[0].content_hash"),
        (
            good.replace("modset_hash = ", "modset_hash = 1 #"),
            "modset_hash",
        ),
        ("not = [valid".to_string(), "invalid TOML"),
    ];
    for (text, expect) in cases {
        let e = parse_lock(&text).unwrap_err().to_string();
        assert!(e.contains(expect), "expected {expect:?} in {e:?}");
    }
}
