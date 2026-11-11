//! Pictures of a skin: icons of the sprite, inline SVG pictures and image files, rasterized by Chromium at
//! device scale 1 and 2 and packed into atlas pages (`atlas.rs`). The package holds no PNG, WebP or SVG.
//!
//! - An icon is an `<svg>` whose content is one `<use href="#id">` of a `<symbol>` of the page. Its picture is
//!   the symbol drawn at the svg's CSS size with the svg's computed paint (`fill stroke stroke-width
//!   stroke-linecap stroke-linejoin`). When every paint that is not `none` is the element's own color
//!   (`currentColor`) in every variant, the icon is a mask: drawn white, kept as alpha (A8) and painted with the
//!   node's color by the runtime; a paint that is a fixed color in every variant makes a colored picture
//!   (RGBA); a mix is an error (E5-005).
//! - An inline `<svg>` without `<use>` with a non-empty box is a static picture (RGBA) of its own markup.
//! - An `<img src>` of the skin is a picture (RGBA) of its CSS size; `name@2x.ext` next to the file is the
//!   scale 2 source.
//! - A picture a style names by `url()` (`background-image`, `border-image-source`) is a picture (RGBA) of
//!   its natural size, named by its path in the skin (the string the style record holds); the runtime
//!   scales it as the style says. `name@2x.ext` is again the scale 2 source; without one the file is drawn
//!   at twice its natural size.
//!
//! The size is the element's CSS width and height in px (the same in every variant and cell); the device size
//! is rounded up to whole pixels, so a sprite starts on the device pixel grid. RGBA pages hold premultiplied
//! alpha. Identical pictures (same source, size and paint) are one picture.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use ui2_package::style::{BgImage, ColorV, Len, LenAuto, Paint, StyleRecord};

use crate::errors::{ConvError, codes};
use crate::ir::{Dom, Element, Node};
use crate::node::NodeClient;
use crate::skin::is_skin_path;
use crate::styles::Compiled;
use crate::write::{Picture, Pictures};

/// What a picture is drawn from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    /// SVG markup, without the size attributes of the root (`{W}` and `{H}` mark where they go).
    Svg(String),
    /// A file of the skin and its scale 2 file.
    File(String, String),
}

