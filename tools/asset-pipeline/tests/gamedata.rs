//! game-data: snapshot hash check and the en_US cut, on tiny SQLite files made by the tests.
//! The real snapshot is only used by `real_snapshot_*`, when CIV5_DB_SNAPSHOT is set.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{LogicalId, Manifest, PrefetchGroup, Variant, VariantFormat};
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
            "civ5-gamedata-test-{}-{}",
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

/// Merged localization fixture: three languages in `LocalizedText`, one NULL text, and the
/// `Language_en_US` view the real file has.
fn make_loc(path: &Path) {
    let db = Db::create(path).unwrap();
    db.exec(
        "CREATE TABLE LocalizedText(Language TEXT, Tag TEXT, Text TEXT, Gender TEXT, Plurality TEXT);
         INSERT INTO LocalizedText VALUES
           ('en_US','TXT_B','Bee','',''),
           ('ru_RU','TXT_B','Pchela','',''),
           ('en_US','TXT_A','Ant','',''),
           ('de_DE','TXT_A','Ameise','',''),
           ('en_US','TXT_NULL',NULL,'','');
         CREATE VIEW Language_en_US AS SELECT Tag, Text, Gender, Plurality FROM LocalizedText
           WHERE Language = 'en_US';",
    )
    .unwrap();
}

fn make_gameplay(path: &Path) {
    let db = Db::create(path).unwrap();
    db.exec("CREATE TABLE Units(Type TEXT); INSERT INTO Units VALUES ('UNIT_WARRIOR');")
        .unwrap();
}

fn write_lock(dir: &Path, gameplay: &Path, loc: &Path) -> PathBuf {
    let path = dir.join("test.lock");
    let text = format!(
        "# test\ngameplay_db_sha256 = {}\nloc_sha256 = {}\n",
        sha256_hex(&std::fs::read(gameplay).unwrap()),
        sha256_hex(&std::fs::read(loc).unwrap())
    );
    std::fs::write(&path, text).unwrap();
    path
}

