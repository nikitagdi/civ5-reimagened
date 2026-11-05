//! The blind test (`golden blind`).
//!
//! Ten pairs of 384x384 crops of the golden captures S1 to S3, taken over the masks of land, water, forest, units and
//! cities; in every pair the original and our frame stand in a random order (the seed is written down). A page on a
//! local server (`127.0.0.1`, the standard `TcpListener`, no dependency) shows one pair at a time with the buttons
//! "original is left" and "original is right"; no metric is shown. The answers go to
//! `target/golden/blind/<date>.json` with the key. The test is passed when no more than 7 of the 10 are guessed.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::image::{Rgba8, png_bytes};
use crate::metrics::Mask;

/// Side of a crop.
pub const CROP: usize = 384;
/// Pairs in the test.
pub const PAIRS: usize = 10;
/// The test is passed when the guesses right are at most this many.
pub const PASS_AT_MOST: usize = 7;
/// Share of a crop that has to lie on the class, by class (units and cities are small).
const MIN_SHARE: [(&str, f64); 5] = [
    ("land", 0.6),
    ("water", 0.6),
    ("forest", 0.5),
    ("unit", 0.03),
    ("city", 0.08),
];
/// The classes of the pairs, in the order the pairs take them (class numbers of the class mask).
pub const CLASSES: [(&str, u8); 5] = [
    ("land", 1),
    ("water", 2),
    ("forest", 4),
    ("unit", 5),
    ("city", 6),
];

/// One capture to take crops from: the frame of the original, our frame and the class mask of ours.
pub struct Source {
    pub capture: String,
    pub reference: Rgba8,
    pub ours: Rgba8,
    /// One class byte per pixel of our frame.
    pub classes: Vec<u8>,
}

/// Where the original stands in a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// A pair of the test: the crop, its class and where the original stands. The key; never sent to the page.
pub struct Pair {
    pub capture: String,
    pub class: &'static str,
    pub x: usize,
    pub y: usize,
    pub original: Side,
    /// PNG of the left and of the right crop.
    pub left: Vec<u8>,
    pub right: Vec<u8>,
}

/// xorshift64*: small, and the same on every machine, so a seed gives the same test.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

fn crop(img: &Rgba8, x0: usize, y0: usize) -> Rgba8 {
    let mut rgba = Vec::with_capacity(CROP * CROP * 4);
    for y in y0..y0 + CROP {
        let at = (y * img.width + x0) * 4;
        rgba.extend_from_slice(&img.rgba[at..at + CROP * 4]);
    }
    Rgba8 {
        width: CROP,
        height: CROP,
        rgba,
    }
}

/// The windows of a source whose share of the eroded class mask is at least `share`: `(x, y)` on a grid of 32 pixels.
fn windows(src: &Source, class: u8, share: f64) -> Vec<(usize, usize)> {
    let (w, h) = (src.reference.width, src.reference.height);
    if w < CROP || h < CROP || src.classes.len() != w * h {
        return Vec::new();
    }
    let mask = Mask::from_class(w, h, &src.classes, class).for_metrics();
    // Summed-area table of the mask: the count in a window takes four reads.
    let stride = w + 1;
    let mut sat = vec![0u32; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(mask.bits[y * w + x]);
            sat[(y + 1) * stride + x + 1] = sat[y * stride + x + 1] + row;
        }
    }
    let need = (share * (CROP * CROP) as f64).ceil() as u32;
    let mut out = Vec::new();
    for y in (0..=h - CROP).step_by(32) {
        for x in (0..=w - CROP).step_by(32) {
            let n = sat[(y + CROP) * stride + x + CROP] + sat[y * stride + x]
                - sat[y * stride + x + CROP]
                - sat[(y + CROP) * stride + x];
            if n >= need {
                out.push((x, y));
            }
        }
    }
    out
}