/// One picture to rasterize.
#[derive(Debug, Clone)]
struct Want {
    path: String,
    source: Source,
    size: [f32; 2],
    mask: bool,
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The markup of an element's children (SVG content as the page holds it).
fn children_markup(e: &Element, out: &mut String) {
    for c in &e.children {
        match c {
            Node::Text(t) => out.push_str(&escape(&t.text)),
            Node::Element(k) => {
                out.push('<');
                out.push_str(&k.tag);
                for (n, v) in &k.attrs {
                    if n == "data-ui2-src" {
                        continue;
                    }
                    out.push_str(&format!(" {n}=\"{}\"", escape(v)));
                }
                out.push('>');
                children_markup(k, out);
                out.push_str(&format!("</{}>", k.tag));
            }
        }
    }
}

fn px(l: &LenAuto) -> Option<f32> {
    match l {
        LenAuto::Len(Len { px, pct }) if *pct == 0.0 => Some(*px),
        _ => None,
    }
}

/// The records of every variant and cell of an element.
fn records(c: &Compiled, id: u32) -> Vec<&StyleRecord> {
    let Some(set) = c.set_of(id) else {
        return Vec::new();
    };
    set.table
        .iter()
        .filter_map(|s| c.records.get(s.index()))
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PaintKind {
    None,
    Current,
    Fixed(ColorV),
}

fn paint_kind(p: &Paint, color: &ColorV) -> PaintKind {
    match p {
        Paint::None => PaintKind::None,
        Paint::Color(c) if c == color => PaintKind::Current,
        Paint::Color(c) => PaintKind::Fixed(*c),
    }
}

fn paint_attr(k: PaintKind, tokens: &ui2_package::Tokens) -> String {
    match k {
        PaintKind::None => "none".into(),
        PaintKind::Current => "#fff".into(),
        PaintKind::Fixed(c) => {
            let [r, g, b, a] = tokens.resolve(0, c);
            format!("rgba({r},{g},{b},{})", f32::from(a) / 255.0)
        }
    }
}

fn err(e: &Element, why: String) -> ConvError {
    ConvError::new(codes::IMAGE, e.place(), why).element(e.describe())
}

/// The size of an element in every variant; an error when it varies or is not in px.
fn size_of(c: &Compiled, e: &Element) -> Result<Option<[f32; 2]>, ConvError> {
    let recs = records(c, e.id);
    let Some(first) = recs.first() else {
        return Ok(None);
    };
    let size = match (px(&first.width), px(&first.height)) {
        (Some(w), Some(h)) => [w, h],
        _ => {
            return Err(err(
                e,
                "a picture needs a width and a height in px".to_string(),
            ));
        }
    };
    if recs
        .iter()
        .any(|r| px(&r.width) != Some(size[0]) || px(&r.height) != Some(size[1]))
    {
        return Err(err(
            e,
            "the size of a picture changes with a condition or a breakpoint".to_string(),
        ));
    }
    Ok(Some(size))
}

fn svg_root(size_attrs: bool, view_box: Option<&str>, paint: &str, body: &str) -> String {
    let mut s = String::from("<svg xmlns=\"http://www.w3.org/2000/svg\"");
    if size_attrs {
        s.push_str(" width=\"{W}\" height=\"{H}\"");
    }
    if let Some(vb) = view_box {
        s.push_str(&format!(" viewBox=\"{}\"", escape(vb)));
    }
    s.push_str(paint);
    s.push('>');
    s.push_str(body);
    s.push_str("</svg>");
    s
}

/// The pictures the page needs, and which element shows which.
fn wanted(dom: &Dom, c: &Compiled) -> (Vec<Want>, BTreeMap<u32, usize>, Vec<ConvError>) {
    let mut wants: Vec<Want> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    let mut of: BTreeMap<u32, usize> = BTreeMap::new();
    let mut errs = Vec::new();
    let symbols: BTreeMap<&str, &Element> = dom
        .elems
        .iter()
        .filter(|e| e.tag == "symbol")
        .filter_map(|e| e.attr("id").map(|id| (id, *e)))
        .collect();
    let mut add = |w: Want, id: u32, wants: &mut Vec<Want>| {
        let at = *index.entry(w.path.clone()).or_insert_with(|| {
            wants.push(w);
            wants.len() - 1
        });
        of.insert(id, at);
    };
    for &e in &dom.elems {
        if !c.node_set.contains_key(&e.id) {
            continue;
        }
        if e.tag == "svg" && e.is_svg() {
            let uses: Vec<&Element> = e.elements().filter(|k| k.tag == "use").collect();
            let size = match size_of(c, e) {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(x) => {
                    errs.push(x);
                    continue;
                }
            };
            if size[0] <= 0.0 || size[1] <= 0.0 {
                // A box with nothing to show (the sprite that holds the symbols).
                continue;
            }
            if uses.is_empty() {
                let mut body = String::new();
                children_markup(e, &mut body);
                let vb = e.attr("viewBox");
                let path = format!("svg/{}@{}x{}", e.place(), size[0], size[1]);
                add(
                    Want {
                        path,
                        source: Source::Svg(svg_root(true, vb, "", &body)),
                        size,
                        mask: false,
                    },
                    e.id,
                    &mut wants,
                );
                continue;
            }
            let href = uses[0]
                .attr("href")
                .or_else(|| uses[0].attr("xlink:href"))
                .unwrap_or("");
            let Some(sym) = href.strip_prefix('#').and_then(|id| symbols.get(id)) else {
                errs.push(err(e, format!("`{href}` is not a symbol of the page")));
                continue;
            };
            // Paint: one kind per paint over all variants.
            let recs = records(c, e.id);
            let kinds = |f: &dyn Fn(&StyleRecord) -> PaintKind| -> Option<PaintKind> {
                let first = f(recs[0]);
                recs.iter().all(|r| f(r) == first).then_some(first)
            };
            let fill = kinds(&|r| paint_kind(&r.fill, &r.color));
            let stroke = kinds(&|r| paint_kind(&r.stroke, &r.color));
            let (Some(fill), Some(stroke)) = (fill, stroke) else {
                errs.push(err(
                    e,
                    "the icon's fill or stroke changes between currentColor and a fixed color"
                        .to_string(),
                ));
                continue;
            };
            let stroke_fixed = recs.iter().all(|r| {
                r.stroke_width == recs[0].stroke_width
                    && r.stroke_linecap == recs[0].stroke_linecap
                    && r.stroke_linejoin == recs[0].stroke_linejoin
            });
            if !stroke_fixed {
                errs.push(err(
                    e,
                    "the icon's stroke width, cap or join changes with a condition".to_string(),
                ));
                continue;
            }
            let current = [fill, stroke].contains(&PaintKind::Current);
            let fixed = [fill, stroke]
                .iter()
                .any(|k| matches!(k, PaintKind::Fixed(_)));
            if current && fixed {
                errs.push(err(
                    e,
                    "an icon paints with currentColor and with a fixed color at once".to_string(),
                ));
                continue;
            }
            let r = recs[0];
            let paint = format!(
                " fill=\"{}\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"{}\" stroke-linejoin=\"{}\"",
                paint_attr(fill, &c.tokens),
                paint_attr(stroke, &c.tokens),
                r.stroke_width.px,
                r.stroke_linecap.keyword(),
                r.stroke_linejoin.keyword()
            );
            let mut body = String::new();
            children_markup(sym, &mut body);
            let id = href.trim_start_matches('#');
            let path = format!(
                "icons/{id}@{}x{}/{}",
                size[0],
                size[1],
                paint.trim().replace('"', "").replace(' ', ";")
            );
            add(
                Want {
                    path,
                    source: Source::Svg(svg_root(true, sym.attr("viewBox"), &paint, &body)),
                    size,
                    mask: current,
                },
                e.id,
                &mut wants,
            );
        } else if e.tag == "img"
            && let Some(src) = e.attr("src")
            && e.attr("data-bind-src").is_none()
        {
            if src.contains(':') || src.starts_with('/') {
                // Checked as a resource elsewhere.
                continue;
            }
            let size = match size_of(c, e) {
                Ok(Some(s)) => s,
                Ok(None) => continue,
                Err(x) => {
                    errs.push(x);
                    continue;
                }
            };
            let file = e
                .place()
                .file
                .rsplit_once('/')
                .map_or(src.to_string(), |(dir, _)| format!("{dir}/{src}"));
            let x2 = x2_name(&file);
            let path = format!("{file}@{}x{}", size[0], size[1]);
            add(
                Want {
                    path,
                    source: Source::File(file.clone(), x2),
                    size,
                    mask: false,
                },
                e.id,
                &mut wants,
            );
        }
    }
    (wants, of, errs)
}

/// The scale 2 file next to a picture: `name@2x.ext`.
fn x2_name(file: &str) -> String {
    match file.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => format!("{stem}@2x.{ext}"),
        _ => file.to_string(),
    }
}

/// The pictures the style records name by path (`background-image` layers, `border-image-source`), each
/// with the first element in document order whose style uses it.
fn named(dom: &Dom, c: &Compiled) -> BTreeMap<String, u32> {
    let mut out = BTreeMap::new();
    for e in &dom.elems {
        for r in records(c, e.id) {
            let layers = r.background_image.iter().filter_map(|l| match l {
                BgImage::Url(s) => Some(*s),
                BgImage::Gradient(_) => None,
            });
            for s in layers.chain(r.border_image_source) {
                out.entry(c.strs.get(s).to_string()).or_insert(e.id);
            }
        }
    }
    out
}

fn device(v: f32, k: f32) -> u32 {
    ((v * k) - 1e-3).ceil().max(1.0) as u32
}

fn b64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        buf = (buf << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// One rasterized picture: device size and straight RGBA at scale 1 and 2.
#[derive(Debug, Clone)]
pub struct Raster {
    /// The name.
    pub path: String,
    /// CSS size.
    pub size: [f32; 2],
    /// A mask icon.
    pub mask: bool,
    /// `(w, h, rgba)` at scale 1 and at scale 2.
    pub scales: [(u32, u32, Vec<u8>); 2],
}

/// Rasterizes the pictures of the page in Chromium and packs them.
// SIMPLIFIED(SIMP-217): pictures and SVG icons are rasterized only at device scale 1 and 2; at any other
// scale the runtime draws the 2x sprite resized (at 1.5 a smaller 2x), not a picture made for that scale.
pub fn pictures(
    node: &mut NodeClient,
    skin_dir: &std::path::Path,
    dom: &Dom,
    c: &Compiled,
) -> Result<Pictures, Vec<ConvError>> {
    let (wants, of, mut errs) = wanted(dom, c);
    let elem = |id: u32| dom.by_id.get(&id).map(|&ix| dom.elems[ix]);
    // Pictures named by path: drawn at their natural size, which Chromium reads from the file.
    let mut natural: Vec<(String, u32)> = Vec::new();
    for (path, id) in named(dom, c) {
        if is_skin_path(&path) && skin_dir.join(&path).is_file() {
            natural.push((path, id));
            continue;
        }
        let why = format!("the picture `{path}` is not a file of the skin");
        errs.push(match elem(id) {
            Some(e) => ConvError::new(codes::RESOURCE, e.place(), why).element(e.describe()),
            None => ConvError::new(codes::RESOURCE, crate::errors::Place::default(), why),
        });
    }
    let mut items = Vec::new();
    for w in &wants {
        let (w1, h1) = (device(w.size[0], 1.0), device(w.size[1], 1.0));
        let (w2, h2) = (device(w.size[0], 2.0), device(w.size[1], 2.0));
        let mut item = json!({ "w1": w1, "h1": h1, "w2": w2, "h2": h2 });
        match &w.source {
            Source::Svg(m) => {
                item["svg1"] = json!(
                    m.replace("{W}", &w1.to_string())
                        .replace("{H}", &h1.to_string())
                );
                item["svg2"] = json!(
                    m.replace("{W}", &w2.to_string())
                        .replace("{H}", &h2.to_string())
                );
            }
            Source::File(f, f2) => {
                item["src1"] = json!(f);
                let has2 = skin_dir.join(f2).is_file();
                item["src2"] = json!(if has2 { f2 } else { f });
            }
        }
        items.push(item);
    }
    for (path, _) in &natural {
        let x2 = x2_name(path);
        let has2 = x2 != *path && skin_dir.join(&x2).is_file();
        items.push(json!({
            "natural": true,
            "src1": path,
            "src2": if has2 { &x2 } else { path },
        }));
    }
    let mut rasters = Vec::new();
    if !items.is_empty() {
        let answer = node
            .call("raster", json!({ "items": items }))
            .map_err(|e| vec![e])?;
        let list = answer
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let size_of = |a: &Value, k: &str| {
            a.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        let jobs = wants.iter().map(|w| (w.path.clone(), Some(w), None)).chain(
            natural
                .iter()
                .map(|(path, id)| (path.clone(), None, Some(*id))),
        );
        for ((path, want, id), a) in jobs.zip(list.iter()) {
            if let Some(why) = a.get("error").and_then(Value::as_str) {
                let why = format!("the picture `{path}` does not draw in Chromium: {why}");
                errs.push(match id.and_then(elem) {
                    Some(e) => ConvError::new(codes::IMAGE, e.place(), why).element(e.describe()),
                    None => ConvError::new(codes::IMAGE, crate::errors::Place::default(), why),
                });
                continue;
            }
            let get = |k: &str| a.get(k).and_then(Value::as_str).and_then(b64);
            let (size, mask, [w1, h1, w2, h2]) = match want {
                Some(w) => (
                    w.size,
                    w.mask,
                    [
                        device(w.size[0], 1.0),
                        device(w.size[1], 1.0),
                        device(w.size[0], 2.0),
                        device(w.size[1], 2.0),
                    ],
                ),
                None => match (size_of(a, "w1"), size_of(a, "h1")) {
                    (Some(w1), Some(h1)) => {
                        ([w1 as f32, h1 as f32], false, [w1, h1, w1 * 2, h1 * 2])
                    }
                    // No size: reported below as a picture without pixels.
                    _ => ([0.0, 0.0], false, [0, 0, 0, 0]),
                },
            };
            match (get("x1"), get("x2")) {
                (Some(a1), Some(a2))
                    if w1 > 0
                        && a1.len() == (w1 * h1 * 4) as usize
                        && a2.len() == (w2 * h2 * 4) as usize =>
                {
                    rasters.push(Raster {
                        path,
                        size,
                        mask,
                        scales: [(w1, h1, a1), (w2, h2, a2)],
                    });
                }
                _ => errs.push(ConvError::new(
                    codes::PROTOCOL,
                    crate::errors::Place::default(),
                    format!("raster: no pixels for {path}"),
                )),
            }
        }
        if list.len() != items.len() {
            errs.push(ConvError::new(
                codes::PROTOCOL,
                crate::errors::Place::default(),
                "raster: the answer has another number of items",
            ));
        }
    }
    if !errs.is_empty() {
        return Err(errs);
    }
    let mut p = crate::atlas::pack(&rasters).map_err(|e| vec![e])?;
    let picture_of: BTreeMap<&str, usize> = p
        .imgs
        .iter()
        .enumerate()
        .map(|(i, x): (usize, &Picture)| (x.path.as_str(), i))
        .collect();
    for (id, w) in of {
        let want = &wants[w];
        if let Some(&i) = picture_of.get(want.path.as_str()) {
            p.of_element.insert(id, (i, want.mask));
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes() {
        assert_eq!(b64("AAEC/w==").as_deref(), Some(&[0u8, 1, 2, 255][..]));
        assert_eq!(b64("aGk=").as_deref(), Some(&b"hi"[..]));
        assert_eq!(b64("a b"), None);
    }

    #[test]
    fn device_sizes_round_up_to_whole_pixels() {
        assert_eq!(device(16.0, 1.0), 16);
        assert_eq!(device(16.0, 2.0), 32);
        assert_eq!(device(16.5, 1.0), 17);
        assert_eq!(device(0.2, 1.0), 1);
    }
}
