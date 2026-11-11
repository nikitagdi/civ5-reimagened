//! Font subsets: the text stack measures the subset exactly like the full font (features and the `wght`
//! axis survive), and subsetting is deterministic. Needs Python with the pinned fontTools; without it the
//! tests say so and pass.

use std::sync::Arc;

use ui2_convert::fonts::{self, FaceDecl};
use ui2_convert::{SUBSETS, errors::Place};
use ui2_text::{FaceId, NumericVariant, RunStyle, TextEngine};

fn decl(path: &str, file: &str) -> FaceDecl {
    FaceDecl {
        family: path.to_string(),
        weight: 400,
        italic: false,
        path: path.to_string(),
        file: file.to_string(),
        place: Place::default(),
    }
}

fn faces() -> Vec<FaceDecl> {
    vec![
        decl(
            "fonts/FiraSans-Regular.ttf",
            "third_party/fonts/fira-sans/FiraSans-Regular.ttf",
        ),
        decl(
            "fonts/FiraSans-Medium.ttf",
            "third_party/fonts/fira-sans/FiraSans-Medium.ttf",
        ),
        decl(
            "fonts/Alegreya.ttf",
            "third_party/fonts/alegreya/Alegreya[wght].ttf",
        ),
    ]
}

fn ranges() -> Vec<(u32, u32)> {
    let mut r: Vec<(u32, u32)> = SUBSETS
        .iter()
        .flat_map(|(_, r)| r.iter().copied())
        .collect();
    r.sort_unstable();
    r
}

fn tables(font: &[u8]) -> Vec<String> {
    let n = u16::from_be_bytes([font[4], font[5]]) as usize;
    (0..n)
        .map(|i| String::from_utf8_lossy(&font[12 + 16 * i..12 + 16 * i + 4]).to_string())
        .collect()
}

fn subsets(tag: &str) -> Option<Vec<Vec<u8>>> {
    if let Err(e) = fonts::fonttools_version() {
        println!("skipped: {e}");
        return None;
    }
    let dir = std::env::temp_dir().join(format!("ui2-convert-fonts-{tag}-{}", std::process::id()));
    let cache = dir.join("cache");
    let out = fonts::subset_all(&faces(), &ranges(), &dir.join("out"), &cache).expect("subsets");
    let _ = std::fs::remove_dir_all(&dir);
    Some(out.into_iter().map(|s| s.bytes).collect())
}

#[test]
fn fonts_subsets_measure_like_the_full_fonts() {
    let Some(subs) = subsets("measure") else {
        return;
    };
    let mut full = TextEngine::new();
    let mut sub = TextEngine::new();
    for (i, (d, s)) in faces().iter().zip(&subs).enumerate() {
        let bytes = std::fs::read(fonts::repo_root().join(&d.file)).expect("font");
        assert!(s.len() < bytes.len(), "{} got smaller", d.path);
        full.load_face(FaceId(i as u32), Arc::from(bytes))
            .expect("full");
        sub.load_face(FaceId(i as u32), Arc::from(s.clone()))
            .expect("subset");
    }
    // The variable face keeps its axis.
    let alegreya = tables(&subs[2]);
    for t in ["fvar", "gvar", "GSUB", "GPOS"] {
        assert!(alegreya.iter().any(|x| x == t), "Alegreya subset has {t}");
    }
    for t in ["GSUB", "GPOS"] {
        assert!(
            tables(&subs[0]).iter().any(|x| x == t),
            "Fira subset has {t}"
        );
    }
    let samples = [
        "Education  3 turns",
        "Astronomy next",
        "+48 312 +21 7 412/650 +31 6 turns 140 +9 4",
        "AVATAR To We Ty office affluent fjord",
        "0123456789 1/2 1,250 AD",
        "Образование — 3 хода",
        "Łódź Ærø façade naïve",
    ];
    let numerics = [
        "normal",
        "tabular-nums lining-nums",
        "proportional-nums oldstyle-nums",
        "tabular-nums oldstyle-nums",
    ];
    let mut checked = 0;
    for face in 0..3u32 {
        let weights: &[f32] = if face == 2 {
            &[400.0, 650.0, 900.0]
        } else {
            &[400.0]
        };
        for &w in weights {
            for size in [12.0, 14.0, 15.0, 22.5] {
                for n in numerics {
                    for s in samples {
                        let style = RunStyle::new(FaceId(face), size)
                            .with_wght(w)
                            .with_numeric(NumericVariant::from_css(n));
                        let a = full.measure(&style, s).expect("full");
                        let b = sub.measure(&style, s).expect("subset");
                        assert_eq!(a, b, "face {face} wght {w} {size}px {n}: {s:?}");
                        checked += 1;
                    }
                }
            }
        }
    }
    // The features change widths, so the comparison covers them.
    let style = |n: &str| RunStyle::new(FaceId(0), 14.0).with_numeric(NumericVariant::from_css(n));
    let t = sub.measure(&style("tabular-nums"), "1111").expect("tnum");
    let p = sub
        .measure(&style("proportional-nums"), "1111")
        .expect("pnum");
    assert_ne!(t, p, "tnum and pnum differ in the subset");
    let w4 = sub
        .measure(
            &RunStyle::new(FaceId(2), 14.0).with_wght(400.0),
            "Education",
        )
        .expect("400");
    let w9 = sub
        .measure(
            &RunStyle::new(FaceId(2), 14.0).with_wght(900.0),
            "Education",
        )
        .expect("900");
    assert!(w9 > w4, "the wght axis works in the subset: {w4} -> {w9}");
    println!("{checked} runs measured equal");
}

#[test]
fn fonts_subsetting_is_deterministic() {
    let (Some(a), Some(b)) = (subsets("a"), subsets("b")) else {
        return;
    };
    assert_eq!(a, b);
}
