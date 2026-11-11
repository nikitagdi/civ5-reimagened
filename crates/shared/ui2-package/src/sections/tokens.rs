//! `TOKN`: root color tokens, synonyms and themes.
//!
//! A color of a record is a literal or a token reference. A theme replaces the values of some root
//! tokens; switching the theme re-resolves the references without reloading the package.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::{StrId, TokenId};
use crate::style::ColorV;

/// One theme: overrides of root token values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Theme {
    /// The theme's name.
    pub name: StrId,
    /// Overridden tokens, strictly ascending by token.
    pub set: Vec<(TokenId, [u8; 4])>,
}

/// The section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// The root tokens (`--bone: #ebe2d2`) with the default theme's values; a `TokenId` is the
    /// index.
    pub roots: Vec<(StrId, [u8; 4])>,
    /// Synonyms: `--ui-text` is `--bone`. The synonyms mods rely on are listed by the profile.
    pub aliases: Vec<(StrId, TokenId)>,
    /// The themes; theme 0 is the default and overrides nothing.
    pub themes: Vec<Theme>,
}

impl Tokens {
    /// The color of a record color under a theme. A literal is itself; a token takes the theme's
    /// override or the default value. An unknown theme uses the defaults; an unknown token is
    /// transparent (the reader checks ids, so this does not happen for a read package).
    pub fn resolve(&self, theme: u16, color: ColorV) -> [u8; 4] {
        match color {
            ColorV::Lit(rgba) => rgba,
            ColorV::Tok(id) => {
                let overridden = self.themes.get(usize::from(theme)).and_then(|t| {
                    t.set
                        .binary_search_by_key(&id, |(token, _)| *token)
                        .ok()
                        .map(|i| t.set[i].1)
                });
                overridden
                    .or_else(|| self.roots.get(id.index()).map(|(_, rgba)| *rgba))
                    .unwrap_or([0, 0, 0, 0])
            }
        }
    }

    /// The token a synonym stands for.
    pub fn alias(&self, name: StrId) -> Option<TokenId> {
        self.aliases
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t)
    }

    pub(crate) fn validate(&self, at: SectionTag, strs: usize) -> Result<(), PackageError> {
        let bad = |what: &str| Err(PackageError::invalid(at, what));
        if u32::try_from(self.roots.len()).is_err() {
            return bad("too many tokens");
        }
        let mut names: Vec<StrId> = self.roots.iter().map(|(n, _)| *n).collect();
        if names.iter().any(|n| n.index() >= strs) {
            return bad("a token name is not in the string table");
        }
        names.extend(self.aliases.iter().map(|(n, _)| *n));
        names.sort_unstable();
        if names.windows(2).any(|w| w[0] == w[1]) {
            return bad("a token or synonym name is used twice");
        }
        if names.iter().any(|n| n.index() >= strs) {
            return bad("a synonym name is not in the string table");
        }
        if self
            .aliases
            .iter()
            .any(|(_, t)| t.index() >= self.roots.len())
        {
            return bad("a synonym points at a missing token");
        }
        for (i, theme) in self.themes.iter().enumerate() {
            if theme.name.index() >= strs {
                return bad("a theme name is not in the string table");
            }
            if theme.set.iter().any(|(t, _)| t.index() >= self.roots.len()) {
                return bad("a theme overrides a missing token");
            }
            if theme.set.windows(2).any(|w| w[0].0 >= w[1].0) {
                return bad("a theme's tokens are not strictly ascending");
            }
            if i == 0 && !theme.set.is_empty() {
                return bad("the default theme must not override anything");
            }
        }
        Ok(())
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Tokens {
    fn remap(&mut self, r: &crate::remap::Remap) {
        for (name, _) in &mut self.roots {
            r.str(name);
        }
        for (name, _) in &mut self.aliases {
            r.str(name);
        }
        for theme in &mut self.themes {
            r.str(&mut theme.name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens() -> Tokens {
        Tokens {
            roots: vec![(StrId(0), [10, 20, 30, 255]), (StrId(1), [1, 2, 3, 255])],
            aliases: vec![(StrId(2), TokenId(0))],
            themes: vec![
                Theme {
                    name: StrId(3),
                    set: vec![],
                },
                Theme {
                    name: StrId(4),
                    set: vec![(TokenId(1), [9, 9, 9, 255])],
                },
            ],
        }
    }

    #[test]
    fn resolve_uses_the_theme_then_the_default() {
        let t = tokens();
        assert_eq!(t.resolve(0, ColorV::Tok(TokenId(1))), [1, 2, 3, 255]);
        assert_eq!(t.resolve(1, ColorV::Tok(TokenId(1))), [9, 9, 9, 255]);
        assert_eq!(t.resolve(1, ColorV::Tok(TokenId(0))), [10, 20, 30, 255]);
        assert_eq!(t.resolve(7, ColorV::Tok(TokenId(1))), [1, 2, 3, 255]);
        assert_eq!(t.resolve(1, ColorV::Lit([5, 6, 7, 8])), [5, 6, 7, 8]);
        assert_eq!(t.resolve(0, ColorV::Tok(TokenId(99))), [0, 0, 0, 0]);
        assert_eq!(t.alias(StrId(2)), Some(TokenId(0)));
    }

    #[test]
    fn validate_catches_bad_references() {
        let at = SectionTag::TOKN;
        assert!(tokens().validate(at, 5).is_ok());
        assert!(tokens().validate(at, 4).is_err()); // theme name 4 is out of range
        let mut t = tokens();
        t.aliases[0].1 = TokenId(2);
        assert!(t.validate(at, 5).is_err());
        let mut t = tokens();
        t.themes[0].set.push((TokenId(0), [0; 4]));
        assert!(t.validate(at, 5).is_err());
        let mut t = tokens();
        t.themes[1].set = vec![(TokenId(1), [0; 4]), (TokenId(0), [0; 4])];
        assert!(t.validate(at, 5).is_err());
        let mut t = tokens();
        t.aliases[0].0 = StrId(0);
        assert!(t.validate(at, 5).is_err());
    }
}
