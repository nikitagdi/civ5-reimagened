//! Switches for the subsystems: the `[DisableSystems]` names of
//! `config.ini`. `render-core` leaves a switched-off subsystem out of the frame plan; the code of
//! the subsystems checks nothing itself.

use std::collections::BTreeSet;
use std::fmt;

use crate::frame::SystemId;

/// The 24 keys of `[DisableSystems]` in the `config.ini` of the original, in its order.
pub const DISABLE_NAMES: [&str; 24] = [
    "Terrain",
    "LandmarkSystem",
    "Overlay",
    "OverlayDebug",
    "Decal",
    "City",
    "Unit",
    "Forest",
    "River",
    "LeaderHead",
    "Audio",
    "Combat",
    "Particle",
    "Projectile",
    "FOW",
    "FluidFOW",
    "YieldIcons",
    "DistanceFog",
    "StrategicView",
    "Minimap",
    "ColorKeyEdit",
    "Waves",
    "Arrows",
    "MovementPath",
];

/// Names that are accepted and do nothing: B2 has no such system (BUILD §5, R-D5).
pub const NOT_IN_B2: [&str; 7] = [
    "OverlayDebug",
    "LeaderHead",
    "Audio",
    "Combat",
    "Projectile",
    "StrategicView",
    "ColorKeyEdit",
];

/// What a name could not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayerWarning {
    /// Not one of the 24 names.
    Unknown(String),
    /// A name of the original whose system B2 does not have.
    NoEffect(&'static str),
}

impl fmt::Display for LayerWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayerWarning::Unknown(n) => write!(f, "unknown [DisableSystems] name {n:?}, ignored"),
            LayerWarning::NoEffect(n) => write!(f, "{n}: B2 has no such system, nothing to switch"),
        }
    }
}

/// The canonical spelling of a `[DisableSystems]` name (any case in), or `None`.
pub fn canonical_name(name: &str) -> Option<&'static str> {
    let name = name.trim();
    DISABLE_NAMES
        .into_iter()
        .find(|n| n.eq_ignore_ascii_case(name))
}

/// The subsystems a `[DisableSystems]` name switches off: those whose table-4.2 name it is. Empty
/// for a name that is in [`NOT_IN_B2`].
pub fn systems_named(name: &str) -> Vec<SystemId> {
    let Some(name) = canonical_name(name) else {
        return Vec::new();
    };
    SystemId::ALL
        .into_iter()
        .filter(|id| id.disable_name() == Some(name))
        .collect()
}

/// The set of switched-off subsystems.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LayerSwitches {
    /// Canonical names that were switched off (also those without effect).
    names: BTreeSet<&'static str>,
    disabled: BTreeSet<usize>,
}

impl LayerSwitches {
    /// Everything on.
    pub fn new() -> Self {
        Self::default()
    }

    /// Switches off the subsystems of `name` (any case). A name of B2-less systems is a
    /// [`LayerWarning::NoEffect`]; an unknown one is [`LayerWarning::Unknown`] and nothing changes.
    pub fn disable(&mut self, name: &str) -> Result<(), LayerWarning> {
        let Some(canonical) = canonical_name(name) else {
            return Err(LayerWarning::Unknown(name.trim().to_owned()));
        };
        self.names.insert(canonical);
        let systems = systems_named(canonical);
        for id in &systems {
            self.disabled.insert(index(*id));
        }
        if systems.is_empty() {
            debug_assert!(NOT_IN_B2.contains(&canonical));
            return Err(LayerWarning::NoEffect(canonical));
        }
        Ok(())
    }

    /// Switches the subsystems of `name` on again.
    pub fn enable(&mut self, name: &str) {
        if let Some(canonical) = canonical_name(name) {
            self.names.remove(canonical);
            for id in systems_named(canonical) {
                self.disabled.remove(&index(id));
            }
        }
    }

