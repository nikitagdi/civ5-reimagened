//! Behaviour of the pack codec and of verification against `assets.lock`.

use mod_assets::{
    AssetKind, AssetsLock, LockProblem, NewFile, PACK_MAGIC, Pack, PackError, PackManifest, Sha256,
    Source, load, valid_asset_id, verify_against_lock,
};
use mod_manifest::ModId;

fn id() -> ModId {
    ModId::new("hello").unwrap()
}

fn src(name: &str, data: &[u8]) -> Source {
    Source {
        name: name.into(),
        sha256: Sha256::of(data),
    }
}

fn raw(id: &str, bytes: &[u8]) -> NewFile {
    NewFile {
        id: id.into(),
        kind: AssetKind::Raw,
        bytes: bytes.to_vec(),
    }
}

/// 4x4 texture with 3 mip levels: 64 + 16 + 4 bytes.
fn tex(id: &str, fill: u8) -> NewFile {
    NewFile {
        id: id.into(),
        kind: AssetKind::TextureRgba8 {
            width: 4,
            height: 4,
            mips: 3,
        },
        bytes: vec![fill; 84],
    }
}

fn sample() -> Pack {
    Pack::build(
        id(),
        vec![src("game.jar", b"jar"), src("index.json", b"idx")],
        vec![
            tex("textures/dirt", 1),
            raw("data/blocks", b"a=1\n"),
            NewFile {
                id: "sounds/dig".into(),
                kind: AssetKind::Sound,
                bytes: vec![7; 300],
            },
        ],
    )
    .unwrap()
}

#[test]
fn round_trip_is_byte_stable() {
    let p = sample();
    let bytes = p.to_bytes();
    assert_eq!(&bytes[..8], &PACK_MAGIC);
    let back = Pack::parse(&bytes).unwrap();
    assert_eq!(back, p);
    assert_eq!(back.to_bytes(), bytes);
    assert_eq!(back.hash(), p.hash());
    assert_eq!(back.get("data/blocks"), Some(&b"a=1\n"[..]));
    assert_eq!(back.get("sounds/dig").map(<[u8]>::len), Some(300));
    assert_eq!(back.get("nope"), None);
    assert_eq!(
        back.ids().collect::<Vec<_>>(),
        ["data/blocks", "sounds/dig", "textures/dirt"]
    );
}

#[test]
fn build_order_does_not_change_the_pack() {
    let a = Pack::build(id(), vec![], vec![raw("a", b"1"), raw("b", b"2")]).unwrap();
    let b = Pack::build(id(), vec![], vec![raw("b", b"2"), raw("a", b"1")]).unwrap();
    assert_eq!(a.to_bytes(), b.to_bytes());
}

#[test]
fn golden_manifest_text() {
    let p = Pack::build(
        id(),
        vec![src("game.jar", b"jar")],
        vec![raw("data/x", b"abc"), tex("t", 0)],
    )
    .unwrap();
    let want = format!(
        "c5-mod-assets 1\nmod hello\nsource game.jar {}\nfile data/x raw 3 {}\nfile t texture-rgba8:4x4x3 84 {}\n",
        Sha256::of(b"jar"),
        Sha256::of(b"abc"),
        Sha256::of(&[0u8; 84]),
    );
    assert_eq!(p.manifest().to_text(), want);
    assert_eq!(PackManifest::parse_text(&want).unwrap(), *p.manifest());
}

#[test]
fn empty_pack_is_valid() {
    let p = Pack::build(id(), vec![], vec![]).unwrap();
    assert_eq!(Pack::parse(&p.to_bytes()).unwrap(), p);
}

#[test]
fn corrupt_file_byte_is_found_by_id() {
    let p = sample();
    let mut bytes = p.to_bytes();
    let last = bytes.len() - 1; // last byte belongs to textures/dirt (sorted last)
    bytes[last] ^= 1;
    match Pack::parse(&bytes) {
        Err(PackError::Corrupt {
            id,
            expected,
            actual,
        }) => {
            assert_eq!(id, "textures/dirt");
            assert_ne!(expected, actual);
        }
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn truncated_and_trailing_bytes() {
    let bytes = sample().to_bytes();
    assert!(matches!(
        Pack::parse(&bytes[..5]),
        Err(PackError::Truncated(_))
    ));
    assert!(matches!(
        Pack::parse(&bytes[..20]),
        Err(PackError::Truncated("the manifest"))
    ));
    assert!(matches!(
        Pack::parse(&bytes[..bytes.len() - 1]),
        Err(PackError::Truncated("the file data"))
    ));
    let mut more = bytes.clone();
    more.extend_from_slice(&[0, 0, 0]);
    assert_eq!(Pack::parse(&more), Err(PackError::TrailingBytes(3)));
}

#[test]
fn bad_magic_and_giant_manifest_length() {
    let mut bytes = sample().to_bytes();
    bytes[0] = b'X';
    assert_eq!(Pack::parse(&bytes), Err(PackError::BadMagic));
    let mut huge = PACK_MAGIC.to_vec();
    huge.extend_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        Pack::parse(&huge),
        Err(PackError::ManifestTooLarge { .. })
    ));
}

