//! Sampler states of the original shaders.
//!
//! The shader bytecode does not carry filter, addressing, LOD bias or anisotropy; the original sets
//! them on the device. They live here as a table "entry point + sampler name (the CTAB name, e.g.
//! `BaseSampler`) -> [`SamplerDesc`]". The table is filled from snapshots (`golden` dumps the
//! states) and from `re-world`; a lookup that finds nothing gets [`SamplerDesc::DEFAULT`] and a
//! warning, once per key.

use std::collections::HashSet;
use std::sync::Mutex;

/// Texture addressing, per axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Address {
    Repeat,
    Clamp,
    Mirror,
}

/// A sampler state. Fields are what D3D9/D3D11 sampler states carry that wgpu can express, plus
/// `lod_bias`, which wgpu cannot: the shader adds it to the level it samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplerDesc {
    pub mag_linear: bool,
    pub min_linear: bool,
    pub mip_linear: bool,
    pub address_u: Address,
    pub address_v: Address,
    pub address_w: Address,
    pub lod_bias: f32,
    /// 1 = off. Only honoured when all three filters are linear (wgpu rule).
    pub anisotropy: u16,
}

impl SamplerDesc {
    /// Linear filter, repeat, no bias, no anisotropy: the fallback for a missing entry.
    pub const DEFAULT: Self = Self {
        mag_linear: true,
        min_linear: true,
        mip_linear: true,
        address_u: Address::Repeat,
        address_v: Address::Repeat,
        address_w: Address::Repeat,
        lod_bias: 0.0,
        anisotropy: 1,
    };

    /// The same state with clamped addressing.
    pub const fn clamped(mut self) -> Self {
        self.address_u = Address::Clamp;
        self.address_v = Address::Clamp;
        self.address_w = Address::Clamp;
        self
    }

    pub fn to_wgpu<'a>(&self, label: &'a str) -> wgpu::SamplerDescriptor<'a> {
        let filter = |linear| {
            if linear {
                wgpu::FilterMode::Linear
            } else {
                wgpu::FilterMode::Nearest
            }
        };
        let address = |a| match a {
            Address::Repeat => wgpu::AddressMode::Repeat,
            Address::Clamp => wgpu::AddressMode::ClampToEdge,
            Address::Mirror => wgpu::AddressMode::MirrorRepeat,
        };
        let all_linear = self.mag_linear && self.min_linear && self.mip_linear;
        wgpu::SamplerDescriptor {
            label: Some(label),
            address_mode_u: address(self.address_u),
            address_mode_v: address(self.address_v),
            address_mode_w: address(self.address_w),
            mag_filter: filter(self.mag_linear),
            min_filter: filter(self.min_linear),
            mipmap_filter: if self.mip_linear {
                wgpu::MipmapFilterMode::Linear
            } else {
                wgpu::MipmapFilterMode::Nearest
            },
            anisotropy_clamp: if all_linear {
                self.anisotropy.max(1)
            } else {
                1
            },
            ..Default::default()
        }
    }
}

impl Default for SamplerDesc {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One table row: `entry` is the shader entry point (`UnitPS`), `name` the CTAB sampler name.
pub type SamplerEntry = (&'static str, &'static str, SamplerDesc);

/// Rows captured so far. Empty until the first snapshot lands (S1, `golden`).
pub static ORIGINAL_SAMPLERS: &[SamplerEntry] = &[];

/// A lookup result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    pub desc: SamplerDesc,
    /// No row matched; `desc` is the default.
    pub defaulted: bool,
}

pub struct SamplerTable {
    rows: &'static [SamplerEntry],
    warned: Mutex<HashSet<(String, String)>>,
}

impl SamplerTable {
    /// The table of the original shaders.
    pub fn original() -> Self {
        Self::with_rows(ORIGINAL_SAMPLERS)
    }

    pub fn with_rows(rows: &'static [SamplerEntry]) -> Self {
        Self {
            rows,
            warned: Mutex::new(HashSet::new()),
        }
    }

    pub fn get(&self, entry: &str, name: &str) -> Option<SamplerDesc> {
        self.rows
            .iter()
            .find(|(e, n, _)| *e == entry && *n == name)
            .map(|(_, _, d)| *d)
    }

    /// The state for `entry`/`name`; the default and a warning (once per key) when there is no row.
    pub fn resolve(&self, entry: &str, name: &str) -> Resolved {
        match self.get(entry, name) {
            Some(desc) => Resolved {
                desc,
                defaulted: false,
            },
            None => {
                let first = self
                    .warned
                    .lock()
                    .map(|mut w| w.insert((entry.to_owned(), name.to_owned())))
                    .unwrap_or(true);
                if first {
                    tracing::warn!(
                        "no sampler state for {entry}/{name}: using linear filter, repeat, no anisotropy"
                    );
                }
                Resolved {
                    desc: SamplerDesc::DEFAULT,
                    defaulted: true,
                }
            }
        }
    }

    pub fn create(
        &self,
        device: &wgpu::Device,
        entry: &str,
        name: &str,
    ) -> (wgpu::Sampler, Resolved) {
        let r = self.resolve(entry, name);
        let label = format!("{entry}/{name}");
        (device.create_sampler(&r.desc.to_wgpu(&label)), r)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static ROWS: &[SamplerEntry] = &[
        (
            "UnitPS",
            "BaseSampler",
            SamplerDesc {
                anisotropy: 8,
                ..SamplerDesc::DEFAULT.clamped()
            },
        ),
        (
            "TerrainPS",
            "BaseSampler",
            SamplerDesc {
                mip_linear: false,
                ..SamplerDesc::DEFAULT
            },
        ),
    ];

    #[test]
    fn lookup_is_by_entry_and_name() {
        let t = SamplerTable::with_rows(ROWS);
        let r = t.resolve("UnitPS", "BaseSampler");
        assert!(!r.defaulted);
        assert_eq!(r.desc.address_u, Address::Clamp);
        assert_eq!(r.desc.anisotropy, 8);
        // Same sampler name, other entry point: a different row.
        let r = t.resolve("TerrainPS", "BaseSampler");
        assert!(!r.defaulted);
        assert!(!r.desc.mip_linear);
        assert_eq!(t.get("UnitPS", "Missing"), None);
    }

    #[test]
    fn missing_entry_gets_the_default() {
        let t = SamplerTable::with_rows(ROWS);
        let r = t.resolve("WaterPS", "BumpSampler");
        assert!(r.defaulted);
        assert_eq!(r.desc, SamplerDesc::DEFAULT);
        assert_eq!(r.desc.address_u, Address::Repeat);
        assert_eq!(r.desc.anisotropy, 1);
        // The warning is remembered per key; a second lookup still defaults.
        assert!(t.resolve("WaterPS", "BumpSampler").defaulted);
        assert_eq!(t.warned.lock().unwrap().len(), 1);
        assert!(SamplerTable::original().resolve("X", "Y").defaulted);
    }

    #[test]
    fn wgpu_descriptor_follows_the_state() {
        let d = ROWS[0].2.to_wgpu("t");
        assert_eq!(d.address_mode_u, wgpu::AddressMode::ClampToEdge);
        assert_eq!(d.anisotropy_clamp, 8);
        assert_eq!(d.min_filter, wgpu::FilterMode::Linear);
        // Anisotropy needs all-linear filters.
        let d = SamplerDesc {
            mag_linear: false,
            anisotropy: 8,
            ..SamplerDesc::DEFAULT
        }
        .to_wgpu("t");
        assert_eq!(d.anisotropy_clamp, 1);
        assert_eq!(d.mag_filter, wgpu::FilterMode::Nearest);
    }
}
