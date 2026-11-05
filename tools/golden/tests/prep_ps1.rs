//! Tests of `capture/prep.ps1` on synthetic ini copies in a temp dir (never the user's files).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Output};

const CFG: &str = "config.ini";
const GFX: &str = "GraphicsSettingsDX11.ini";

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Windows PowerShell 5.1 if there (the user runs it), else PowerShell 7.
fn powershell() -> Option<&'static str> {
    ["powershell", "pwsh"].into_iter().find(|exe| {
        Command::new(exe)
            .args(["-NoProfile", "-Command", "exit 0"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

struct Env {
    root: PathBuf,
    docs: PathBuf,
    cache: PathBuf,
}

impl Env {
    fn ini_dir(&self) -> PathBuf {
        self.docs
            .join("My Games")
            .join("Sid Meier's Civilization 5")
    }
    fn backup(&self, f: &str) -> PathBuf {
        self.cache.join("captures").join("ini-backup").join(f)
    }
    fn read(&self, f: &str) -> Vec<u8> {
        std::fs::read(self.ini_dir().join(f)).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn fixture(name: &str, crlf: bool) -> Vec<u8> {
    let text = std::fs::read_to_string(crate_dir().join("tests/fixtures").join(name))
        .unwrap()
        .replace("\r\n", "\n");
    if crlf {
        text.replace('\n', "\r\n")
    } else {
        text
    }
    .into_bytes()
}

fn make_env(test: &str, crlf: bool) -> Env {
    let root = std::env::temp_dir().join(format!("golden-prep-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let env = Env {
        docs: root.join("Documents"),
        cache: root.join("cache"),
        root,
    };
    std::fs::create_dir_all(env.ini_dir()).unwrap();
    std::fs::write(env.ini_dir().join(CFG), fixture(CFG, crlf)).unwrap();
    std::fs::write(env.ini_dir().join(GFX), fixture(GFX, crlf)).unwrap();
    // A neighbour that prep must not touch.
    std::fs::write(env.ini_dir().join("UserSettings.ini"), b"[X]\r\nA = 1\r\n").unwrap();
    env
}

fn run(env: &Env, args: &[&str]) -> Output {
    let exe = powershell().expect("no PowerShell");
    Command::new(exe)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(crate_dir().join("capture/prep.ps1"))
        .args(args)
        .arg("-MyDocs")
        .arg(&env.docs)
        .arg("-CacheDir")
        .arg(&env.cache)
        .output()
        .unwrap()
}

fn prep(env: &Env, scene: &str) {
    let o = run(env, &["-Scene", scene]);
    assert!(
        o.status.success(),
        "prep {scene} failed: {}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
}

fn restore(env: &Env) {
    let o = run(env, &["-Restore"]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

/// (section, key) -> (line index, value). Lines of the form `key = value` outside comments.
type IniMap = BTreeMap<(String, String), (usize, String)>;

fn parse(bytes: &[u8]) -> (Vec<String>, IniMap) {
    let text: String = bytes.iter().map(|&b| b as char).collect();
    let lines: Vec<String> = text.split_inclusive('\n').map(str::to_string).collect();
    let mut map = BTreeMap::new();
    let mut sec = String::new();
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim();
        if t.starts_with('[') && t.ends_with(']') {
            sec = t[1..t.len() - 1].to_string();
        } else if !t.starts_with(';')
            && let Some((k, v)) = t.split_once('=')
        {
            map.insert(
                (sec.clone(), k.trim().to_string()),
                (i, v.trim().to_string()),
            );
        }
    }
    (lines, map)
}

/// `None` = same as before (S3 seeds).
fn table(
    scene: &str,
) -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    let seed = if scene == "S3" { None } else { Some("12345") };
    let fow = if scene == "S2" { "1" } else { "0" };
    vec![
        (GFX, "UserSettings", "FullScreen", Some("0")),
        (GFX, "UserSettings", "WindowResX", Some("1920")),
        (GFX, "UserSettings", "WindowResY", Some("1080")),
        (GFX, "UserSettings", "MSAASamples", Some("4")),
        (CFG, "Debug", "ThreadingMode", Some("1")),
        (CFG, "User Settings", "Threaded Submission", Some("0")),
        (CFG, "Debug", "LoggingEnabled", Some("1")),
        (CFG, "CONFIG", "SyncRandSeed", seed),
        (CFG, "CONFIG", "MapRandSeed", seed),
        (CFG, "DisableSystems", "FOW", Some(fow)),
        (CFG, "DisableSystems", "FluidFOW", Some(fow)),
    ]
}

fn skip() -> bool {
    if powershell().is_none() {
        eprintln!("skipped: no PowerShell on this machine");
        return true;
    }
    false
}

#[test]
fn changes_only_the_keys_of_table_4_2() {
    if skip() {
        return;
    }
    for crlf in [true, false] {
        for scene in ["S1", "S2", "S3", "S4"] {
            let env = make_env(&format!("keys-{scene}-{crlf}"), crlf);
            let before = [(CFG, env.read(CFG)), (GFX, env.read(GFX))];
            prep(&env, scene);
            for (file, old) in &before {
                let new = env.read(file);
                let (old_lines, old_map) = parse(old);
                let (new_lines, new_map) = parse(&new);
                assert_eq!(
                    old_lines.len(),
                    new_lines.len(),
                    "{scene} {file}: line count"
                );
                let mut touched = Vec::new();
                for (i, (a, b)) in old_lines.iter().zip(&new_lines).enumerate() {
                    if a != b {
                        touched.push(i);
                    }
                }
                let mut expected_lines = Vec::new();
                for (f, sec, key, val) in table(scene) {
                    if f != *file {
                        continue;
                    }
                    let k = (sec.to_string(), key.to_string());
                    let want = val
                        .map(str::to_string)
                        .unwrap_or_else(|| old_map[&k].1.clone());
                    assert_eq!(new_map[&k].1, want, "{scene} {file} [{sec}] {key}");
                    if old_map[&k].1 != want {
                        expected_lines.push(old_map[&k].0);
                    }
                }
                expected_lines.sort();
                assert_eq!(
                    touched, expected_lines,
                    "{scene} {file}: only table keys change"
                );
                // Line endings of every line are kept.
                if !crlf {
                    assert!(!new.contains(&b'\r'));
                }
            }
            assert_eq!(
                std::fs::read(env.ini_dir().join("UserSettings.ini")).unwrap(),
                b"[X]\r\nA = 1\r\n"
            );
        }
    }
}

#[test]
fn decoy_keys_in_other_sections_stay() {
    if skip() {
        return;
    }
    let env = make_env("decoy", true);
    prep(&env, "S1");
    let (_, m) = parse(&env.read(CFG));
    assert_eq!(m[&("Debugging".into(), "ThreadingMode".into())].1, "9");
    assert_eq!(m[&("Debugging".into(), "LoggingEnabled".into())].1, "9");
    let (_, g) = parse(&env.read(GFX));
    assert_eq!(
        g[&("LeaderheadSettings".into(), "FullScreen".into())].1,
        "7"
    );
}

#[test]
fn restore_returns_bytes_exactly() {
    if skip() {
        return;
    }
    for crlf in [true, false] {
        let env = make_env(&format!("restore-{crlf}"), crlf);
        let (c0, g0) = (env.read(CFG), env.read(GFX));
        prep(&env, "S2");
        assert_ne!(env.read(CFG), c0);
        restore(&env);
        assert_eq!(env.read(CFG), c0);
        assert_eq!(env.read(GFX), g0);
    }
}

#[test]
fn second_prep_keeps_the_backup() {
    if skip() {
        return;
    }
    let env = make_env("twice", true);
    let (c0, g0) = (env.read(CFG), env.read(GFX));
    prep(&env, "S1");
    let s1 = (env.read(CFG), env.read(GFX));
    prep(&env, "S1");
    assert_eq!(
        (env.read(CFG), env.read(GFX)),
        s1,
        "same scene twice is idempotent"
    );
    prep(&env, "S2");
    prep(&env, "S3");
    assert_eq!(std::fs::read(env.backup(CFG)).unwrap(), c0);
    assert_eq!(std::fs::read(env.backup(GFX)).unwrap(), g0);
    // S3 puts the seeds back to the values of the backup.
    let (_, m) = parse(&env.read(CFG));
    assert_eq!(m[&("CONFIG".into(), "SyncRandSeed".into())].1, "0");
    assert_eq!(m[&("CONFIG".into(), "MapRandSeed".into())].1, "777");
    restore(&env);
    assert_eq!((env.read(CFG), env.read(GFX)), (c0, g0));
}

#[test]
fn missing_key_is_added_in_its_section_and_restored() {
    if skip() {
        return;
    }
    let env = make_env("missing", true);
    let text = String::from_utf8(env.read(CFG)).unwrap();
    let cut: String = text
        .split_inclusive('\n')
        .filter(|l| !l.starts_with("Threaded Submission"))
        .collect();
    std::fs::write(env.ini_dir().join(CFG), cut.as_bytes()).unwrap();
    prep(&env, "S1");
    let (_, m) = parse(&env.read(CFG));
    assert_eq!(
        m[&("User Settings".into(), "Threaded Submission".into())].1,
        "0"
    );
    assert_eq!(
        m[&("User Settings".into(), "SomethingElse".into())].1,
        "keep"
    );
    restore(&env);
    assert_eq!(env.read(CFG), cut.as_bytes());
}

#[test]
fn creates_capture_folders_and_never_touches_other_folders() {
    if skip() {
        return;
    }
    let env = make_env("game", true);
    let game = env.root.join("game");
    std::fs::create_dir_all(&game).unwrap();
    std::fs::write(game.join("CivilizationV_DX11.exe"), b"x").unwrap();
    prep(&env, "S2");
    restore(&env);
    for s in ["S1", "S2", "S3", "S4"] {
        assert!(env.cache.join("captures").join(s).is_dir());
    }
    assert_eq!(
        std::fs::read(game.join("CivilizationV_DX11.exe")).unwrap(),
        b"x"
    );
    assert_eq!(std::fs::read_dir(&game).unwrap().count(), 1);
    // The script never names the install folder.
    let script = std::fs::read_to_string(crate_dir().join("capture/prep.ps1")).unwrap();
    assert!(!script.contains("SteamLibrary") && !script.contains("CivilizationV"));
}

#[test]
fn refuses_bad_calls() {
    if skip() {
        return;
    }
    let env = make_env("bad", true);
    // Restore without a backup.
    assert!(!run(&env, &["-Restore"]).status.success());
    // Neither or both modes.
    assert!(!run(&env, &[]).status.success());
    assert!(!run(&env, &["-Scene", "S1", "-Restore"]).status.success());
    // A Steam library path as the Documents folder.
    let bad = Env {
        root: env.root.clone(),
        docs: env.root.join("steamapps").join("common"),
        cache: env.cache.clone(),
    };
    std::fs::create_dir_all(bad.ini_dir()).unwrap();
    std::fs::write(bad.ini_dir().join(CFG), b"[A]\r\n").unwrap();
    std::fs::write(bad.ini_dir().join(GFX), b"[A]\r\n").unwrap();
    let o = run(&bad, &["-Scene", "S1"]);
    assert!(!o.status.success());
    std::mem::forget(bad); // `env` removes the shared root
}