/// The pairs of the test: the classes in turn, the captures in turn within a class, a random window of the source, a
/// random side for the original. A class that has no window anywhere is skipped (the others fill its place).
pub fn pick_pairs(sources: &[Source], seed: u64) -> Result<Vec<Pair>> {
    if sources.is_empty() {
        bail!("blind: no capture with a render of ours and its class mask");
    }
    let mut rng = Rng::new(seed);
    let share = |name: &str| {
        MIN_SHARE
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(0.5, |(_, s)| *s)
    };
    let mut pairs: Vec<Pair> = Vec::new();
    let mut round = 0usize;
    while pairs.len() < PAIRS {
        let before = pairs.len();
        for (name, number) in CLASSES {
            if pairs.len() >= PAIRS {
                break;
            }
            // The captures that have a window of this class, starting at the one of this round.
            for k in 0..sources.len() {
                let src = &sources[(round + k) % sources.len()];
                let found = windows(src, number, share(name));
                if found.is_empty() {
                    continue;
                }
                let (x, y) = found[rng.below(found.len())];
                let original = if rng.below(2) == 0 {
                    Side::Left
                } else {
                    Side::Right
                };
                let (a, b) = (crop(&src.reference, x, y), crop(&src.ours, x, y));
                let (left, right) = match original {
                    Side::Left => (png_bytes(&a), png_bytes(&b)),
                    Side::Right => (png_bytes(&b), png_bytes(&a)),
                };
                pairs.push(Pair {
                    capture: src.capture.clone(),
                    class: name,
                    x,
                    y,
                    original,
                    left,
                    right,
                });
                break;
            }
        }
        if pairs.len() == before {
            bail!(
                "blind: only {} of {PAIRS} pairs found: the captures have no window of 384x384 on the classes",
                pairs.len()
            );
        }
        round += 1;
    }
    // The order of the pairs is mixed too, so that the class does not give the position away.
    for i in (1..pairs.len()).rev() {
        pairs.swap(i, rng.below(i + 1));
    }
    Ok(pairs)
}

/// The answers of the page against the key: `answers[i]` is `"left"` or `"right"`, where the person thinks the original
/// stands. Returns the number of right guesses.
pub fn score(pairs: &[Pair], answers: &[String]) -> usize {
    pairs
        .iter()
        .zip(answers)
        .filter(|(p, a)| a.as_str() == p.original.name())
        .count()
}

/// Days since 1970-01-01 to the civil date (Howard Hinnant).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `2026-10-27_153045` (UTC) for a time in seconds since the epoch: the name of the answers file.
pub fn stamp(secs: u64) -> String {
    let (y, m, d) = civil((secs / 86_400) as i64);
    let t = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}_{:02}{:02}{:02}",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Blind test</title>
