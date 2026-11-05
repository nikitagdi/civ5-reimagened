//! `maps/formats.toml`: where our texture format differs from D3D9's.
//!
//! Both sides of a comparison get **the same texture in our format**; the oracle's wrapper adds
//! the swizzle and the quantisation, so that the oracle sees what D3D9 would have seen.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::random::{Rng, f32_to_f16};
use crate::oracle::{Chan, FormatView, Quantize};
use crate::sm3::ir::SamplerKind;

/// Our texture format of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OurFormat {
    Rgba8Unorm,
    R8Unorm,
    Rgba16Float,
}

impl OurFormat {
    pub fn parse(s: &str) -> Option<OurFormat> {
        Some(match s {
            "Rgba8Unorm" => OurFormat::Rgba8Unorm,
            "R8Unorm" => OurFormat::R8Unorm,
            "Rgba16Float" => OurFormat::Rgba16Float,
            _ => return None,
        })
    }
    pub fn bytes_per_texel(self) -> usize {
        match self {
            OurFormat::Rgba8Unorm => 4,
            OurFormat::R8Unorm => 1,
            OurFormat::Rgba16Float => 8,
        }
    }
    pub fn wgpu(self) -> wgpu::TextureFormat {
        match self {
            OurFormat::Rgba8Unorm => wgpu::TextureFormat::Rgba8Unorm,
            OurFormat::R8Unorm => wgpu::TextureFormat::R8Unorm,
            OurFormat::Rgba16Float => wgpu::TextureFormat::Rgba16Float,
        }
    }
}

/// How the random texel values are made representable in the original's format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataQuant {
    /// Plain random bytes of our format.
    None,
    /// R5G6B5: random 5-6-5 values, expanded to 8 bits.
    Rgb565,
    /// 16 bit UNORM: random `k / 65535`, stored as half floats.
    Unorm16,
}

/// A row of `[format.*]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatDef {
    pub name: String,
    pub our: OurFormat,
    /// Where x, y, z, w of the D3D9 value come from (`r g b a 0 1`).
    pub view: [Chan; 4],
    pub quant: DataQuant,
}