#[test]
fn pack_hash_mismatch_is_reported() {
    let p = sample();
    let bytes = p.to_bytes();
    assert!(Pack::parse_expecting(&bytes, &p.hash()).is_ok());
    let wrong = Sha256::of(b"other");
    assert_eq!(
        Pack::parse_expecting(&bytes, &wrong),
        Err(PackError::HashMismatch {
            expected: wrong,
            actual: p.hash()
        })
    );
}

#[test]
fn asset_ids() {
    for ok in ["a", "textures/block/dirt", "steve.png", "a-b_c/d.e", "0x"] {
        assert!(valid_asset_id(ok), "{ok}");
    }
    for bad in [
        "",
        "/a",
        "a/",
        "a//b",
        "../a",
        "a/..",
        ".hidden",
        "A",
        "a b",
        "a\\b",
        &"x".repeat(129),
    ] {
        assert!(!valid_asset_id(bad), "{bad:?}");
    }
    assert!(matches!(
        Pack::build(id(), vec![], vec![raw("../evil", b"")]),
        Err(PackError::BadId(_))
    ));
}

#[test]
fn duplicates_and_sizes_are_rejected() {
    assert_eq!(
        Pack::build(id(), vec![], vec![raw("a", b"1"), raw("a", b"2")]).unwrap_err(),
        PackError::DuplicateId("a".into())
    );
    assert_eq!(
        Pack::build(id(), vec![src("s", b"1"), src("s", b"2")], vec![]).unwrap_err(),
        PackError::DuplicateSource("s".into())
    );
    let short = NewFile {
        id: "t".into(),
        kind: AssetKind::TextureRgba8 {
            width: 4,
            height: 4,
            mips: 1,
        },
        bytes: vec![0; 63],
    };
    assert!(matches!(
        Pack::build(id(), vec![], vec![short]),
        Err(PackError::BadSize {
            expected: 64,
            actual: 63,
            ..
        })
    ));
    let zero = NewFile {
        id: "t".into(),
        kind: AssetKind::TextureRgba8 {
            width: 0,
            height: 4,
            mips: 1,
        },
        bytes: vec![],
    };
    assert!(matches!(
        Pack::build(id(), vec![], vec![zero]),
        Err(PackError::BadTexture { .. })
    ));
}

#[test]
fn mip_sizes_clamp_to_one() {
    let k = AssetKind::TextureRgba8 {
        width: 4,
        height: 1,
        mips: 3,
    };
    // 4x1 + 2x1 + 1x1 pixels
    assert_eq!(k.exact_size(), Some((4 + 2 + 1) * 4));
    assert_eq!(AssetKind::Raw.exact_size(), None);
}

#[test]
fn manifest_text_is_strict() {
    let good = sample().manifest().to_text();
    assert!(PackManifest::parse_text(&good).is_ok());
    let cases = [
        good.trim_end().to_string(),                        // no final newline
        good.replace("c5-mod-assets 1", "c5-mod-assets 2"), // header
        good.replace("mod hello", "mod Hello"),             // mod id
        format!("{good}wat\n"),                             // unknown record
        good.replace("file data/blocks raw", "file data/blocks gif"),
        good.replace("source game.jar", "source game.jar extra"),
    ];
    for (i, c) in cases.iter().enumerate() {
        assert!(PackManifest::parse_text(c).is_err(), "case {i}");
    }
    // Out-of-order files are not canonical.
    let lines: Vec<&str> = good.lines().collect();
    let mut swapped = lines.clone();
    let n = swapped.len();
    swapped.swap(n - 1, n - 2);
    let text = format!("{}\n", swapped.join("\n"));
    assert!(matches!(
        PackManifest::parse_text(&text),
        Err(PackError::Manifest { .. })
    ));
}

#[test]
fn pack_hash_covers_every_byte() {
    let a = sample();
    let b = Pack::build(
        id(),
        vec![src("game.jar", b"jar"), src("index.json", b"idx")],
        vec![
            tex("textures/dirt", 2),
            raw("data/blocks", b"a=1\n"),
            NewFile {
                id: "sounds/dig".into(),
                kind: AssetKind::Sound,
                bytes: vec![7; 300],
            },
        ],
    )
    .unwrap();
    assert_ne!(a.hash(), b.hash());
}

// ----- assets.lock -----

