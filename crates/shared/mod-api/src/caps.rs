//! Capabilities (D2): declared in `mod.toml` (`rust.caps`), validated here, checked when the API is used.

use std::collections::BTreeSet;
use std::fmt;

use mod_manifest::{CAP_NET_CHANNEL, Manifest, Part};
use thiserror::Error;

/// A known capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Cap {
    /// Channels to and from the mod's other half.
    NetChannel,
    /// Whole-truth map facts (`ServerCtx::facts`) on the server, the replica view on the client.
    WorldRead,
    /// Claim keyboard and pointer input (and pointer lock).
    InputClaim,
    /// Override the camera.
    Camera,
    /// A render system in `MainSlot::Mod`. Trusted-native: a sandboxed host refuses it.
    RenderPipeline,
    /// Draw HUD elements.
    Hud,
    /// Per-mod persistent key-value store on the client.
    Kv,
    /// Play sounds.
    Audio,
    /// Request the mod's asset pack.
    Assets,
    /// Cut columns out of the terrain mesh.
    TerrainCutout,
}

impl Cap {
    pub const ALL: [Cap; 10] = [
        Cap::NetChannel,
        Cap::WorldRead,
        Cap::InputClaim,
        Cap::Camera,
        Cap::RenderPipeline,
        Cap::Hud,
        Cap::Kv,
        Cap::Audio,
        Cap::Assets,
        Cap::TerrainCutout,
    ];

    /// The name used in `mod.toml`.
    pub fn as_str(self) -> &'static str {
        match self {
            Cap::NetChannel => CAP_NET_CHANNEL,
            Cap::WorldRead => "world.read",
            Cap::InputClaim => "input.claim",
            Cap::Camera => "camera",
            Cap::RenderPipeline => "render.pipeline",
            Cap::Hud => "hud",
            Cap::Kv => "kv",
            Cap::Audio => "audio",
            Cap::Assets => "assets",
            Cap::TerrainCutout => "terrain.cutout",
        }
    }

    pub fn parse(s: &str) -> Option<Cap> {
        Cap::ALL.into_iter().find(|c| c.as_str() == s)
    }

    /// Meaningful only to the client half; a mod without a `client-rs` part cannot declare it.
    pub fn client_only(self) -> bool {
        matches!(
            self,
            Cap::InputClaim
                | Cap::Camera
                | Cap::RenderPipeline
                | Cap::Hud
                | Cap::Kv
                | Cap::Audio
                | Cap::TerrainCutout
        )
    }

    /// A native-only capability that a future sandboxed (WIT) host must refuse.
    pub fn trusted_native(self) -> bool {
        self == Cap::RenderPipeline
    }
}

impl fmt::Display for Cap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CapsError {
    #[error("unknown capability `{0}`")]
    Unknown(String),
    #[error("capability `{0}` is client-side, but the mod has no `client-rs` part")]
    ClientOnly(Cap),
}

/// A capability was used without being declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("capability `{0}` is not declared in the manifest")]
pub struct CapDenied(pub Cap);

/// The set of capabilities one mod holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Caps(BTreeSet<Cap>);

impl Caps {
    /// No capabilities.
    pub fn none() -> Self {
        Self::default()
    }

    /// Every capability (tests).
    pub fn all() -> Self {
        Self(Cap::ALL.into_iter().collect())
    }

    pub fn of(caps: &[Cap]) -> Self {
        Self(caps.iter().copied().collect())
    }

    /// Parses capability names; an unknown name is an error.
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<Self, CapsError> {
        let mut set = BTreeSet::new();
        for n in names {
            set.insert(Cap::parse(n).ok_or_else(|| CapsError::Unknown(n.to_string()))?);
        }
        Ok(Self(set))
    }

    /// Validates the `rust.caps` of a parsed manifest against the known list.
    pub fn from_manifest(m: &Manifest) -> Result<Self, CapsError> {
        let caps = Self::from_names(m.rust.caps.iter().map(String::as_str))?;
        if !m.has_part(Part::ClientRs)
            && let Some(c) = caps.iter().find(|c| c.client_only())
        {
            return Err(CapsError::ClientOnly(c));
        }
        Ok(caps)
    }

    pub fn has(&self, cap: Cap) -> bool {
        self.0.contains(&cap)
    }

    pub fn require(&self, cap: Cap) -> Result<(), CapDenied> {
        if self.has(cap) {
            Ok(())
        } else {
            Err(CapDenied(cap))
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Cap> + '_ {
        self.0.iter().copied()
    }
}
