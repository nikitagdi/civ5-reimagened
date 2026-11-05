//! `civ5-assets tex-audit`: which textures of an output folder ship in which format (B3
//! asset-delivery). Lists every texture blob of the manifest with its size, dimensions and mips,
//! and summarises bytes by format and by id prefix. Plain RGBA8 2D textures are the candidates
//! for BC7 / BC1 / BC4 (see `textures::repack`); the rest is for the record.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use asset_format::{TextureBlob, TextureFormat};

use super::budget::{ReportError, prefix_of, read_manifest};

/// One texture of the manifest.
#[derive(Debug, Clone)]
pub struct TexRow {
    pub id: String,
    pub format: TextureFormat,
    pub dim: asset_format::TexDim,
    pub srgb: bool,
    pub width: u32,
    pub height: u32,
    pub mips: usize,
    /// Payload bytes of all mips (what the GPU holds).
    pub gpu_bytes: u64,
    /// Blob size on disk (what is downloaded before transfer compression).
    pub blob_bytes: u64,
}

pub fn run(out: &Path) -> Result<Vec<TexRow>, ReportError> {
    let manifest = read_manifest(out)?;
    let mut rows = Vec::new();
    for (id, variants) in manifest.iter() {
        for v in variants {
            let path = out.join(v.hash.to_string());
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            // Anything that is not a texture blob fails the magic check: not a row.
            let Ok(t) = TextureBlob::decode(&bytes) else {
                continue;
            };
            rows.push(TexRow {
                id: id.as_str().to_owned(),
                format: t.format,
                dim: t.dim,
                srgb: t.srgb,
                width: t.width,
                height: t.height,
                mips: t.mips.len(),
                gpu_bytes: t.mips.iter().map(|m| m.len() as u64).sum(),
                blob_bytes: bytes.len() as u64,
            });
        }
    }
    Ok(rows)
}

pub fn to_text(rows: &[TexRow], top: usize) -> String {
    let mut s = String::new();
    let mut by_fmt: BTreeMap<String, (usize, u64, u64)> = BTreeMap::new();
    let mut by_prefix: BTreeMap<(String, String), (usize, u64)> = BTreeMap::new();
    for r in rows {
        let f = format!("{:?}{}", r.format, if r.srgb { "/srgb" } else { "" });
        let e = by_fmt.entry(f.clone()).or_default();
        e.0 += 1;
        e.1 += r.gpu_bytes;
        e.2 += r.blob_bytes;
        let e = by_prefix
            .entry((prefix_of(&r.id).to_owned(), format!("{:?}", r.format)))
            .or_default();
        e.0 += 1;
        e.1 += r.gpu_bytes;
    }
    let total: u64 = rows.iter().map(|r| r.gpu_bytes).sum();
    let _ = writeln!(
        s,
        "textures: {} blobs, {:.1} MB on the GPU",
        rows.len(),
        mb(total)
    );
    let _ = writeln!(s, "\nby format\n  format          count   gpu MB  blob MB");
    for (f, (n, g, b)) in &by_fmt {
        let _ = writeln!(s, "  {f:<14} {n:>6} {:>8.1} {:>8.1}", mb(*g), mb(*b));
    }
    let _ = writeln!(
        s,
        "\nby id prefix and format\n  prefix format  count   gpu MB"
    );
    for ((p, f), (n, g)) in &by_prefix {
        let _ = writeln!(s, "  {p:<6} {f:<8} {n:>6} {:>8.1}", mb(*g));
    }
    let mut cands: Vec<&TexRow> = rows
        .iter()
        .filter(|r| r.format == TextureFormat::Rgba8 && r.dim == asset_format::TexDim::D2)
        .collect();
    cands.sort_by_key(|r| std::cmp::Reverse(r.gpu_bytes));
    let _ = writeln!(s, "\nlargest RGBA8 2D textures (candidates)");
    for r in cands.iter().take(top) {
        let _ = writeln!(
            s,
            "  {:>8.2} MB  {}x{} mips={} {}  {}",
            mb(r.gpu_bytes),
            r.width,
            r.height,
            r.mips,
            if r.srgb { "srgb" } else { "lin" },
            r.id
        );
    }
    s
}

fn mb(b: u64) -> f64 {
    b as f64 / 1e6
}
