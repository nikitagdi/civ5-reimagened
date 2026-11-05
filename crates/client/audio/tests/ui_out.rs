//! Against a real `build --set ui` output: every sound file of the set decodes and the table
//! plays. Runs only when `CIV5_UI_OUT` names the output folder.

use std::path::PathBuf;

use asset_format::{AudioTable, LogicalId, Manifest};
use audio::{PlayOutcome, UiAudio, decode_wav};
use platform::NullOutput;

fn out_dir() -> Option<PathBuf> {
    std::env::var_os("CIV5_UI_OUT").map(PathBuf::from)
}

fn blob(dir: &std::path::Path, m: &Manifest, id: &str) -> Vec<u8> {
    let v = &m.variants(&LogicalId::new(id))[0];
    std::fs::read(dir.join(v.hash.to_string())).unwrap()
}

#[test]
fn every_sound_file_decodes_and_policy_plays() {
    let Some(dir) = out_dir() else {
        eprintln!("CIV5_UI_OUT not set: skipped");
        return;
    };
    let ptr = std::fs::read_to_string(dir.join("manifest")).unwrap();
    let m = Manifest::decode(&std::fs::read(dir.join(ptr.trim())).unwrap()).unwrap();
    let table = AudioTable::decode(&blob(&dir, &m, "ui:snd")).unwrap();
    let mut files = 0;
    for s in &table.sounds {
        for f in &s.files {
            let bytes = blob(&dir, &m, f);
            if bytes.starts_with(b"RIFF") {
                let pcm = decode_wav(&bytes).unwrap_or_else(|e| panic!("{f}: {e}"));
                assert!(pcm.frames() > 0, "{f} is empty");
            }
            files += 1;
        }
    }
    assert!(files > 20, "only {files} files");

    let mut a = UiAudio::new(table, NullOutput::running(), 1);
    assert_eq!(a.play_2d(0, "AS2D_INTERFACE_POLICY"), PlayOutcome::Pending);
    for f in a.wanted_files() {
        a.on_file_loaded(10, &f, &blob(&dir, &m, &f)).unwrap();
    }
    let played = &a.output().played;
    assert_eq!(played.len(), 1);
    // iMinVolume = iMaxVolume = 45.
    assert!((played[0].2 - 0.45).abs() < 1e-6);
}