struct Fixture {
    _tmp: TempDir,
    snapshot: PathBuf,
    lock: PathBuf,
    out: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new();
    let snapshot = tmp.path().join("snap");
    std::fs::create_dir_all(&snapshot).unwrap();
    let (g, l) = (snapshot.join(GAMEPLAY_DB_FILE), snapshot.join(LOC_DB_FILE));
    make_gameplay(&g);
    make_loc(&l);
    let lock = write_lock(tmp.path(), &g, &l);
    let out = tmp.path().join("out");
    Fixture {
        snapshot,
        lock,
        out,
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

#[test]
fn matching_snapshot_is_copied_byte_for_byte_into_the_manifest() {
    let f = fixture();
    let s = gamedata::build(&f.snapshot, &f.lock, &f.out).unwrap();
    let m = manifest_of(&f.out);
    let store = BlobStore::open(&f.out).unwrap();
    let v = &m.variants(&LogicalId::new(GAMEPLAY_DB_ID))[0];
    assert_eq!(v.size, s.gameplay_bytes);
    assert_eq!(
        std::fs::read(store.blob_path(&v.hash)).unwrap(),
        std::fs::read(f.snapshot.join(GAMEPLAY_DB_FILE)).unwrap()
    );
    assert!(has(&m, LOC_EN_US_ID));
}

#[test]
fn hash_mismatch_names_the_file_and_the_pinned_hash_and_stores_nothing() {
    let f = fixture();
    // Change the gameplay DB after the lock was written.
    let g = f.snapshot.join(GAMEPLAY_DB_FILE);
    let mut bytes = std::fs::read(&g).unwrap();
    bytes.push(0);
    std::fs::write(&g, &bytes).unwrap();
    let lock = Lock::read(&f.lock).unwrap();

    let err = gamedata::build(&f.snapshot, &f.lock, &f.out).unwrap_err();
    let GameDataError::HashMismatch {
        path,
        expected,
        actual,
        ..
    } = &err
    else {
        panic!("wrong error: {err}");
    };
    assert!(path.ends_with(GAMEPLAY_DB_FILE), "{path}");
    assert_eq!(expected, &lock.gameplay_db_sha256);
    assert_eq!(actual, &sha256_hex(&bytes));
    let text = err.to_string();
    assert!(
        text.contains(GAMEPLAY_DB_FILE) && text.contains(expected.as_str()),
        "{text}"
    );
    assert!(!has(&manifest_of(&f.out), GAMEPLAY_DB_ID));
}

#[test]
fn loc_hash_mismatch_is_also_an_error() {
    let f = fixture();
    let l = f.snapshot.join(LOC_DB_FILE);
    let mut bytes = std::fs::read(&l).unwrap();
    bytes.push(1);
    std::fs::write(&l, bytes).unwrap();
    let err = gamedata::build(&f.snapshot, &f.lock, &f.out).unwrap_err();
    assert!(
        matches!(&err, GameDataError::HashMismatch { path, .. } if path.ends_with(LOC_DB_FILE)),
        "{err}"
    );
}

#[test]
fn missing_snapshot_file_is_a_clear_error() {
    let f = fixture();
    std::fs::remove_file(f.snapshot.join(LOC_DB_FILE)).unwrap();
    let err = gamedata::build(&f.snapshot, &f.lock, &f.out).unwrap_err();
    assert!(
        matches!(err, GameDataError::MissingSnapshotFile { .. }),
        "{err}"
    );
}

#[test]
fn lock_parser_rejects_bad_input() {
    let good = format!(
        "gameplay_db_sha256 = {0}\nloc_sha256 = {0}\n",
        "a".repeat(64)
    );
    assert!(Lock::parse(&good, "t").is_ok());
    for bad in [
        "gameplay_db_sha256 = abc\nloc_sha256 = abc\n".to_owned(),
        format!("gameplay_db_sha256 = {}\n", "a".repeat(64)),
        format!("{good}extra = {}\n", "a".repeat(64)),
        "nonsense\n".to_owned(),
    ] {
        assert!(Lock::parse(&bad, "t").is_err(), "{bad}");
    }
}

#[test]
fn committed_lock_parses() {
    Lock::read(&gamedata::default_lock_path()).unwrap();
}

#[test]
fn en_us_cut_keeps_only_en_us_rows_ordered_by_tag() {
    let tmp = TempDir::new();
    let (src, dest) = (tmp.path().join("loc.db"), tmp.path().join("cut.db"));
    make_loc(&src);
    let before = std::fs::read(&src).unwrap();

    let n = gamedata::cut_en_us(&src, &dest).unwrap();
    assert_eq!(n, 3);
    let db = Db::open_read_only(&dest).unwrap();
    assert_eq!(
        db.scalar_i64("SELECT count(*) FROM Language_en_US")
            .unwrap(),
        3
    );
    assert_eq!(
        db.scalar_text("SELECT Text FROM Language_en_US WHERE Tag='TXT_A'")
            .unwrap()
            .as_deref(),
        Some("Ant")
    );
    // WITHOUT ROWID: stored in Tag order.
    assert_eq!(
        db.scalar_text("SELECT Tag FROM Language_en_US LIMIT 1")
            .unwrap()
            .as_deref(),
        Some("TXT_A")
    );
    // Other languages and the extra columns are gone; a NULL text stays NULL.
    assert_eq!(
        db.scalar_i64("SELECT count(*) FROM Language_en_US WHERE Text IN ('Pchela','Ameise')")
            .unwrap(),
        0
    );
    assert_eq!(
        db.scalar_i64("SELECT count(*) FROM pragma_table_info('Language_en_US')")
            .unwrap(),
        2
    );
    assert_eq!(
        db.scalar_i64("SELECT count(*) FROM Language_en_US WHERE Text IS NULL")
            .unwrap(),
        1
    );
    drop(db);
    // The source is untouched and no journal files appear next to it.
    assert_eq!(std::fs::read(&src).unwrap(), before);
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 2);
}

#[test]
fn en_us_cut_is_deterministic() {
    let tmp = TempDir::new();
    let src = tmp.path().join("loc.db");
    make_loc(&src);
    let (a, b) = (tmp.path().join("a.db"), tmp.path().join("b.db"));
    gamedata::cut_en_us(&src, &a).unwrap();
    gamedata::cut_en_us(&src, &b).unwrap();
    assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap());
}

#[test]
fn en_us_cut_without_the_source_table_is_a_sqlite_error() {
    let tmp = TempDir::new();
    let src = tmp.path().join("empty.db");
    Db::create(&src).unwrap().exec("CREATE TABLE X(a)").unwrap();
    let err = gamedata::cut_en_us(&src, &tmp.path().join("cut.db")).unwrap_err();
    assert!(err.to_string().contains("LocalizedText"), "{err}");
}

#[test]
fn game_data_keeps_entries_of_earlier_builds_in_the_manifest() {
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
    gamedata::build(&f.snapshot, &f.lock, &f.out).unwrap();
    let m = manifest_of(&f.out);
    assert!(has(&m, "tex:earlier"));
    assert!(has(&m, GAMEPLAY_DB_ID));
}

/// Real snapshot against the committed lock; runs only when CIV5_DB_SNAPSHOT is set.
#[test]
fn real_snapshot_matches_the_committed_lock_and_cuts_30012_rows() {
    let Some(dir) = std::env::var_os(gamedata::SNAPSHOT_ENV).map(PathBuf::from) else {
        eprintln!("skipped: {} is not set", gamedata::SNAPSHOT_ENV);
        return;
    };
    let tmp = TempDir::new();
    let s = gamedata::build(
        &dir,
        &gamedata::default_lock_path(),
        &tmp.path().join("out"),
    )
    .unwrap();
    assert_eq!(s.loc_rows, 30_012);
}
