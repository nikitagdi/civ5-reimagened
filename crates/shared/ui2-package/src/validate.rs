//! Checks that span sections: every index in range and the sections agreeing with the manifest.
//!
//! `Package::read` runs, in order: the container (magic, format major, size, hash, section table:
//! count, order, alignment, extent, padding), the decoding of each section (no byte left over), then
//! [`check`] with each section's own rules (sorted tables, subtree tiling, style limits, finite
//! numbers, blob extents) and the references between sections: strings, templates, style sets and
//! records, expressions, text templates, tokens, pictures, asset references and nodes. A body
//! template of `for`/`if` must come after the template that uses it. Versions and model paths are
//! compared with the client's afterwards (`Package::check_compat`, `Package::check_paths`), and the
//! hashes of fonts and pages when the host calls `Package::verify_blobs`.

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::sections::bindings::Bindings;
use crate::sections::blobs::{ARefs, Fonts, Images, Scripts};
use crate::sections::extn::Extn;
use crate::sections::l10n::L10n;
use crate::sections::manifest::Manifest;
use crate::sections::srcmap::SrcMap;
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
    pub bindings: &'a Bindings,
    pub l10n: &'a L10n,
    pub extn: &'a Extn,
    pub srcmap: Option<&'a SrcMap>,
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
        exprs: p.bindings.exprs.len(),
        tpls: p.bindings.tpls.len(),
        imgs: p.images.imgs.len(),
        arefs: p.arefs.items.len(),
    };
    p.templates.validate(SectionTag::TMPL, &limits)?;
    p.bindings.validate(SectionTag::BIND, strs, p.templates)?;
    p.l10n.validate(SectionTag::L10N, strs, p.manifest)?;
    p.extn.validate(SectionTag::EXTN, strs, p.templates)?;
    if let Some(map) = p.srcmap {
        map.validate(SectionTag::SRCM, p.templates)?;
    }

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
