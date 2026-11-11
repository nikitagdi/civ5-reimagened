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

#[test]
fn bad_mockup_scans_are_current() {
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
    for dir in dirs {
        let run = check_skin(&dir, Input::Chrome);
        let fresh = run.dump().expect("the scan ran");
        let stored = std::fs::read_to_string(dir.join("ir.json")).expect("ir.json");
        assert_eq!(
            fresh,
            stored,
            "{}: the stored ir.json is stale; regenerate it with `ui2-convert check <dir> --dump-ir <dir>/ir.json`",
            dir.display()
        );
    }
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
