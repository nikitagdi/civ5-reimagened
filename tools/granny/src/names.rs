//! Member-name dictionary: stripped type trees (Civ5 FPK copies) keep `!crc32(name)` instead of the name.

use std::sync::OnceLock;

const NAMES_TXT: &str = include_str!("../data/member-names.txt");

/// Hash Granny stores in place of a stripped member name.
pub fn name_hash(name: &str) -> u32 {
    name_hash_bytes(name.as_bytes())
}

pub fn name_hash_bytes(name: &[u8]) -> u32 {
    !crate::crc32::crc32(name)
}

/// The hash of `prefix`, given the hash of `prefix + suffix`: `name_hash_prefix(name_hash(a + b),
/// b) == name_hash(a)`. Two stripped strings that end in known suffixes share a prefix (e.g. the
/// same source directory) exactly when their prefix hashes are equal.
pub fn name_hash_prefix(hash: u32, suffix: &[u8]) -> u32 {
    crate::crc32::unwind(hash, suffix)
}

fn table() -> &'static [(u32, &'static str)] {
    static TABLE: OnceLock<Vec<(u32, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t: Vec<_> = known_member_names().map(|n| (name_hash(n), n)).collect();
        t.sort_unstable();
        t.dedup_by_key(|e| e.0);
        t
    })
}

/// Looks up a stripped member name in the built-in dictionary.
pub fn member_name(hash: u32) -> Option<&'static str> {
    let t = table();
    t.binary_search_by_key(&hash, |e| e.0).ok().map(|i| t[i].1)
}

/// All names in `data/member-names.txt` (one per line, `#` starts a comment).
pub fn known_member_names() -> impl Iterator<Item = &'static str> {
    NAMES_TXT
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

const FGX_NAMES_TXT: &str = include_str!("../data/fgx-names.txt");

fn fgx_table() -> &'static [(u32, &'static str)] {
    static TABLE: OnceLock<Vec<(u32, &'static str)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t: Vec<_> = known_fgx_names().map(|n| (name_hash(n), n)).collect();
        t.sort_unstable();
        t.dedup_by_key(|e| e.0);
        t
    })
}

/// Looks up a stripped `FireGrafixMaterial` string (shader set, parameter block or parameter
/// name) in the FGX dictionary.
pub fn fgx_name(hash: u32) -> Option<&'static str> {
    // The empty string stripped to its hash (0xFFFFFFFF), as unused slots are.
    if hash == name_hash("") {
        return Some("");
    }
    let t = fgx_table();
    t.binary_search_by_key(&hash, |e| e.0).ok().map(|i| t[i].1)
}

/// All names in `data/fgx-names.txt` (one per line, `#` starts a comment). They are strings
/// harvested from gr2 files that keep their text (`harvest_fgx` example).
pub fn known_fgx_names() -> impl Iterator<Item = &'static str> {
    FGX_NAMES_TXT
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fgx_dictionary_has_no_hash_collisions() {
        let mut all: Vec<_> = known_fgx_names().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), fgx_table().len());
    }

    #[test]
    fn fgx_hash_is_inverted_crc32() {
        // The stripped-name hash is crc32(name) ^ 0xFFFFFFFF (checked against the game's stripped files).
        for n in known_fgx_names().chain(known_member_names()) {
            assert_eq!(
                name_hash(n),
                crate::crc32(n.as_bytes()) ^ 0xFFFF_FFFF,
                "{n}"
            );
        }
        assert_eq!(name_hash("ParamBlock"), 0x1d5f_0625);
        assert_eq!(name_hash("ParamFlags"), 0x9541_d0bd);
    }

    #[test]
    fn dictionary_resolves_core_names() {
        for n in ["Bones", "Duration", "Position", "TriFirst", "CurveData"] {
            assert_eq!(member_name(name_hash(n)), Some(n), "{n}");
        }
    }

    #[test]
    fn dictionary_has_no_hash_collisions() {
        let mut all: Vec<_> = known_member_names().collect();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), table().len());
    }
}