    /// `FOW,FluidFOW` (`?disable=`, `--disable`): names split at commas, semicolons and spaces.
    pub fn from_names(text: &str) -> (Self, Vec<LayerWarning>) {
        let mut out = Self::new();
        let mut warnings = Vec::new();
        for name in text
            .split([',', ';', ' '])
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            if let Err(w) = out.disable(name) {
                warnings.push(w);
            }
        }
        (out, warnings)
    }

    /// The `[DisableSystems]` section of a `config.ini` text: `Name = 1` switches off, `0` leaves on.
    pub fn from_config_ini(text: &str) -> (Self, Vec<LayerWarning>) {
        let mut out = Self::new();
        let mut warnings = Vec::new();
        let mut inside = false;
        for raw in text.lines() {
            let line = raw.trim_start_matches('\u{feff}').trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix('[') {
                let section = rest.split(']').next().unwrap_or("").trim();
                inside = section.eq_ignore_ascii_case("DisableSystems");
                continue;
            }
            if !inside {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.split(';').next().unwrap_or("").trim();
            if value.parse::<i64>().is_ok_and(|v| v != 0) {
                if let Err(w) = out.disable(name) {
                    warnings.push(w);
                }
            } else if canonical_name(name).is_none() {
                warnings.push(LayerWarning::Unknown(name.trim().to_owned()));
            }
        }
        (out, warnings)
    }

    /// Adds the switches of `other`.
    pub fn merge(&mut self, other: &LayerSwitches) {
        self.names.extend(other.names.iter().copied());
        self.disabled.extend(other.disabled.iter().copied());
    }

    pub fn is_disabled(&self, id: SystemId) -> bool {
        self.disabled.contains(&index(id))
    }

    /// The switched-off names, canonical spelling, sorted.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.names.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

fn index(id: SystemId) -> usize {
    SystemId::ALL
        .iter()
        .position(|&s| s == id)
        .expect("every SystemId is in ALL")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AC-R9.3: all 24 names parse; each switches a subsystem off by table 4.2 or is marked as
    /// not in B2.
    #[test]
    fn all_24_names_switch_a_subsystem_or_are_marked_not_in_b2() {
        assert_eq!(DISABLE_NAMES.len(), 24);
        let unique: BTreeSet<_> = DISABLE_NAMES.iter().collect();
        assert_eq!(unique.len(), 24);
        for name in DISABLE_NAMES {
            let systems = systems_named(name);
            if NOT_IN_B2.contains(&name) {
                assert!(systems.is_empty(), "{name}");
                let mut s = LayerSwitches::new();
                assert_eq!(s.disable(name), Err(LayerWarning::NoEffect(name)));
                assert!(SystemId::ALL.iter().all(|&id| !s.is_disabled(id)), "{name}");
            } else {
                assert!(!systems.is_empty(), "{name} switches nothing");
                let mut s = LayerSwitches::new();
                assert_eq!(s.disable(name), Ok(()), "{name}");
                for id in SystemId::ALL {
                    assert_eq!(s.is_disabled(id), systems.contains(&id), "{name} {id:?}");
                }
            }
        }
        // Every subsystem with a name is reached by one of the 24 (`Shadows` has none).
        for id in SystemId::ALL {
            match id.disable_name() {
                Some(n) => assert!(canonical_name(n) == Some(n), "{id:?}: {n}"),
                None => assert_eq!(id, SystemId::Shadows),
            }
        }
    }

    #[test]
    fn names_ignore_case_and_terrain_takes_water_and_ice() {
        let (s, warnings) = LayerSwitches::from_names("fow, FLUIDFOW;terrain");
        assert!(warnings.is_empty(), "{warnings:?}");
        for id in [
            SystemId::FowClassic,
            SystemId::FluidFow,
            SystemId::Terrain,
            SystemId::Water,
            SystemId::Ice,
        ] {
            assert!(s.is_disabled(id), "{id:?}");
        }
        assert!(!s.is_disabled(SystemId::Unit));
        assert_eq!(
            s.names().collect::<Vec<_>>(),
            ["FOW", "FluidFOW", "Terrain"]
        );
        // Arrows takes the trade routes with it (table 4.2).
        let (s, _) = LayerSwitches::from_names("Arrows");
        assert!(s.is_disabled(SystemId::Arrows) && s.is_disabled(SystemId::Trade));
    }

    #[test]
    fn unknown_names_warn_and_change_nothing() {
        let (s, warnings) = LayerSwitches::from_names("Bogus,Unit,Audio");
        assert_eq!(
            warnings,
            [
                LayerWarning::Unknown("Bogus".to_owned()),
                LayerWarning::NoEffect("Audio")
            ]
        );
        assert!(s.is_disabled(SystemId::Unit));
        assert_eq!(s.names().collect::<Vec<_>>(), ["Audio", "Unit"]);
        assert!(LayerSwitches::from_names("").0.is_empty());
    }

    /// The `[DisableSystems]` section of the user's `config.ini`: `OverlayDebug` and `ColorKeyEdit`
    /// are 1 there and have no system in B2.
    #[test]
    fn the_section_of_config_ini_is_read() {
        let ini = "\
[User Settings]
Terrain = 1

[DisableSystems]

; Set to 1 to disable Terrain system
Terrain = 0
OverlayDebug = 1
Unit = 0
FOW = 1
ColorKeyEdit = 1
Waves = 0

[Debug]
Overlay = 1
";
        let (s, warnings) = LayerSwitches::from_config_ini(ini);
        assert_eq!(
            warnings,
            [
                LayerWarning::NoEffect("OverlayDebug"),
                LayerWarning::NoEffect("ColorKeyEdit")
            ]
        );
        assert!(s.is_disabled(SystemId::FowClassic));
        for id in SystemId::ALL {
            assert_eq!(s.is_disabled(id), id == SystemId::FowClassic, "{id:?}");
        }
    }

    #[test]
    fn enable_and_merge() {
        let (mut s, _) = LayerSwitches::from_names("Unit,City");
        s.enable("unit");
        assert!(!s.is_disabled(SystemId::Unit) && s.is_disabled(SystemId::City));
        let (other, _) = LayerSwitches::from_names("Forest");
        s.merge(&other);
        assert!(s.is_disabled(SystemId::Forest) && s.is_disabled(SystemId::City));
    }
}
