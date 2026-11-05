//! Measurement, not a gate: sizes and times of the `GCS1` packers on real saves. Run with
//! `CIV5_SAVE_DIR=<dir with .c5sv files, searched recursively> cargo test -p server --release --test
//! save_pack_measure -- --ignored --nocapture`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use game_host::saves::{self, SaveSections};

fn files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|x| x == "c5sv") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs CIV5_SAVE_DIR"]
fn measure_gcs1_packers() {
    let dir = std::env::var("CIV5_SAVE_DIR").expect("CIV5_SAVE_DIR");
    let mut all = Vec::new();
    files(Path::new(&dir), &mut all);
    all.sort();
    // The biggest (latest) save and a middle one.
    let picks = [all[all.len() / 2].clone(), all[all.len() - 1].clone()];
    for p in picks {
        let bytes = std::fs::read(&p).unwrap();
        let t = Instant::now();
        let save = saves::decode(&bytes).unwrap();
        println!(
            "\n{}: file {} B (ruzstd-packed), decode (ruzstd) {:?}, raw GCS1 {} B",
            p.file_name().unwrap().to_string_lossy(),
            bytes.len(),
            t.elapsed(),
            save.gcs1.len()
        );
        let sections = SaveSections {
            seats: save.info.seats.clone(),
            settings: save.info.settings.clone(),
            journals: save.journals.clone(),
            cmds: save.cmds.clone(),
            city_memo: save.city_memo.clone().unwrap_or_default(),
        };
        let t = Instant::now();
        let out = saves::encode(&save.info.header, &save.gcs1, &sections);
        println!(
            "  ruzstd Fastest : {:>9} B  pack {:?}",
            out.len(),
            t.elapsed()
        );
        for level in [1, 3, 5, 7, 9, 12] {
            let t = Instant::now();
            let out = saves::encode_with(&save.info.header, &save.gcs1, &sections, |raw| {
                wire::compress_with(raw, |r| zstd::bulk::compress(r, level).unwrap())
            });
            let dt = t.elapsed();
            let t = Instant::now();
            let back = saves::decode(&out).unwrap();
            let dd = t.elapsed();
            assert_eq!(back.gcs1, save.gcs1);
            println!(
                "  zstd level {level:>2}  : {:>9} B  pack {dt:?}  decode (ruzstd) {dd:?}",
                out.len()
            );
        }
    }
}

#[test]
#[ignore = "needs CIV5_SAVE_DIR"]
fn measure_whole_game_dir_at_the_server_level() {
    let dir = std::env::var("CIV5_SAVE_DIR").expect("CIV5_SAVE_DIR");
    let mut all = Vec::new();
    files(Path::new(&dir), &mut all);
    all.sort();
    let (mut before, mut after, mut worst) = (0usize, 0usize, std::time::Duration::ZERO);
    for p in &all {
        let bytes = std::fs::read(p).unwrap();
        let save = saves::decode(&bytes).unwrap();
        let sections = SaveSections {
            seats: save.info.seats.clone(),
            settings: save.info.settings.clone(),
            journals: save.journals.clone(),
            cmds: save.cmds.clone(),
            city_memo: save.city_memo.clone().unwrap_or_default(),
        };
        let t = Instant::now();
        let out = saves::encode_with(&save.info.header, &save.gcs1, &sections, |raw| {
            wire::compress_with(raw, |r| zstd::bulk::compress(r, 7).unwrap())
        });
        worst = worst.max(t.elapsed());
        before += bytes.len();
        after += out.len();
    }
    println!(
        "\n{} saves: {before} B -> {after} B ({:.1}x); slowest whole encode at level 7 {worst:?}",
        all.len(),
        before as f64 / after as f64
    );
}
