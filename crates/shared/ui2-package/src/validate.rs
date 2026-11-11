//! Checks that span sections: every index in range and the sections agreeing with the manifest.

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::sections::blobs::{ARefs, Fonts, Images, Scripts};
use crate::sections::manifest::Manifest;
use crate::sections::strings::StringTable;
use crate::sections::styles::Styles;
use crate::sections::templates::{Limits, Templates};
use crate::sections::tokens::Tokens;

/// The decoded sections of a package, with the byte-area lengths of the blob sections.
pub(crate) struct Parts<'a> {
    pub manifest: &'a Manifest,
    pub strings: &'a StringTable,
    pub templates: &'a Templates,
    pub styles: &'a Styles,
    pub tokens: &'a Tokens,
    pub fonts: &'a Fonts,
    pub images: &'a Images,
    pub scripts: &'a Scripts,
    pub arefs: &'a ARefs,
    pub font_area: usize,
    pub image_area: usize,
    pub script_area: usize,
}

/// Runs the structural checks of `Package::read`.
pub(crate) fn check(p: &Parts<'_>) -> Result<(), PackageError> {
    p.manifest.validate(SectionTag::MANI)?;
    p.strings.validate(SectionTag::STRS)?;
    let strs = p.strings.items.len();
    p.styles.validate(SectionTag::STYL, strs)?;
    p.tokens.validate(SectionTag::TOKN, strs)?;
    p.fonts.validate(SectionTag::FONT, strs, p.font_area)?;
    p.images.validate(SectionTag::IMGS, strs, p.image_area)?;
    p.scripts.validate(SectionTag::SCRP, strs, p.script_area)?;
    p.arefs.validate(SectionTag::AREF, strs)?;
    let limits = Limits {
        strs,
        style_sets: p.styles.sets.len(),
        // The tables of BIND are checked when that section exists.
        exprs: usize::MAX,
        tpls: usize::MAX,
        imgs: p.images.imgs.len(),
        arefs: p.arefs.items.len(),
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