#[test]
fn lock_of_pack_verifies_and_round_trips() {
    let p = sample();
    let lock = AssetsLock::of_pack(&p);
    assert_eq!(verify_against_lock(&p, &lock), Ok(()));
    let text = lock.to_text();
    assert!(text.starts_with("c5-assets-lock 1\nsource game.jar "));
    assert_eq!(AssetsLock::parse_text(&text).unwrap(), lock);
    assert_eq!(AssetsLock::parse_text(&text).unwrap().to_text(), text);
}

#[test]
fn lock_detects_bad_hash_missing_and_extra_files() {
    let p = sample();
    let mut lock = AssetsLock::of_pack(&p);
    // changed hash
    lock.files[0].1 = Sha256::of(b"different");
    // a file the pack lacks
    lock.files.push(("zzz/gone".into(), Sha256::of(b"g")));
    // a file the lock lacks
    let removed = lock.files.remove(1).0;
    let VerifyErrorProblems(problems) = verify(&p, &lock);
    assert!(
        problems
            .iter()
            .any(|x| matches!(x, LockProblem::FileMismatch { id, .. } if id == "data/blocks"))
    );
    assert!(problems.contains(&LockProblem::FileMissing("zzz/gone".into())));
    assert!(problems.contains(&LockProblem::FileExtra(removed)));
    assert_eq!(problems.len(), 3);
}

struct VerifyErrorProblems(Vec<LockProblem>);

fn verify(p: &Pack, lock: &AssetsLock) -> VerifyErrorProblems {
    VerifyErrorProblems(verify_against_lock(p, lock).unwrap_err().0)
}

#[test]
fn lock_detects_source_changes() {
    let p = sample();
    let mut lock = AssetsLock::of_pack(&p);
    lock.sources[0].sha256 = Sha256::of(b"another jar");
    lock.sources.push(Source {
        name: "zzz.bin".into(),
        sha256: Sha256::of(b"z"),
    });
    lock.sources.remove(1); // index.json
    let VerifyErrorProblems(problems) = verify(&p, &lock);
    assert!(
        problems
            .iter()
            .any(|x| matches!(x, LockProblem::SourceMismatch { name, .. } if name == "game.jar"))
    );
    assert!(problems.contains(&LockProblem::SourceMissing("zzz.bin".into())));
    assert!(problems.contains(&LockProblem::SourceExtra("index.json".into())));
}

#[test]
fn lock_text_is_strict() {
    let good = AssetsLock::of_pack(&sample()).to_text();
    let first_file = good
        .lines()
        .find(|l| l.starts_with("file "))
        .unwrap()
        .to_string();
    let cases = [
        good.trim_end().to_string(),
        good.replace("c5-assets-lock 1", "c5-assets-lock 9"),
        format!("{good}file zzz nothex\n"),
        format!("{good}{first_file}\n"), // duplicate / unsorted
        format!("{good}extra record\n"),
        format!("{good}file Bad/Id {}\n", Sha256::of(b"")),
        format!("{good}source late.bin {}\n", Sha256::of(b"")), // source after file
    ];
    for (i, c) in cases.iter().enumerate() {
        assert!(
            matches!(AssetsLock::parse_text(c), Err(PackError::Lock { .. })),
            "case {i}"
        );
    }
}

// ----- load -----

#[test]
fn load_signals_a_missing_pack() {
    assert_eq!(load(None, None, None), Err(PackError::Missing));
    let p = sample();
    assert_eq!(load(None, Some(&p.hash()), None), Err(PackError::Missing));
}

#[test]
fn load_checks_hash_then_lock() {
    let p = sample();
    let bytes = p.to_bytes();
    let lock = AssetsLock::of_pack(&p);
    assert_eq!(load(Some(&bytes), Some(&p.hash()), Some(&lock)).unwrap(), p);

    let mut tampered = bytes.clone();
    let n = tampered.len();
    tampered[n - 1] ^= 0xff;
    assert!(matches!(
        load(Some(&tampered), Some(&p.hash()), Some(&lock)),
        Err(PackError::HashMismatch { .. })
    ));
    // Without the expected hash the per-file check still catches it.
    assert!(matches!(
        load(Some(&tampered), None, Some(&lock)),
        Err(PackError::Corrupt { .. })
    ));

    // A well-formed pack that differs from the lock.
    let other = Pack::build(id(), vec![], vec![raw("a", b"1")]).unwrap();
    assert!(matches!(
        load(Some(&other.to_bytes()), None, Some(&lock)),
        Err(PackError::Verify(_))
    ));
    // No lock: only the internal checks.
    assert!(load(Some(&other.to_bytes()), None, None).is_ok());
}

#[test]
fn verify_error_names_every_problem() {
    let p = sample();
    let lock = AssetsLock::default();
    let msg = verify_against_lock(&p, &lock).unwrap_err().to_string();
    assert!(msg.contains("assets.lock"), "{msg}");
    assert!(
        msg.contains("data/blocks") && msg.contains("textures/dirt") && msg.contains("game.jar"),
        "{msg}"
    );
}
