//! Checks that span sections: every index in range and the sections agreeing with the manifest.

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::sections::manifest::Manifest;
use crate::sections::strings::StringTable;
use crate::sections::styles::Styles;
use crate::sections::templates::{Limits, Templates};
use crate::sections::tokens::Tokens;

/// The decoded sections of a package.
pub(crate) struct Parts<'a> {
    pub manifest: &'a Manifest,
    pub strings: &'a StringTable,
    pub templates: &'a Templates,
    pub styles: &'a Styles,
    pub tokens: &'a Tokens,
}

/// Runs the structural checks of `Package::read`.
pub(crate) fn check(p: &Parts<'_>) -> Result<(), PackageError> {
    p.manifest.validate(SectionTag::MANI)?;
    p.strings.validate(SectionTag::STRS)?;
    let strs = p.strings.items.len();
    p.styles.validate(SectionTag::STYL, strs)?;
    p.tokens.validate(SectionTag::TOKN, strs)?;
    let limits = Limits {
        strs,
        style_sets: p.styles.sets.len(),
        // The tables of BIND, IMGS and AREF are checked when those sections exist.
        exprs: usize::MAX,
        tpls: usize::MAX,
        imgs: usize::MAX,
        arefs: usize::MAX,
    };
    p.templates.validate(SectionTag::TMPL, &limits)?;

    // The themes of the manifest and of TOKN are the same list.
    let themes = &p.manifest.themes;
    if p.tokens.themes.len() != themes.len() {
        return Err(PackageError::invalid(
            SectionTag::TOKN,
            "the themes of the manifest and of the token table differ",
        ));
    }
    for (theme, name) in p.tokens.themes.iter().zip(themes) {
        if p.strings.items.get(theme.name.index()) != Some(name) {
            return Err(PackageError::invalid(
                SectionTag::TOKN,
                "a theme name differs from the manifest's",
            ));
        }
    }
    Ok(())
}
