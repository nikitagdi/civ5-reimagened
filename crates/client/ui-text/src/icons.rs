//! Icon fonts and named colors: the tables behind `[ICON_*]` and `[COLOR_*]`.

use std::collections::HashMap;

/// RGBA8, non-premultiplied, byte order r, g, b, a.
pub type Rgba = [u8; 4];

/// Where icon-font `.ggxml` files come from. Fonts are loaded on first use of one of their icons.
pub trait FontSource {
    /// The `.ggxml` text for a font file stem such as `FontIcons` or `FontIcons_Expansion2`.
    fn ggxml(&self, file_stem: &str) -> Option<String>;
}

/// `IconFontTextures` + `IconFontMapping`: icon name -> (font file, glyph code).
#[derive(Debug, Clone, Default)]
pub struct IconTable {
    /// `IconFontTexture` key -> `IconFontTextureFile`.
    files: HashMap<String, String>,
    /// `IconName` -> (`IconFontTexture` key, `IconMapping`).
    icons: HashMap<String, (String, u32)>,
}

impl IconTable {
    pub fn new() -> IconTable {
        IconTable::default()
    }

    /// A row of `IconFontTextures`.
    pub fn add_texture(&mut self, key: &str, file_stem: &str) {
        self.files.insert(key.to_string(), file_stem.to_string());
    }

    /// A row of `IconFontMapping`.
    pub fn add_icon(&mut self, name: &str, mapping: u32, texture_key: &str) {
        self.icons
            .insert(name.to_string(), (texture_key.to_string(), mapping));
    }

    /// Font file stem and glyph code of an icon name such as `ICON_GOLD`.
    pub fn get(&self, name: &str) -> Option<(&str, u32)> {
        let (key, glyph) = self.icons.get(name)?;
        Some((self.files.get(key)?.as_str(), *glyph))
    }

    /// The `IconFontMapping` row of an icon, whether or not its texture key is defined:
    /// (`IconFontTexture` key, glyph code).
    pub fn mapping(&self, name: &str) -> Option<(&str, u32)> {
        self.icons.get(name).map(|(k, g)| (k.as_str(), *g))
    }

    /// `IconFontTexture` keys that have a file.
    pub fn has_texture(&self, key: &str) -> bool {
        self.files.contains_key(key)
    }

    pub fn len(&self) -> usize {
        self.icons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }

    /// Every icon name.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.icons.keys().map(String::as_str)
    }
}

/// The `Colors` table: `COLOR_*` name -> RGBA8.
#[derive(Debug, Clone, Default)]
pub struct ColorTable {
    colors: HashMap<String, Rgba>,
}

/// A color channel in 0..=1 as the exe packs it: `(int)(v * 255)`, truncated.
pub fn channel_u8(v: f32) -> u8 {
    (v * 255.0) as i32 as u8
}

impl ColorTable {
    pub fn new() -> ColorTable {
        ColorTable::default()
    }

    /// A row of `Colors` (`Red`, `Green`, `Blue`, `Alpha` in 0..=1).
    pub fn add(&mut self, name: &str, rgba: [f32; 4]) {
        // re: 0x086cc490 UIManager::GetGameplayColorByName (each channel `(int)(v * 255)`)
        self.colors.insert(
            name.to_string(),
            [
                channel_u8(rgba[0]),
                channel_u8(rgba[1]),
                channel_u8(rgba[2]),
                channel_u8(rgba[3]),
            ],
        );
    }

    pub fn add_rgba8(&mut self, name: &str, rgba: Rgba) {
        self.colors.insert(name.to_string(), rgba);
    }

    pub fn get(&self, name: &str) -> Option<Rgba> {
        self.colors.get(name).copied()
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }
}

/// Control colors in 0..=1 to RGBA8 the way `ParseTextIntoBlocks` packs them.
pub fn pack_control_color(c: [f32; 4]) -> Rgba {
    // re: 0x08699e96 TextControl::ParseTextIntoBlocks (the three layer colors, * 255 and truncated)
    [
        channel_u8(c[0]),
        channel_u8(c[1]),
        channel_u8(c[2]),
        channel_u8(c[3]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_resolve_through_the_texture_key() {
        let mut t = IconTable::new();
        t.add_texture("ICON_FONT_TEXTURE_DEFAULT", "FontIcons");
        t.add_icon("ICON_GOLD", 4, "ICON_FONT_TEXTURE_DEFAULT");
        t.add_icon("ICON_ORPHAN", 5, "NO_SUCH_TEXTURE");
        assert_eq!(t.get("ICON_GOLD"), Some(("FontIcons", 4)));
        assert_eq!(t.get("ICON_ORPHAN"), None);
        assert_eq!(t.get("ICON_NOPE"), None);
    }

    #[test]
    fn colors_truncate() {
        let mut c = ColorTable::new();
        c.add("COLOR_RED", [1.0, 0.0, 0.0, 1.0]);
        c.add("COLOR_GREY", [0.5, 0.5, 0.5, 0.45]);
        assert_eq!(c.get("COLOR_RED"), Some([255, 0, 0, 255]));
        assert_eq!(c.get("COLOR_GREY"), Some([127, 127, 127, 114]));
        assert_eq!(pack_control_color([1.0, 1.0, 1.0, 1.0]), [255; 4]);
    }
}