impl FormatDef {
    /// What the oracle's wrapper applies to a sampled value.
    pub fn oracle_view(&self) -> FormatView {
        FormatView {
            chans: self.view,
            quantize: if self.quant == DataQuant::Unorm16 {
                Quantize::Unorm16
            } else {
                Quantize::None
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFormat {
    /// Our format: `Rgba8Unorm`, `R8Unorm`, `Rgba16Float`.
    our: String,
    /// Four of `r g b a 0 1`.
    view: String,
    /// `565` or `unorm16`; omitted for none.
    #[serde(default)]
    quantize: Option<String>,
    /// Who asked for the row (documentation only).
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    d3d9: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSampler {
    /// `Package::Entry:STAGE`, `*` matches any run of characters.
    entry: String,
    /// CTAB sampler name.
    name: String,
    format: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    format: BTreeMap<String, RawFormat>,
    #[serde(default)]
    sampler: Vec<RawSampler>,
}

#[derive(Debug, thiserror::Error)]
pub enum FormatsError {
    #[error("{path}: {why}", path = .path.display())]
    Io {
        path: std::path::PathBuf,
        why: String,
    },
    #[error("formats.toml: {0}")]
    Bad(String),
}

#[derive(Clone, Debug, Default)]
pub struct Formats {
    pub defs: BTreeMap<String, FormatDef>,
    rules: Vec<(String, String, String)>,
}

impl Formats {
    pub fn parse(text: &str) -> Result<Formats, FormatsError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| FormatsError::Bad(e.to_string()))?;
        let mut defs = BTreeMap::new();
        for (name, r) in raw.format {
            let our = OurFormat::parse(&r.our).ok_or_else(|| {
                FormatsError::Bad(format!("[format.{name}]: unknown our = {:?}", r.our))
            })?;
            let chars: Vec<char> = r.view.chars().collect();
            if chars.len() != 4 {
                return Err(FormatsError::Bad(format!(
                    "[format.{name}]: view {:?} needs four of r g b a 0 1",
                    r.view
                )));
            }
            let mut view = [Chan::R; 4];
            for (k, c) in chars.iter().enumerate() {
                view[k] = Chan::parse(*c).ok_or_else(|| {
                    FormatsError::Bad(format!("[format.{name}]: view character {c:?}"))
                })?;
            }
            let quant = match r.quantize.as_deref() {
                None | Some("none") => DataQuant::None,
                Some("565") => DataQuant::Rgb565,
                Some("unorm16") => DataQuant::Unorm16,
                Some(other) => {
                    return Err(FormatsError::Bad(format!(
                        "[format.{name}]: unknown quantize = {other:?}"
                    )));
                }
            };
            let _ = (&r.owner, &r.d3d9);
            defs.insert(
                name.clone(),
                FormatDef {
                    name,
                    our,
                    view,
                    quant,
                },
            );
        }
        let mut rules = Vec::new();
        for s in raw.sampler {
            if !defs.contains_key(&s.format) {
                return Err(FormatsError::Bad(format!(
                    "[[sampler]] {} {}: unknown format {:?}",
                    s.entry, s.name, s.format
                )));
            }
            rules.push((s.entry, s.name, s.format));
        }
        Ok(Formats { defs, rules })
    }

    pub fn load(path: &Path) -> Result<Formats, FormatsError> {
        let text = std::fs::read_to_string(path).map_err(|e| FormatsError::Io {
            path: path.to_path_buf(),
            why: e.to_string(),
        })?;
        Formats::parse(&text)
    }

    /// The row for sampler `name` of the entry `entry`, if the map has one (last match wins).
    pub fn for_sampler(&self, entry: &str, name: &str) -> Option<&FormatDef> {
        self.rules
            .iter()
            .rev()
            .find(|(pat, n, _)| n == name && glob(pat, entry))
            .and_then(|(_, _, f)| self.defs.get(f))
    }
}

/// `*` matches any run of characters (also none); everything else matches itself.
pub fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (k, part) in parts.iter().enumerate() {
        if k == 0 {
            match rest.strip_prefix(part) {
                Some(r) => rest = r,
                None => return false,
            }
        } else if k == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// CPU contents of a test texture, in our format.
#[derive(Clone, Debug)]
pub struct TexData {
    pub kind: SamplerKind,
    pub format: OurFormat,
    /// Edge length of level 0.
    pub size: u32,
    /// One entry per mip level; cube levels hold the six faces one after the other.
    pub mips: Vec<Vec<u8>>,
}

/// Edge of level 0 for 2D, cube and volume textures.
pub fn edge_of(kind: SamplerKind) -> u32 {
    match kind {
        SamplerKind::Cube => 16,
        SamplerKind::Volume => 8,
        _ => 32,
    }
}

impl TexData {
    pub fn layers(&self) -> u32 {
        if self.kind == SamplerKind::Cube { 6 } else { 1 }
    }

    /// Level `l` is `edge(l)` texels wide and high (and deep, for a volume).
    pub fn edge(&self, level: usize) -> u32 {
        (self.size >> level).max(1)
    }

    /// Random contents for sampler `name`, deterministic in `(seed, name)`. `def` is the row of
    /// the format map, or `None` for a plain `Rgba8Unorm` texture.
    pub fn random(seed: u64, name: &str, kind: SamplerKind, def: Option<&FormatDef>) -> TexData {
        let format = def.map_or(OurFormat::Rgba8Unorm, |d| d.our);
        let quant = def.map_or(DataQuant::None, |d| d.quant);
        let size = edge_of(kind);
        let levels = size.ilog2() as usize + 1;
        let mut rng = Rng::keyed(seed, &format!("texture:{name}"));
        let mut mips = Vec::new();
        for l in 0..levels {
            let e = (size >> l).max(1) as usize;
            let texels = match kind {
                SamplerKind::Cube => e * e * 6,
                SamplerKind::Volume => e * e * e,
                _ => e * e,
            };
            let mut data = Vec::with_capacity(texels * format.bytes_per_texel());
            for _ in 0..texels {
                match (format, quant) {
                    (OurFormat::Rgba8Unorm, DataQuant::Rgb565) => {
                        let (r5, g6, b5) = (rng.below(32), rng.below(64), rng.below(32));
                        data.extend_from_slice(&[
                            ((r5 << 3) | (r5 >> 2)) as u8,
                            ((g6 << 2) | (g6 >> 4)) as u8,
                            ((b5 << 3) | (b5 >> 2)) as u8,
                            255,
                        ]);
                    }
                    (OurFormat::Rgba8Unorm, _) => {
                        data.extend_from_slice(&rng.next_u32().to_le_bytes());
                    }
                    (OurFormat::R8Unorm, _) => data.push(rng.next_u32() as u8),
                    (OurFormat::Rgba16Float, q) => {
                        for _ in 0..4 {
                            let k = rng.below(65536);
                            let v = if q == DataQuant::Unorm16 {
                                k as f32 / 65535.0
                            } else {
                                rng.unit()
                            };
                            data.extend_from_slice(&f32_to_f16(v).to_le_bytes());
                        }
                    }
                }
            }
            mips.push(data);
        }
        TexData {
            kind,
            format,
            size,
            mips,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[format.L8]
our = "R8Unorm"
view = "rrr1"

[format.A8]
our = "R8Unorm"
view = "000r"

[format.R5G6B5]
our = "Rgba8Unorm"
view = "rgb1"
quantize = "565"

[format.LEAN]
our = "Rgba16Float"
view = "rgba"
quantize = "unorm16"

[[sampler]]
entry = "TerrainShader::*"
name = "DisplacementMap"
format = "A8"
"#;

    #[test]
    fn parses_rows_and_looks_up_by_entry_glob() {
        let f = Formats::parse(SAMPLE).unwrap();
        assert_eq!(f.defs.len(), 4);
        let a8 = f
            .for_sampler("TerrainShader::TerrainVS:VS", "DisplacementMap")
            .unwrap();
        assert_eq!(a8.view, [Chan::Zero, Chan::Zero, Chan::Zero, Chan::R]);
        assert_eq!(a8.our, OurFormat::R8Unorm);
        assert!(
            f.for_sampler("WaterShader::X:PS", "DisplacementMap")
                .is_none()
        );
        assert!(f.for_sampler("TerrainShader::X:PS", "Other").is_none());
        assert_eq!(f.defs["LEAN"].oracle_view().quantize, Quantize::Unorm16);
    }

    #[test]
    fn bad_rows_are_errors() {
        assert!(Formats::parse("[format.X]\nour = \"Foo\"\nview = \"rgba\"\n").is_err());
        assert!(Formats::parse("[format.X]\nour = \"R8Unorm\"\nview = \"rg\"\n").is_err());
        assert!(Formats::parse("[format.X]\nour = \"R8Unorm\"\nview = \"rgbz\"\n").is_err());
        assert!(
            Formats::parse("[[sampler]]\nentry = \"*\"\nname = \"a\"\nformat = \"nope\"\n")
                .is_err()
        );
    }

    #[test]
    fn glob_handles_stars() {
        assert!(glob("*", "anything"));
        assert!(glob("A::*:PS", "A::b:PS"));
        assert!(!glob("A::*:PS", "A::b:VS"));
        assert!(glob("A::B:PS", "A::B:PS"));
        assert!(!glob("A::B:PS", "A::B:PSX"));
        assert!(glob("*Water*", "xWaterShader"));
    }

    #[test]
    fn texture_contents_follow_the_format() {
        let f = Formats::parse(SAMPLE).unwrap();
        let t = TexData::random(1, "t", SamplerKind::Tex2d, Some(&f.defs["R5G6B5"]));
        assert_eq!(t.mips.len(), 6);
        assert_eq!(t.mips[0].len(), 32 * 32 * 4);
        // Every texel is a 5-6-5 value expanded to 8 bits, alpha 255.
        for px in t.mips[0].chunks_exact(4) {
            assert_eq!(px[0] & 7, px[0] >> 5, "5 bit red is replicated");
            assert_eq!(px[1] & 3, px[1] >> 6, "6 bit green is replicated");
            assert_eq!(px[3], 255);
        }
        let c = TexData::random(1, "t", SamplerKind::Cube, None);
        assert_eq!(c.mips[0].len(), 16 * 16 * 6 * 4);
        let v = TexData::random(1, "t", SamplerKind::Volume, Some(&f.defs["LEAN"]));
        assert_eq!(v.mips[0].len(), 8 * 8 * 8 * 8);
        // Deterministic.
        let a = TexData::random(9, "n", SamplerKind::Tex2d, None);
        let b = TexData::random(9, "n", SamplerKind::Tex2d, None);
        assert_eq!(a.mips, b.mips);
    }
}
