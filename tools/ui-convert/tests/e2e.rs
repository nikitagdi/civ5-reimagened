//! Tests with Node and Chromium. They run only with `CIV5_UI2_CHROME=1` (and Node plus the
//! Playwright install of `tools/playtest`); otherwise each prints why it skipped.
//!
//! `$env:CIV5_UI2_CHROME=1; cargo test -p ui2-convert --test e2e -- --test-threads=1`

use std::path::{Path, PathBuf};

use ui2_convert::{Input, check_skin, node};

fn chrome() -> bool {
    if std::env::var("CIV5_UI2_CHROME").ok().as_deref() != Some("1") {
        eprintln!("skipped: CIV5_UI2_CHROME is not set");
        return false;
    }
    match node::available() {
        Ok(()) => true,
        Err(why) => {
            eprintln!("skipped: {why}");
            false
        }
    }
}

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

/// Every bad mockup through the whole pipeline (`check` = both build passes, pictures, the package budget): it
/// gives exactly the codes of its `expect.txt` at their places, and its stored scan (`ir.json`, read by
/// `tests/bad.rs` without a browser) is the one this run made.
#[test]
fn bad_mockups() {
    if !chrome() {
        return;
    }
    let root = crate_dir().join("tests").join("bad");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/bad")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("expect.txt").is_file())
        .collect();
    dirs.sort();
    assert_eq!(dirs.len(), 41, "the whole set of bad mockups is present");
    let mut failures = Vec::new();
    for dir in dirs {
        let out = ui2_convert::build::build(&dir, false);
        let run = &out.run;
        let mut got: Vec<String> = run
            .errors
            .iter()
            .map(|e| match e.place.file.as_str() {
                "" => format!("{} (skin)", e.code),
                f => format!("{} {f}:{}", e.code, e.place.line),
            })
            .collect();
        got.sort();
        let text = std::fs::read_to_string(dir.join("expect.txt")).expect("expect.txt");
        let mut want: Vec<String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect();
        want.sort();
        let name = dir.file_name().map(|n| n.to_string_lossy().to_string());
        if got != want {
            failures.push(format!(
                "{name:?}: got {got:?}, expected {want:?}\n{}",
                run.errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        let stored = std::fs::read_to_string(dir.join("ir.json")).expect("ir.json");
        if run.dump().as_deref() != Some(stored.as_str()) {
            failures.push(format!(
                "{name:?}: the stored ir.json is stale; regenerate it with `ui2-convert check <dir> --dump-ir <dir>/ir.json`"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn layout_fixture_is_current() {
    if !chrome() {
        return;
    }
    let fixtures = crate_dir().join("tests").join("fixtures");
    let run = check_skin(&fixtures.join("layout"), Input::Chrome);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let stored = std::fs::read_to_string(fixtures.join("layout.ir.json")).expect("fixture");
    assert_eq!(
        run.dump().expect("dump"),
        stored,
        "layout.ir.json is stale; see tests/fixtures/README.md"
    );
}

#[test]
fn engraved_checks_run_in_chrome() {
    if !chrome() {
        return;
    }
    let dir = crate_dir()
        .join("..")
        .join("..")
        .join("skins")
        .join("engraved");
    let run = check_skin(&dir, Input::Chrome);
    assert!(run.scanned.is_some());
    // The checks ran; the codes depend on the state of the skin and the profile.
    for e in &run.errors {
        assert!(!e.code.starts_with("E9-00") || e.code == "E9-005", "{e}");
    }
}

fn engraved() -> PathBuf {
    crate_dir()
        .join("..")
        .join("..")
        .join("skins")
        .join("engraved")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for e in std::fs::read_dir(from).expect("read_dir").flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).expect("copy");
        }
    }
}

/// Two builds of the skin, and a build of a copy in another folder, give the same bytes.
#[test]
fn engraved_builds_and_the_package_is_deterministic() {
    if !chrome() {
        return;
    }
    let a = ui2_convert::build::build(&engraved(), true);
    assert!(a.run.errors.is_empty(), "{:?}", a.run.errors);
    let a = a.written.expect("package");
    let b = ui2_convert::build::build(&engraved(), true)
        .written
        .expect("package");
    assert_eq!(a.hash, b.hash);
    assert_eq!(a.bytes, b.bytes);
    let copy = std::env::temp_dir().join(format!("ui2-e2e-copy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    copy_dir(&engraved(), &copy.join("engraved"));
    let c = ui2_convert::build::build(&copy.join("engraved"), true);
    let _ = std::fs::remove_dir_all(&copy);
    assert!(c.run.errors.is_empty(), "{:?}", c.run.errors);
    assert_eq!(c.written.expect("package").bytes, a.bytes);
    assert_eq!(a.file_name, format!("engraved-{}.ui2", &a.hash[..16]));
    // The package reads back.
    let pkg = ui2_package::Package::read(std::sync::Arc::from(a.bytes.as_slice())).expect("read");
    assert_eq!(pkg.fonts().len(), 3);
    pictures_are_atlas_pages(&pkg, &a.bytes);
}

/// The cases of the text probe (`tests/fixtures/text-probe`): the `data-part` names under `probe.`. Each sits at
/// its own top inside the probe box, so a difference in one case does not move the others.
const PROBE_CASES: &[&str] = &[
    "ellipsis",
    "ellipsis-narrow",
    "hhea",
    "hhea-48",
    "typo",
    "typo-48",
    "span-weight",
    "span-kern",
    "flex-text",
    "flex-text-tall",
];

/// Cases that do not match Chromium yet, with why. They are measured and printed, not asserted.
const PROBE_KNOWN: &[(&str, &str)] = &[
    (
        "hhea",
        "a face without USE_TYPO_METRICS: the runtime takes hhea, Chromium on Windows the OS/2 win metrics",
    ),
    ("hhea-48", "as hhea"),
    (
        "flex-text",
        "text directly in a flex container is not aligned by align-items in the runtime",
    ),
    ("flex-text-tall", "as flex-text"),
];

/// The probe fonts (`make_fonts.py`) under the target folder, which `skin.toml` names relative to the
/// repository. `None` (with the reason printed) when the target folder is outside the repository.
fn probe_fonts() -> Option<()> {
    let repo = ui2_convert::fonts::repo_root();
    let out = ui2_convert::build::target_dir()
        .join("ui2")
        .join("text-probe-fonts");
    let canon = |p: &Path| std::fs::canonicalize(p).ok();
    std::fs::create_dir_all(&out).expect("mkdir");
    match (canon(&out), canon(&repo)) {
        (Some(o), Some(r)) if o == r.join("target").join("ui2").join("text-probe-fonts") => {}
        _ => {
            eprintln!("skipped: the probe fonts must be under target/ui2 of the repository");
            return None;
        }
    }
    let python = std::env::var("CIV5_PYTHON").unwrap_or_else(|_| "python".to_string());
    let status = std::process::Command::new(python)
        .arg("-E")
        .arg(
            crate_dir()
                .join("tests")
                .join("fixtures")
                .join("text-probe")
                .join("make_fonts.py"),
        )
        .arg(repo.join("third_party/fonts/fira-sans/FiraSans-Regular.ttf"))
        .arg(&out)
        .status()
        .expect("python starts");
    assert!(status.success(), "make_fonts.py failed");
    Some(())
}

/// Text cases outside the text spike, against Chromium through `verify`: the ellipsis cut, faces whose hhea and
/// typo metrics differ (with and without USE_TYPO_METRICS), a span of another weight inside a word, kerning
/// across a span, text directly in a flex container. The matching cases stay within 0.5 px at every reference
/// viewport; `font-feature-settings` (outside the profile) is E2-010 where it is written.
#[test]
fn text_probe() {
    if !chrome() || probe_fonts().is_none() {
        return;
    }
    let dir = crate_dir()
        .join("tests")
        .join("fixtures")
        .join("text-probe");
    let out = match ui2_convert::verify::verify(&dir, "probe") {
        Ok(o) => o,
        Err(e) => panic!(
            "{}",
            e.iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ),
    };
    let report = &out.report;
    assert!(
        report.runtime_errors.is_empty(),
        "{:?}",
        report.runtime_errors
    );
    let mut worst: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();
    for v in &report.viewports {
        assert!(v.mismatch.is_none(), "{:?}", v.mismatch);
        for r in &v.rows {
            // The layers and the probe box hold no case of their own.
            let Some(case) = r.part.strip_prefix("probe.") else {
                continue;
            };
            let w = worst.entry(case).or_insert(0.0);
            *w = w.max(r.delta);
        }
    }
    let mut off = Vec::new();
    for (case, d) in &worst {
        let known = PROBE_KNOWN.iter().find(|(c, _)| c == case);
        println!(
            "text probe {case}: worst {d:.3} px{}",
            known.map_or(String::new(), |(_, why)| format!(" (known: {why})"))
        );
        if known.is_none() && *d > ui2_convert::verify::TOLERANCE {
            off.push(format!("{case}: {d:.3} px"));
        }
    }
    assert_eq!(
        worst
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        PROBE_CASES.iter().copied().collect(),
        "{worst:?}"
    );
    assert!(off.is_empty(), "{off:?}\n{}", report.lines().join("\n"));

    // `font-feature-settings` is not a property of the profile: an error at the declaration.
    let copy = std::env::temp_dir().join(format!("ui2-e2e-text-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&copy);
    copy_dir(&dir, &copy);
    let css = copy.join("css").join("a.css");
    let text = std::fs::read_to_string(&css).expect("a.css");
    // After the file's last line: an empty line, the selector, the declaration.
    let line = text.lines().count() + 3;
    std::fs::write(
        &css,
        format!("{text}\n.c1 {{\n  font-feature-settings: \"smcp\";\n}}\n"),
    )
    .expect("a.css");
    let run = check_skin(&copy, Input::Chrome);
    let _ = std::fs::remove_dir_all(&copy);
    let got: Vec<String> = run
        .errors
        .iter()
        .map(|e| format!("{} {}:{} {}", e.code, e.place.file, e.place.line, e.prop))
        .collect();
    assert_eq!(
        got,
        [format!("E2-010 css/a.css:{line} font-feature-settings")],
        "{:?}",
        run.errors
    );
}

/// One HTTP GET on the loopback server: status, headers (names lower case) and body.
fn http_get(base: &str, path: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    use std::io::{Read, Write};
    let port: u16 = base
        .trim_end_matches('/')
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .expect("a port in the server URL");
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("request");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("response");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a header block");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .expect("a status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    if headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v == "chunked")
    {
        let mut out = Vec::new();
        let mut rest = &body[..];
        loop {
            let eol = rest
                .windows(2)
                .position(|w| w == b"\r\n")
                .expect("chunk size");
            let n = usize::from_str_radix(String::from_utf8_lossy(&rest[..eol]).trim(), 16)
                .expect("hex chunk size");
            if n == 0 {
                break;
            }
            out.extend_from_slice(&rest[eol + 2..eol + 2 + n]);
            rest = &rest[eol + 2 + n + 2..];
        }
        body = out;
    }
    (status, headers, body)
}

fn header<'a>(h: &'a [(String, String)], name: &str) -> Option<&'a str> {
    h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// Watch mode on a copy of the skin: the package server answers `.rev`, `.ui2` and `.errors.json` with the
/// cross-origin headers; a CSS edit gives a new `.rev` within 3 s (no client); an error keeps the package and
/// its `.rev`; a script edit builds without Chromium. The test stops the session (Node, Chromium, server).
#[test]
fn watch_cycle() {
    if !chrome() {
        return;
    }
    use std::time::{Duration, Instant};
    use ui2_convert::watch::{Change, POLL, WatchOpts, Watcher};
    let root = std::env::temp_dir().join(format!("ui2-e2e-watch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let skin = root.join("engraved");
    copy_dir(&engraved(), &skin);
    std::fs::create_dir_all(skin.join("scripts")).expect("mkdir");
    std::fs::write(skin.join("scripts").join("watch.js"), "// watch test\n").expect("script");
    let toml = std::fs::read_to_string(skin.join("skin.toml")).expect("skin.toml");
    std::fs::write(
        skin.join("skin.toml"),
        format!("{toml}\n[scripts]\nfiles = [\"scripts/watch.js\"]\n"),
    )
    .expect("skin.toml");
    let opts = WatchOpts {
        port: 0,
        out: Some(root.join("out")),
    };
    let (mut w, first) = Watcher::start(&skin, &opts).expect("the watch starts");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert!(first.built, "{:?}", first.errors);
        println!("first cycle: {} ms, stages {:?}", first.ms, first.stages);
        assert_eq!(first.rev, 1);
        let base = w.url().to_string();
        let get = |what: &str| http_get(&base, &format!("/pkg/engraved.{what}"));
        let (status, h, body) = get("rev");
        assert_eq!((status, body.as_slice()), (200, &b"1"[..]));
        assert_eq!(header(&h, "access-control-allow-origin"), Some("*"));
        assert_eq!(
            header(&h, "cross-origin-resource-policy"),
            Some("cross-origin")
        );
        let (status, _, bytes) = get("ui2");
        assert_eq!(status, 200);
        let one = ui2_package::Package::read(std::sync::Arc::from(bytes.as_slice()))
            .expect("the served package reads")
            .hash();
        assert_eq!(get("errors.json").2, b"[]\n");

        // Waits for the cycle the last edit causes.
        fn next(w: &mut Watcher, since: Instant) -> (ui2_convert::watch::Cycle, Duration) {
            loop {
                if let Some(c) = w.poll() {
                    return (c, since.elapsed());
                }
                assert!(since.elapsed() < Duration::from_secs(30), "no cycle");
                std::thread::sleep(POLL);
            }
        }
        let css = skin.join("css").join("hud.css");
        let original = std::fs::read_to_string(&css).expect("hud.css");

        // A CSS edit: a new package within 3 s.
        let t = Instant::now();
        std::fs::write(
            &css,
            format!("{original}\n.top .spacer {{\n  min-width: 1px;\n}}\n"),
        )
        .expect("edit");
        let (c, took) = next(&mut w, t);
        println!(
            "css edit: {took:?} (cycle {} ms, stages {:?})",
            c.ms, c.stages
        );
        assert!(c.built, "{:?}", c.errors);
        assert_eq!((c.change, c.rev), (Change::Full, 2));
        assert!(took <= Duration::from_secs(3), "{took:?}");
        assert_eq!(get("rev").2, b"2");
        let two = ui2_package::Package::read(std::sync::Arc::from(get("ui2").2.as_slice()))
            .expect("reads")
            .hash();
        assert_ne!(one, two);

        // An error: the package and its number stay, the errors are served.
        let t = Instant::now();
        std::fs::write(&css, format!("{original}\n.top {{\n  float: right;\n}}\n")).expect("edit");
        let (c, _) = next(&mut w, t);
        assert!(!c.built);
        assert_eq!(c.rev, 2);
        assert!(
            c.errors.iter().any(|e| e.code == "E2-010"),
            "{:?}",
            c.errors
        );
        assert_eq!(get("rev").2, b"2");
        let errors: serde_json::Value =
            serde_json::from_slice(&get("errors.json").2).expect("errors.json");
        assert_eq!(errors[0]["code"], "E2-010");
        assert_eq!(get("ui2").2, bytes_of(&w, two));

        // Fixed again, then a script edit: built without Chromium.
        let t = Instant::now();
        std::fs::write(&css, &original).expect("restore");
        let (c, _) = next(&mut w, t);
        assert!(c.built && c.rev == 3, "{:?}", c.errors);
        let t = Instant::now();
        std::fs::write(
            skin.join("scripts").join("watch.js"),
            "// watch test, edited\n",
        )
        .expect("script edit");
        let (c, took) = next(&mut w, t);
        println!("script edit: {took:?} (cycle {} ms)", c.ms);
        assert!(c.built, "{:?}", c.errors);
        assert_eq!((c.change, c.rev), (Change::Scripts, 4));
        assert_eq!(get("rev").2, b"4");
    }));
    let pkg_dir = w.pkg_dir();
    w.close();
    let _ = std::fs::remove_dir_all(&root);
    if let Err(e) = result {
        std::panic::resume_unwind(e);
    }
    assert!(pkg_dir.starts_with(&root));
}

/// The package file the watch serves, when its hash is `hash`.
fn bytes_of(w: &ui2_convert::watch::Watcher, hash: ui2_package::PackageHash) -> Vec<u8> {
    let bytes = std::fs::read(w.pkg_dir().join(format!("{}.ui2", w.id()))).expect("package file");
    let read = ui2_package::Package::read(std::sync::Arc::from(bytes.as_slice())).expect("reads");
    assert_eq!(read.hash(), hash);
    bytes
}

/// A PNG of `w x h` straight RGBA pixels (stored deflate blocks: enough for a test picture).
fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in bytes {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let mut body = tag.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend_from_slice(&crc32(&body).to_be_bytes());
    }
    let mut raw = Vec::new();
    for row in rgba.chunks_exact((w * 4) as usize) {
        raw.push(0u8); // filter: none
        raw.extend_from_slice(row);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &x in &raw {
        a = (a + u32::from(x)) % 65521;
        b = (b + a) % 65521;
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (i, blk) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = blk.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(blk);
    }
    z.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The 2 x 2 test picture: half-transparent red, green, transparent blue, white.
const DOT: [u8; 16] = [
    255, 0, 0, 128, 0, 255, 0, 255, 0, 0, 255, 0, 255, 255, 255, 255,
];

/// A minimal skin with pictures named by `url()`, written into a fresh temporary folder. `css` is the
/// stylesheet after the layer rule.
fn picture_skin(name: &str, css: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ui2-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let layout = crate_dir().join("tests").join("fixtures").join("layout");
    for sub in ["css", "images", "l10n"] {
        std::fs::create_dir_all(dir.join(sub)).expect("mkdir");
    }
    std::fs::write(
        dir.join("skin.toml"),
        format!(
            "[skin]\nid = \"{name}\"\nversion = \"0.1.0\"\ntitle = \"skin.title\"\nmodel = \"1.0\"\napi = \"1.0\"\n"
        ),
    )
    .expect("skin.toml");
    std::fs::copy(
        layout.join("css").join("tokens.css"),
        dir.join("css").join("tokens.css"),
    )
    .expect("tokens");
    std::fs::copy(
        layout.join("l10n").join("en_US.toml"),
        dir.join("l10n").join("en_US.toml"),
    )
    .expect("l10n");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\">\n  <title>Pictures</title>\n  <link rel=\"stylesheet\" href=\"css/tokens.css\">\n  <link rel=\"stylesheet\" href=\"css/a.css\">\n</head>\n<body>\n  <div class=\"layer\" data-layer=\"map\"></div>\n  <div class=\"layer\" data-layer=\"hud\">\n    <div data-slot=\"mods\"></div>\n    <div class=\"tile\"></div>\n    <div class=\"frame\"></div>\n    <div class=\"flag\"></div>\n  </div>\n  <div class=\"layer\" data-layer=\"window\"></div>\n  <div class=\"layer\" data-layer=\"tooltip\"></div>\n</body>\n</html>\n",
    )
    .expect("index.html");
    std::fs::write(
        dir.join("css").join("a.css"),
        format!(
            ".layer {{\n  position: absolute;\n  inset: 0;\n  pointer-events: none;\n}}\n{css}"
        ),
    )
    .expect("a.css");
    std::fs::write(dir.join("images").join("dot.png"), png(2, 2, &DOT)).expect("png");
    std::fs::write(
        dir.join("images").join("flag.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"6\" height=\"4\"><rect width=\"6\" height=\"4\" fill=\"#0000ff\"/></svg>\n",
    )
    .expect("svg");
    dir
}

const PICTURES_CSS: &str = ".tile {\n  width: 8px;\n  height: 8px;\n  background-image: url(\"../images/dot.png\");\n  background-size: 8px 8px;\n}\n.frame {\n  width: 12px;\n  height: 12px;\n  border-width: 2px;\n  border-style: solid;\n  border-color: transparent;\n  border-image-source: url(\"../images/dot.png\");\n  border-image-slice: 1;\n  border-image-width: 2px;\n}\n.flag {\n  width: 6px;\n  height: 4px;\n  background-image: url(../images/flag.svg);\n}\n";

/// `url()` pictures become colored sprites of their natural size at scale 1 and 2 on premultiplied RGBA pages,
/// named by their path in the skin; the style records hold that path.
#[test]
fn url_pictures_are_premultiplied_rgba_sprites() {
    if !chrome() {
        return;
    }
    use ui2_package::PageFormat;
    use ui2_package::style::BgImage;
    let dir = picture_skin("e2e-pictures", PICTURES_CSS);
    let a = ui2_convert::build::build(&dir, true);
    let b = ui2_convert::build::build(&dir, true);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(a.run.errors.is_empty(), "{:?}", a.run.errors);
    let a = a.written.expect("package");
    assert_eq!(a.bytes, b.written.expect("package").bytes, "deterministic");
    let pkg = ui2_package::Package::read(std::sync::Arc::from(a.bytes.as_slice())).expect("read");
    let images = pkg.images();
    assert_eq!(
        images
            .pages
            .iter()
            .map(|p| (p.format, p.scale))
            .collect::<Vec<_>>(),
        [(PageFormat::Rgba8, 1), (PageFormat::Rgba8, 2)]
    );
    let img = |path: &str| {
        images
            .imgs
            .iter()
            .find(|i| pkg.str(i.path) == Some(path))
            .unwrap_or_else(|| panic!("no picture {path}"))
    };
    let dot = img("images/dot.png");
    assert_eq!(dot.size, [2.0, 2.0]);
    assert!(!dot.mask);
    assert_eq!(dot.x1.rect[2..], [4, 4]);
    assert_eq!(dot.x2.expect("2x").rect[2..], [6, 6]);
    assert_eq!(img("images/flag.svg").size, [6.0, 4.0]);
    // Premultiplied: half-transparent red is (128, 0, 0, 128); transparent blue is all zero.
    let page = &images.pages[usize::from(dot.x1.page)];
    let px = pkg.blob_owned(page.blob).expect("page");
    let w = usize::from(page.size[0]);
    let [x, y, _, _] = dot.x1.rect.map(usize::from);
    let at = |cx: usize, cy: usize| {
        let i = ((y + 1 + cy) * w + x + 1 + cx) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    assert_eq!(at(0, 0), [128, 0, 0, 128]);
    assert_eq!(at(1, 0), [0, 255, 0, 255]);
    assert_eq!(at(0, 1), [0, 0, 0, 0]);
    assert_eq!(at(1, 1), [255, 255, 255, 255]);
    // The records name the pictures by path.
    let records = &pkg.styles().records;
    let named = |f: &dyn Fn(&ui2_package::style::StyleRecord) -> Option<ui2_package::StrId>| {
        records
            .iter()
            .filter_map(f)
            .filter_map(|s| pkg.str(s))
            .collect::<Vec<_>>()
    };
    let bg = named(&|r| match r.background_image.first() {
        Some(BgImage::Url(s)) => Some(*s),
        _ => None,
    });
    assert!(
        bg.contains(&"images/dot.png") && bg.contains(&"images/flag.svg"),
        "{bg:?}"
    );
    assert_eq!(named(&|r| r.border_image_source), ["images/dot.png"]);
}

/// A picture that is not a file of the skin is E1-003 at the declaration.
#[test]
fn url_pictures_outside_the_skin_are_errors_at_the_declaration() {
    if !chrome() {
        return;
    }
    let dir = picture_skin(
        "e2e-bad-pictures",
        ".tile {\n  width: 8px;\n  height: 8px;\n  background-image: url(\"../images/missing.png\");\n}\n.frame {\n  width: 8px;\n  height: 8px;\n  background-image: url(../../outside.png);\n}\n",
    );
    let run = check_skin(&dir, Input::Chrome);
    let _ = std::fs::remove_dir_all(&dir);
    let got: Vec<String> = run
        .errors
        .iter()
        .filter(|e| e.code == "E1-003")
        .map(|e| format!("{} {}:{} {}", e.code, e.place.file, e.place.line, e.prop))
        .collect();
    assert_eq!(
        got,
        [
            "E1-003 css/a.css:9 background-image",
            "E1-003 css/a.css:14 background-image"
        ],
        "{:?}",
        run.errors
    );
}

/// The shorthands `grid-template`, `grid-row`, `grid-column`, `grid-area` and `border-image` may be written;
/// Chrome expands them into the profile's longhands.
#[test]
fn grid_and_border_image_shorthands_compile() {
    if !chrome() {
        return;
    }
    let dir = picture_skin(
        "e2e-shorthands",
        ".tile {\n  display: grid;\n  grid-template: 4px 4px / 4px 4px;\n  width: 8px;\n  height: 8px;\n}\n.frame {\n  width: 12px;\n  height: 12px;\n  grid-row: 1 / 2;\n  grid-column: 1;\n  border: 2px solid transparent;\n  border-image: url(\"../images/dot.png\") 1 / 2px stretch;\n}\n.flag {\n  width: 6px;\n  height: 4px;\n  grid-area: 2 / 1 / 3 / 3;\n}\n",
    );
    let run = check_skin(&dir, Input::Chrome);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(run.errors.is_empty(), "{:?}", run.errors);
    let s = run.styles.as_ref().expect("styles");
    assert!(
        s.records
            .iter()
            .any(|r| r.grid_template_rows.len() == 2 && r.grid_template_columns.len() == 2)
    );
    assert!(s.records.iter().any(|r| {
        r.border_image_source
            .is_some_and(|p| s.strs.get(p) == "images/dot.png")
    }));
    use ui2_package::style::GridLine;
    let lines = |r: &ui2_package::style::StyleRecord| {
        [
            r.grid_row_start,
            r.grid_row_end,
            r.grid_column_start,
            r.grid_column_end,
        ]
    };
    let all: Vec<[GridLine; 4]> = s.records.iter().map(lines).collect();
    use GridLine::Line;
    assert!(
        all.contains(&[Line(2), Line(3), Line(1), Line(3)]),
        "{all:?}"
    );
    assert!(
        all.contains(&[Line(1), Line(2), Line(1), GridLine::Auto]),
        "{all:?}"
    );
}

/// A clip polygon that mixes px and % passes the parser in its reference boxes; at the element's own size it
/// can be concave (E2-022 at the element).
#[test]
fn clip_polygons_are_convex_at_the_element_size() {
    if !chrome() {
        return;
    }
    let poly = "clip-path: polygon(0 0, 100% 0, 100% 100%, 0 100%, calc(10px - 2%) 50%);";
    let dir = picture_skin(
        "e2e-clip",
        &format!(
            ".tile {{\n  width: 1000px;\n  height: 8px;\n  {poly}\n}}\n.frame {{\n  width: 12px;\n  height: 12px;\n  {poly}\n}}\n"
        ),
    );
    let run = check_skin(&dir, Input::Chrome);
    let _ = std::fs::remove_dir_all(&dir);
    let got: Vec<String> = run
        .errors
        .iter()
        .map(|e| format!("{} {}:{} {}", e.code, e.place.file, e.place.line, e.prop))
        .collect();
    assert_eq!(got, ["E2-022 index.html:14 clip-path"], "{:?}", run.errors);
}

/// The icons are mask sprites at scale 1 and 2 on A8 pages; no PNG, WebP or SVG file is inside.
fn pictures_are_atlas_pages(pkg: &ui2_package::Package, bytes: &[u8]) {
    use ui2_package::PageFormat;
    for magic in [&b"\x89PNG"[..], b"RIFF", b"<svg", b"<?xml"] {
        assert!(
            !bytes.windows(magic.len()).any(|w| w == magic),
            "{:?} in the package",
            String::from_utf8_lossy(magic)
        );
    }
    let images = pkg.images();
    assert!(images.pages.iter().all(|p| p.format == PageFormat::A8));
    assert_eq!(
        images.pages.iter().map(|p| p.scale).collect::<Vec<_>>(),
        [1, 2]
    );
    // The science flask: 16 x 16 at scale 1 (the research ring and the science yield).
    let sci = images
        .imgs
        .iter()
        .find(|i| {
            pkg.str(i.path)
                .is_some_and(|p| p.starts_with("icons/i-sci@16x16"))
        })
        .expect("science icon");
    assert!(sci.mask);
    assert_eq!(sci.x1.rect[2..], [18, 18]);
    assert_eq!(sci.x2.expect("2x").rect[2..], [34, 34]);
    let page = &images.pages[usize::from(sci.x1.page)];
    let px = pkg.blob_owned(page.blob).expect("page");
    let w = usize::from(page.size[0]);
    let [x, y, sw, sh] = sci.x1.rect.map(usize::from);
    let mut art = String::new();
    let mut covered = 0;
    for row in y..y + sh {
        for col in x..x + sw {
            let a = px[row * w + col];
            let edge = row == y || col == x || row == y + sh - 1 || col == x + sw - 1;
            assert!(!edge || a == 0, "the margin is transparent");
            covered += usize::from(a > 0);
            art.push(match a {
                0 => '.',
                1..=127 => '+',
                _ => '#',
            });
        }
        art.push('\n');
    }
    println!("{art}");
    assert!(
        covered > 30 && covered < 16 * 16,
        "{covered} pixels covered"
    );
}