<style>
body{font:16px/1.4 system-ui,sans-serif;margin:0;padding:16px;background:#1b1d20;color:#e8e8e8}
main{max-width:860px;margin:0 auto}
.pair{display:flex;gap:16px;justify-content:center;flex-wrap:wrap}
.pair img{width:384px;height:384px;max-width:100%;image-rendering:auto;border:1px solid #444}
.btns{display:flex;gap:16px;justify-content:center;margin:16px 0}
button{font:inherit;padding:10px 20px;border-radius:6px;border:1px solid #666;background:#2c3036;color:inherit;cursor:pointer}
button:hover,button:focus-visible{background:#3a4048;outline:2px solid #7aa2ff}
.muted{color:#9aa;text-align:center}
</style></head><body><main>
<h1>Which one is the original game?</h1>
<p class="muted" id="count"></p>
<div class="pair"><img id="a" alt="left crop"><img id="b" alt="right crop"></div>
<div class="btns"><button id="l" type="button">Original is on the left</button>
<button id="r" type="button">Original is on the right</button></div>
<p class="muted" id="msg"></p>
</main>
<script>
(async () => {
  const info = await (await fetch("/pairs")).json();
  const answers = [];
  let at = 0;
  const $ = (id) => document.getElementById(id);
  function show() {
    if (at >= info.count) return finish();
    $("a").src = "/img/pair" + at + "-l.png";
    $("b").src = "/img/pair" + at + "-r.png";
    $("count").textContent = "Pair " + (at + 1) + " of " + info.count;
  }
  async function finish() {
    $("l").disabled = $("r").disabled = true;
    const res = await fetch("/submit", { method: "POST", body: JSON.stringify({ answers }) });
    const out = await res.json();
    $("msg").textContent = "Done. Answers saved. You can close the page.";
    $("count").textContent = "";
    $("a").remove(); $("b").remove();
    void out;
  }
  function answer(side) { answers.push(side); at += 1; show(); }
  $("l").onclick = () => answer("left");
  $("r").onclick = () => answer("right");
  document.addEventListener("keydown", (e) => {
    if (e.key === "ArrowLeft") answer("left");
    if (e.key === "ArrowRight") answer("right");
  });
  show();
})();
</script></body></html>
"#;

/// What the server gives back, and what it learned: the answers once they are submitted.
pub struct Outcome {
    pub answers: Vec<String>,
    pub correct: usize,
    pub file: PathBuf,
}

fn respond(s: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    s.write_all(body)?;
    s.flush()
}

/// One request: the method, the path and the body. Reads the head up to the empty line and `Content-Length` bytes.
fn read_request(s: &mut TcpStream) -> Result<(String, String, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 64 * 1024 {
            bail!("request head too long");
        }
        let n = s.read(&mut chunk)?;
        if n == 0 {
            bail!("connection closed in the request head");
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default().to_owned();
    let path = first.next().unwrap_or_default().to_owned();
    let length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    if length > 64 * 1024 {
        bail!("request body too long");
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < length {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Ok((method, path, body))
}

/// Serves the test on `listener` until the answers come; writes `<dir>/<stamp>.json`.
pub fn serve(
    listener: &TcpListener,
    pairs: &[Pair],
    seed: u64,
    dir: &Path,
    stamp_text: &str,
) -> Result<Outcome> {
    loop {
        let (mut s, peer) = listener.accept()?;
        if !peer.ip().is_loopback() {
            continue;
        }
        s.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        let Ok((method, path, body)) = read_request(&mut s) else {
            continue;
        };
        let sent = match (method.as_str(), path.as_str()) {
            ("GET", "/" | "/index.html") => respond(
                &mut s,
                "200 OK",
                "text/html; charset=utf-8",
                PAGE.as_bytes(),
            ),
            ("GET", "/pairs") => respond(
                &mut s,
                "200 OK",
                "application/json",
                json!({"count": pairs.len()}).to_string().as_bytes(),
            ),
            ("GET", p) if p.starts_with("/img/pair") => {
                // `/img/pair<N>-l.png`: the number and the side, nothing else is read from the path.
                let png = p
                    .strip_prefix("/img/pair")
                    .and_then(|r| r.strip_suffix(".png"))
                    .and_then(|r| r.split_once('-'))
                    .and_then(|(n, side)| {
                        let pair = pairs.get(n.parse::<usize>().ok()?)?;
                        match side {
                            "l" => Some(&pair.left),
                            "r" => Some(&pair.right),
                            _ => None,
                        }
                    });
                match png {
                    Some(b) => respond(&mut s, "200 OK", "image/png", b),
                    None => respond(&mut s, "404 Not Found", "text/plain", b"no such image"),
                }
            }
            ("POST", "/submit") => {
                let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let answers: Vec<String> = v["answers"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                if answers.len() != pairs.len()
                    || answers.iter().any(|a| a != "left" && a != "right")
                {
                    respond(
                        &mut s,
                        "400 Bad Request",
                        "text/plain",
                        b"need one answer per pair",
                    )?;
                    continue;
                }
                let correct = score(pairs, &answers);
                let file = dir.join(format!("{stamp_text}.json"));
                std::fs::create_dir_all(dir)?;
                let key: Vec<Value> = pairs
                    .iter()
                    .zip(&answers)
                    .enumerate()
                    .map(|(i, (p, a))| {
                        json!({
                            "pair": i,
                            "capture": p.capture,
                            "class": p.class,
                            "x": p.x,
                            "y": p.y,
                            "original": p.original.name(),
                            "answer": a,
                            "right": a == p.original.name(),
                        })
                    })
                    .collect();
                let report = json!({
                    "seed": seed,
                    "crop": CROP,
                    "pairs": pairs.len(),
                    "correct": correct,
                    "passed": correct <= PASS_AT_MOST,
                    "pass_rule": format!("passed when at most {PASS_AT_MOST} of {PAIRS} are guessed"),
                    "key": key,
                });
                std::fs::write(&file, serde_json::to_string_pretty(&report)?)?;
                respond(
                    &mut s,
                    "200 OK",
                    "application/json",
                    json!({"saved": true}).to_string().as_bytes(),
                )?;
                return Ok(Outcome {
                    answers,
                    correct,
                    file,
                });
            }
            _ => respond(&mut s, "404 Not Found", "text/plain", b"not found"),
        };
        let _ = sent;
    }
}

/// Binds `127.0.0.1:port` (0 for any free port): the test never listens on another address.
pub fn bind(port: u16) -> Result<(TcpListener, SocketAddr)> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .with_context(|| format!("cannot listen on 127.0.0.1:{port}"))?;
    let addr = l.local_addr()?;
    Ok((l, addr))
}

/// A frame of `w` x `h` with a class mask of bands: land on the left, water, forest, units and cities in strips,
/// and a different picture for the original and for ours.
pub fn synthetic_source(name: &str, w: usize, h: usize) -> Source {
    let mut classes = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            classes[y * w + x] = match x * 5 / w {
                0 => 1,
                1 => 2,
                2 => 4,
                3 => 5,
                _ => 6,
            };
        }
    }
    let paint = |bias: u8| Rgba8 {
        width: w,
        height: h,
        rgba: (0..w * h)
            .flat_map(|i| [(i % 251) as u8, bias, (i / w) as u8, 255])
            .collect(),
    };
    Source {
        capture: name.into(),
        reference: paint(10),
        ours: paint(200),
        classes,
    }
}

fn get(addr: SocketAddr, path: &str) -> Result<(String, Vec<u8>)> {
    let mut s = TcpStream::connect(addr)?;
    write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n")?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)?;
    split(out)
}

fn post(addr: SocketAddr, path: &str, body: &str) -> Result<(String, Vec<u8>)> {
    let mut s = TcpStream::connect(addr)?;
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)?;
    split(out)
}

fn split(out: Vec<u8>) -> Result<(String, Vec<u8>)> {
    let i = out
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("no head in the response")?;
    let head = String::from_utf8_lossy(&out[..i]).into_owned();
    let status = head.lines().next().unwrap_or_default().to_owned();
    Ok((status, out[i + 4..].to_vec()))
}

/// The self-test of `golden blind --self-test`: synthetic captures, the server on a free port of `127.0.0.1`, a client
/// that reads the page, the pairs and an image, is refused a short answer, and submits all ten answers right; then the
/// file with the key is checked. Returns a line for the log.
pub fn self_test(dir: &Path) -> Result<String> {
    let sources = [
        synthetic_source("S1a", 2400, 400),
        synthetic_source("S2", 2400, 400),
    ];
    let pairs = pick_pairs(&sources, 11)?;
    let key: Vec<String> = pairs.iter().map(|p| p.original.name().into()).collect();
    let (l, addr) = bind(0)?;
    if !addr.ip().is_loopback() {
        bail!("the server listens on {addr}, not on 127.0.0.1");
    }
    let d = dir.to_path_buf();
    let server = std::thread::spawn(move || serve(&l, &pairs, 11, &d, "self-test"));
    let check = |ok: bool, what: &str| {
        if ok {
            Ok(())
        } else {
            Err(anyhow::anyhow!("self-test: {what}"))
        }
    };
    let (status, page) = get(addr, "/")?;
    check(
        status.contains("200")
            && String::from_utf8_lossy(&page).contains("Original is on the left"),
        "the page",
    )?;
    let (_, info) = get(addr, "/pairs")?;
    let info = String::from_utf8_lossy(&info).into_owned();
    check(
        !info.contains("original") && !info.contains("flip"),
        "the pairs list shows the key or a metric",
    )?;
    let (status, img) = get(addr, "/img/pair0-l.png")?;
    check(
        status.contains("200") && img.starts_with(&[0x89, b'P', b'N', b'G']),
        "an image",
    )?;
    check(
        get(addr, "/img/pair99-l.png")?.0.contains("404"),
        "an image out of range is served",
    )?;
    check(
        get(addr, "/img/../secret")?.0.contains("404"),
        "a path outside the images is served",
    )?;
    check(
        post(addr, "/submit", "{\"answers\":[\"left\"]}")?
            .0
            .contains("400"),
        "a short answer is taken",
    )?;
    let answers = serde_json::to_string(&json!({ "answers": key }))?;
    check(
        post(addr, "/submit", &answers)?.0.contains("200"),
        "the submission",
    )?;
    let out = server
        .join()
        .map_err(|_| anyhow::anyhow!("self-test: the server thread panicked"))??;
    check(out.correct == PAIRS, "the guesses are not counted")?;
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&out.file)?)?;
    check(
        saved["seed"] == 11 && saved["correct"] == PAIRS,
        "the file has no seed or score",
    )?;
    check(saved["passed"] == false, "ten right answers pass the test")?;
    check(
        saved["key"].as_array().is_some_and(|k| k.len() == PAIRS),
        "the file has no key",
    )?;
    Ok(format!(
        "blind self-test: ok ({PAIRS} pairs, answers file {})",
        out.file.display()
    ))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_test_and_ten_pairs_over_all_classes() {
        let sources = [
            synthetic_source("S1a", 2400, 400),
            synthetic_source("S3", 2400, 400),
        ];
        let a = pick_pairs(&sources, 42).unwrap();
        let b = pick_pairs(&sources, 42).unwrap();
        assert_eq!(a.len(), PAIRS);
        assert_eq!(
            a.iter().map(|p| (p.x, p.y, p.original)).collect::<Vec<_>>(),
            b.iter().map(|p| (p.x, p.y, p.original)).collect::<Vec<_>>()
        );
        for (class, _) in CLASSES {
            assert!(
                a.iter().filter(|p| p.class == class).count() >= 1,
                "{class}"
            );
        }
        assert!(a.iter().any(|p| p.original == Side::Left));
        assert!(a.iter().any(|p| p.original == Side::Right));
        let c = pick_pairs(&sources, 43).unwrap();
        assert_ne!(
            a.iter().map(|p| (p.x, p.y, p.original)).collect::<Vec<_>>(),
            c.iter().map(|p| (p.x, p.y, p.original)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn crops_lie_on_their_class() {
        let src = synthetic_source("S1a", 2400, 400);
        // `land` is the first fifth of the width, 480 px: a window needs 60% of its width on it.
        let land = windows(&src, 1, 0.6);
        assert!(!land.is_empty());
        for (x, _) in land {
            assert!(x + (0.6 * CROP as f64) as usize <= 480 + 3, "{x}");
        }
        assert!(windows(&src, 5, 0.03).iter().all(|&(x, _)| x + CROP > 960));
    }

    #[test]
    fn too_small_a_capture_is_an_error() {
        assert!(pick_pairs(&[synthetic_source("S1a", 300, 300)], 1).is_err());
        assert!(pick_pairs(&[], 1).is_err());
    }

    #[test]
    fn the_side_of_the_original_is_in_the_crops() {
        let sources = [synthetic_source("S1a", 2400, 400)];
        for p in pick_pairs(&sources, 7).unwrap() {
            // The reference is painted with green 10, ours with 200: the crop on the original's side has the 10.
            let (orig, ours) = match p.original {
                Side::Left => (&p.left, &p.right),
                Side::Right => (&p.right, &p.left),
            };
            assert_eq!(green(orig), 10);
            assert_eq!(green(ours), 200);
        }
    }

    /// The green byte of the first pixel of a PNG of `png_bytes`: 8 bytes of signature, IHDR (25), the IDAT head (8),
    /// the zlib head (2), the head of the stored block (5), the filter byte (1), then red, green.
    fn green(png: &[u8]) -> u8 {
        png[8 + 25 + 8 + 2 + 5 + 1 + 1]
    }

    #[test]
    fn score_counts_the_guesses_that_match_the_key() {
        let sources = [synthetic_source("S1a", 2400, 400)];
        let pairs = pick_pairs(&sources, 3).unwrap();
        let all_right: Vec<String> = pairs.iter().map(|p| p.original.name().into()).collect();
        assert_eq!(score(&pairs, &all_right), PAIRS);
        let all_wrong: Vec<String> = pairs
            .iter()
            .map(|p| match p.original {
                Side::Left => "right".into(),
                Side::Right => "left".into(),
            })
            .collect();
        assert_eq!(score(&pairs, &all_wrong), 0);
    }

    #[test]
    fn stamp_is_a_utc_date_and_time() {
        assert_eq!(stamp(0), "1970-01-01_000000");
        assert_eq!(stamp(1_788_000_000), "2026-08-29_104000");
        assert_eq!(stamp(951_782_400 + 3661), "2000-02-29_010101");
    }

    /// `self_test` is the whole test against the server on a free port (it is also `golden blind --self-test`).
    #[test]
    fn server_serves_the_pairs_and_keeps_the_answers() {
        let dir = std::env::temp_dir().join(format!("golden-blind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let line = self_test(&dir).unwrap();
        assert!(line.contains("10 pairs"), "{line}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
