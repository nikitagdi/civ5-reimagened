//! One table "D3D vertex semantic -> `@location`" for the whole renderer.
//! The vertex layout of a mesh (`Mesh.layout` of the blob) and the `@location`s of the ported
//! vertex shaders both come from here, and so does `golden` when it builds a layout from a
//! capture. 16 attributes is the limit of WebGL2.
//!
//! Data of an instance takes the `TEXCOORD` slots the family's mesh does not use, as the
//! instanced shaders of the original do (`BuildingVS_Instanced` gets its matrix through
//! `TEXCOORD`); which are free is written by the `ShaderFamily`.

/// Vertex attributes of `downlevel_webgl2_defaults`: `max_vertex_attributes`.
pub const MAX_VERTEX_ATTRIBUTES: u32 = 16;

/// A vertex semantic of SM3 as the original's shaders declare it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Semantic {
    Position0,
    Normal0,
    /// `TEXCOORD0..7`.
    Texcoord(u8),
    /// `COLOR0..1`.
    Color(u8),
    Tangent0,
    Binormal0,
    BlendIndices0,
    BlendWeight0,
}

impl Semantic {
    /// Every semantic of the table, in `@location` order.
    pub const ALL: [Semantic; 16] = [
        Semantic::Position0,
        Semantic::Normal0,
        Semantic::Texcoord(0),
        Semantic::Texcoord(1),
        Semantic::Texcoord(2),
        Semantic::Texcoord(3),
        Semantic::Texcoord(4),
        Semantic::Texcoord(5),
        Semantic::Texcoord(6),
        Semantic::Texcoord(7),
        Semantic::Color(0),
        Semantic::Color(1),
        Semantic::Tangent0,
        Semantic::Binormal0,
        Semantic::BlendIndices0,
        Semantic::BlendWeight0,
    ];

    /// The `@location` of the semantic: POSITION0 0, NORMAL0 1, TEXCOORD0..7 2..9, COLOR0..1
    /// 10..11, TANGENT0 12, BINORMAL0 13, BLENDINDICES0 14, BLENDWEIGHT0 15.
    pub const fn location(self) -> u32 {
        match self {
            Semantic::Position0 => 0,
            Semantic::Normal0 => 1,
            Semantic::Texcoord(i) => 2 + i as u32,
            Semantic::Color(i) => 10 + i as u32,
            Semantic::Tangent0 => 12,
            Semantic::Binormal0 => 13,
            Semantic::BlendIndices0 => 14,
            Semantic::BlendWeight0 => 15,
        }
    }

    /// The semantic at `@location` `location`.
    pub fn from_location(location: u32) -> Option<Semantic> {
        Self::ALL.get(location as usize).copied()
    }

    /// The name in a shader declaration (`dcl_texcoord3`, `float4 p : TEXCOORD3`).
    pub fn d3d_name(self) -> String {
        match self {
            Semantic::Position0 => "POSITION0".into(),
            Semantic::Normal0 => "NORMAL0".into(),
            Semantic::Texcoord(i) => format!("TEXCOORD{i}"),
            Semantic::Color(i) => format!("COLOR{i}"),
            Semantic::Tangent0 => "TANGENT0".into(),
            Semantic::Binormal0 => "BINORMAL0".into(),
            Semantic::BlendIndices0 => "BLENDINDICES0".into(),
            Semantic::BlendWeight0 => "BLENDWEIGHT0".into(),
        }
    }

    /// Parses `POSITION0`, `TEXCOORD3`, `COLOR1`, ... (case-insensitive). A bare name such as
    /// `POSITION` means index 0. Semantics the table does not have give `None`.
    pub fn parse(text: &str) -> Option<Semantic> {
        let upper = text.trim().to_ascii_uppercase();
        let split = upper
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(upper.len());
        let (name, digits) = upper.split_at(split);
        let index: u8 = if digits.is_empty() {
            0
        } else {
            digits.parse().ok()?
        };
        match (name, index) {
            ("POSITION", 0) => Some(Semantic::Position0),
            ("NORMAL", 0) => Some(Semantic::Normal0),
            ("TEXCOORD", 0..=7) => Some(Semantic::Texcoord(index)),
            ("COLOR", 0..=1) => Some(Semantic::Color(index)),
            ("TANGENT", 0) => Some(Semantic::Tangent0),
            ("BINORMAL", 0) => Some(Semantic::Binormal0),
            ("BLENDINDICES", 0) => Some(Semantic::BlendIndices0),
            ("BLENDWEIGHT", 0) => Some(Semantic::BlendWeight0),
            _ => None,
        }
    }
}

/// The `TEXCOORD` locations a mesh with the semantics `used` leaves free, for instance data.
pub fn free_texcoord_locations(used: &[Semantic]) -> Vec<u32> {
    (0..8)
        .map(Semantic::Texcoord)
        .filter(|s| !used.contains(s))
        .map(Semantic::location)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_spec_and_fills_the_webgl2_limit() {
        let loc = |s: Semantic| s.location();
        assert_eq!(loc(Semantic::Position0), 0);
        assert_eq!(loc(Semantic::Normal0), 1);
        assert_eq!(loc(Semantic::Texcoord(0)), 2);
        assert_eq!(loc(Semantic::Texcoord(7)), 9);
        assert_eq!(loc(Semantic::Color(0)), 10);
        assert_eq!(loc(Semantic::Color(1)), 11);
        assert_eq!(loc(Semantic::Tangent0), 12);
        assert_eq!(loc(Semantic::Binormal0), 13);
        assert_eq!(loc(Semantic::BlendIndices0), 14);
        assert_eq!(loc(Semantic::BlendWeight0), 15);
        // Exactly the 16 attributes of WebGL2, each location once, in order.
        let locations: Vec<u32> = Semantic::ALL.iter().map(|s| s.location()).collect();
        assert_eq!(locations, (0..MAX_VERTEX_ATTRIBUTES).collect::<Vec<_>>());
        for s in Semantic::ALL {
            assert_eq!(Semantic::from_location(s.location()), Some(s));
            assert_eq!(Semantic::parse(&s.d3d_name()), Some(s));
        }
        assert_eq!(Semantic::from_location(16), None);
    }

    #[test]
    fn names_parse_like_a_shader_declaration() {
        assert_eq!(Semantic::parse("texcoord3"), Some(Semantic::Texcoord(3)));
        assert_eq!(Semantic::parse("POSITION"), Some(Semantic::Position0));
        assert_eq!(Semantic::parse(" Color1 "), Some(Semantic::Color(1)));
        // Not in the table: a second set of normals, an eighth texture set, a point size.
        assert_eq!(Semantic::parse("NORMAL1"), None);
        assert_eq!(Semantic::parse("TEXCOORD8"), None);
        assert_eq!(Semantic::parse("PSIZE0"), None);
        assert_eq!(Semantic::parse(""), None);
    }

    #[test]
    fn instance_data_takes_the_free_texcoords() {
        let used = [
            Semantic::Position0,
            Semantic::Normal0,
            Semantic::Texcoord(0),
            Semantic::Texcoord(1),
        ];
        assert_eq!(free_texcoord_locations(&used), [4, 5, 6, 7, 8, 9]);
        assert_eq!(free_texcoord_locations(&[]).len(), 8);
    }
}
