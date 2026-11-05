//! Scan of a real install; skipped unless `CIV5_ROOT` points at one.

use asset_pipeline::game::GameSource;

#[test]
fn scans_real_install() {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return;
    };
    let src = GameSource::open(&root).expect("scan failed");
    let st = src.stats();

    assert!(st.pack_errors.is_empty(), "{:?}", st.pack_errors);
    // Resource/Common (27) + Resource/DX9 (71); the 49 DX9_Low packs make up the 147 total.
    assert_eq!(st.packs, 98);
    let packed: u64 = st.packed.values().map(|s| s.count).sum();
    assert!(packed > 25_000, "only {packed} packed entries");
    // D15: Lekmod v34.15 is an allowed art source (a low-priority layer), not an excluded mod.
    assert!(
        !st.excluded
            .iter()
            .any(|e| e.dir.eq_ignore_ascii_case("LEKMOD_v34.15"))
    );

    let warrior = src
        .lookup("Warrior.fxsxml")
        .expect("warrior.fxsxml missing");
    let bytes = src.read(warrior).expect("read warrior.fxsxml");
    assert_eq!(bytes.len() as u64, warrior.size());
    assert!(src.lookup("warrior_diff.dds").is_some());
}
