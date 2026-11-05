//! UI file index (`C5UI`, manifest id `ui:index`) and per-language text table (`C5LT`,
//! `ui:text:<lang>`).
//!
//! The index is the result of the pipeline's UI file system: one entry per bare file name (the
//! visible copy), the layers it was built from, the fonts, the icon atlases, the skin style files
//! and the load groups of the windows. The client looks files up by name without a directory,
//! as the original does.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{FormatError, blob};

pub const UI_INDEX_MAGIC: [u8; 4] = *b"C5UI";
pub const UI_INDEX_ID: &str = "ui:index";
pub const LOCALE_MAGIC: [u8; 4] = *b"C5LT";
pub const UI_TABLES_MAGIC: [u8; 4] = *b"C5TB";
pub const UI_TABLES_ID: &str = "ui:tables";

/// Manifest id of a UI file: `ui:f:` and the lower-case bare name.
pub fn ui_file_id(name: &str) -> String {
    format!("ui:f:{}", name.to_ascii_lowercase())
}

/// Manifest id of a UI texture: `ui:tx:` and the lower-case bare name.
pub fn ui_texture_id(name: &str) -> String {
    format!("ui:tx:{}", name.to_ascii_lowercase())
}

/// Manifest id of a UI sound file.
pub fn ui_sound_file_id(name: &str) -> String {
    format!("ui:au:{}", name.to_ascii_lowercase())
}

/// Manifest id of the text table of a language (`en_US` -> `ui:text:en_us`).
pub fn ui_text_id(lang: &str) -> String {
    format!("ui:text:{}", lang.to_ascii_lowercase())
}

/// Last path segment (either slash), lower case.
pub fn bare_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim()
        .to_ascii_lowercase()
}

/// What a file is. Postcard writes the variant index: new kinds go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiFileKind {
    Xml,
    Lua,
    /// `.ggxml` bitmap font description.
    Font,
    /// `.txt` and other plain text.
    Text,
    /// A texture; the id is `ui:tx:...`.
    Texture,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiLayerInfo {
    pub name: String,
    /// Folder relative to `Assets/` (`DLC/UI_bc1`), empty for the root.
    pub dir: String,
    pub priority: i32,
    /// Position in the override order, lowest first.
    pub order: u32,
    pub packages: Vec<String>,
    pub priority_defaulted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiFileInfo {
    /// Lower-case bare name: the key.
    pub name: String,
    /// Manifest id.
    pub id: String,
    pub kind: UiFileKind,
    /// Index into `layers`.
    pub layer: u32,
    pub size: u64,
    /// Path of the visible copy inside its layer.
    pub path: String,
    /// Copies this one hides.
    pub hidden: u32,
    /// For `X-index.dds`: the palette texture it indexes (`x.dds`).
    pub index_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiFont {
    /// `TwCenMT16`, `FontIcons`.
    pub name: String,
    /// Bare name of the `.ggxml`.
    pub ggxml: String,
    /// Bare names of the textures it names.
    pub textures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiAtlas {
    pub atlas: String,
    pub icon_size: u32,
    /// Bare name of the texture file.
    pub file: String,
    pub icons_per_row: u32,
    pub icons_per_column: u32,
}

/// Pixel size of a texture as stored. The client needs it before the texture has arrived (a control that takes a
/// texture by name lays out with its size); the rows of the atlas tables do not always give it (a mod's `IconsPerRow`
/// can be wrong).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiTexDim {
    /// Bare name of the texture file.
    pub file: String,
    pub width: u32,
    pub height: u32,
}

/// Load group of a window (or of the boot set, the HUD, an atlas).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiGroup {
    /// `ui:boot`, `ui:hud`, `ui:win:<context>`, `ui:atlas:<atlas>`.
    pub id: String,
    /// The closure: every manifest id the group needs, sorted, no repeats.
    pub members: Vec<String>,
    /// Bytes of the members as stored.
    pub bytes: u64,
    /// Groups to ask for together with this one.
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiIndex {
    pub layers: Vec<UiLayerInfo>,
    /// Sorted by `name`, no repeats.
    pub files: Vec<UiFileInfo>,
    pub fonts: Vec<UiFont>,
    pub atlases: Vec<UiAtlas>,
    /// Sizes of the textures, sorted by `file` (version 2).
    pub tex_dims: Vec<UiTexDim>,
    /// `Style` and `Color` files of the active skins, lowest priority first.
    pub skin_styles: Vec<String>,
    pub skin_colors: Vec<String>,
    /// Sorted by `id`, no repeats.
    pub groups: Vec<UiGroup>,
}

/// `C5UI` v1 layout: no `tex_dims`.
#[derive(Deserialize)]
struct UiIndexV1 {
    layers: Vec<UiLayerInfo>,
    files: Vec<UiFileInfo>,
    fonts: Vec<UiFont>,
    atlases: Vec<UiAtlas>,
    skin_styles: Vec<String>,
    skin_colors: Vec<String>,
    groups: Vec<UiGroup>,
}

impl UiIndex {
    pub const VERSION: u16 = 2;
    pub const MIN_VERSION: u16 = 1;

    /// Deterministic: files and groups are written sorted, members sorted without repeats.
    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.files.sort_by(|a, b| a.name.cmp(&b.name));
        t.tex_dims.sort_by(|a, b| a.file.cmp(&b.file));
        t.groups.sort_by(|a, b| a.id.cmp(&b.id));
        for g in &mut t.groups {
            g.members.sort();
            g.members.dedup();
            g.requires.sort();
            g.requires.dedup();
        }
        blob::encode_v(UI_INDEX_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (version, body) =
            blob::split_range(UI_INDEX_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let t: Self = if version == 1 {
            let v1: UiIndexV1 = blob::payload(body)?;
            Self {
                layers: v1.layers,
                files: v1.files,
                fonts: v1.fonts,
                atlases: v1.atlases,
                tex_dims: Vec::new(),
                skin_styles: v1.skin_styles,
                skin_colors: v1.skin_colors,
                groups: v1.groups,
            }
        } else {
            blob::payload(body)?
        };
        t.check()?;
        Ok(t)
    }

    pub fn check(&self) -> Result<(), FormatError> {
        for w in self.files.windows(2) {
            if w[0].name >= w[1].name {
                return Err(FormatError::invalid(format!(
                    "files are not sorted or repeat: {:?} then {:?}",
                    w[0].name, w[1].name
                )));
            }
        }
        for f in &self.files {
            if f.layer as usize >= self.layers.len() {
                return Err(FormatError::invalid(format!(
                    "file {:?} names layer {} of {}",
                    f.name,
                    f.layer,
                    self.layers.len()
                )));
            }
        }
        for w in self.tex_dims.windows(2) {
            if w[0].file >= w[1].file {
                return Err(FormatError::invalid(format!(
                    "texture sizes are not sorted or repeat: {:?} then {:?}",
                    w[0].file, w[1].file
                )));
            }
        }
        for w in self.groups.windows(2) {
            if w[0].id >= w[1].id {
                return Err(FormatError::invalid(format!(
                    "groups are not sorted or repeat: {:?} then {:?}",
                    w[0].id, w[1].id
                )));
            }
        }
        let ids: BTreeSet<&str> = self.groups.iter().map(|g| g.id.as_str()).collect();
        for g in &self.groups {
            if g.members.windows(2).any(|w| w[0] >= w[1]) {
                return Err(FormatError::invalid(format!(
                    "group {:?}: members are not sorted or repeat",
                    g.id
                )));
            }
            if let Some(r) = g.requires.iter().find(|r| !ids.contains(r.as_str())) {
                return Err(FormatError::invalid(format!(
                    "group {:?} requires {r:?}, which is not in the index",
                    g.id
                )));
            }
        }
        Ok(())
    }

    /// The visible copy of a bare file name, any case.
    pub fn find(&self, name: &str) -> Option<&UiFileInfo> {
        let key = name.trim().to_ascii_lowercase();
        self.files
            .binary_search_by(|f| f.name.as_str().cmp(key.as_str()))
            .ok()
            .map(|i| &self.files[i])
    }

    /// A path as XML, Lua or the DB write it (`assets\UI\Art\X.dds`): directories are ignored.
    pub fn find_path(&self, path: &str) -> Option<&UiFileInfo> {
        self.find(&bare_name(path))
    }

    /// `include("IconSupport")`: the first of `exts` the index has, in the order given.
    pub fn find_stem(&self, stem: &str, exts: &[&str]) -> Option<&UiFileInfo> {
        let stem = bare_name(stem);
        exts.iter()
            .find_map(|e| self.find(&format!("{stem}.{}", e.trim_start_matches('.'))))
    }

    pub fn group(&self, id: &str) -> Option<&UiGroup> {
        self.groups
            .binary_search_by(|g| g.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.groups[i])
    }

    /// The stored pixel size of a texture (a bare name, any case); `None` for an index without sizes (v1) or a name
    /// that is no texture.
    pub fn tex_dim(&self, file: &str) -> Option<(u32, u32)> {
        let key = file.trim().to_ascii_lowercase();
        self.tex_dims
            .binary_search_by(|d| d.file.as_str().cmp(key.as_str()))
            .ok()
            .map(|i| (self.tex_dims[i].width, self.tex_dims[i].height))
    }

    /// Atlas rows by name, every icon size.
    pub fn atlas_rows<'a>(&'a self, atlas: &'a str) -> impl Iterator<Item = &'a UiAtlas> + 'a {
        self.atlases.iter().filter(move |a| a.atlas == atlas)
    }
}

/// Texts of one language: tag to text, sorted by tag.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocaleTable {
    /// `en_US`, `ru_RU`.
    pub lang: String,
    pub entries: Vec<(String, String)>,
    /// The `Gender` and `Plurality` columns of the texts that have one, sorted by tag.
    pub grammar: Vec<LocaleGrammar>,
}

/// Raw `Gender` / `Plurality` column strings of one text (`neuter:an`, `1|2`); empty is absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocaleGrammar {
    pub tag: String,
    pub gender: String,
    pub plurality: String,
}

/// `C5LT` v1 layout: no grammar columns.
#[derive(Deserialize)]
struct LocaleTableV1 {
    lang: String,
    entries: Vec<(String, String)>,
}

impl LocaleTable {
    pub const VERSION: u16 = 2;
    pub const MIN_VERSION: u16 = 1;

    pub fn new(lang: impl Into<String>) -> Self {
        Self {
            lang: lang.into(),
            entries: Vec::new(),
            grammar: Vec::new(),
        }
    }

    /// Deterministic: entries and grammar rows written sorted by tag.
    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.entries.sort_by(|a, b| a.0.cmp(&b.0));
        t.grammar.sort_by(|a, b| a.tag.cmp(&b.tag));
        blob::encode_v(LOCALE_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (version, body) =
            blob::split_range(LOCALE_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let t: Self = if version == 1 {
            let v1: LocaleTableV1 = blob::payload(body)?;
            Self {
                lang: v1.lang,
                entries: v1.entries,
                grammar: Vec::new(),
            }
        } else {
            blob::payload(body)?
        };
        if let Some(w) = t.entries.windows(2).find(|w| w[0].0 >= w[1].0) {
            return Err(FormatError::invalid(format!(
                "texts are not sorted or repeat: {:?} then {:?}",
                w[0].0, w[1].0
            )));
        }
        if let Some(w) = t.grammar.windows(2).find(|w| w[0].tag >= w[1].tag) {
            return Err(FormatError::invalid(format!(
                "grammar rows are not sorted or repeat: {:?} then {:?}",
                w[0].tag, w[1].tag
            )));
        }
        Ok(t)
    }

    pub fn get(&self, tag: &str) -> Option<&str> {
        self.entries
            .binary_search_by(|e| e.0.as_str().cmp(tag))
            .ok()
            .map(|i| self.entries[i].1.as_str())
    }

    /// `(gender, plurality)` raw columns of a text; `None` for an empty column.
    pub fn grammar_of(&self, tag: &str) -> (Option<&str>, Option<&str>) {
        let Ok(i) = self.grammar.binary_search_by(|g| g.tag.as_str().cmp(tag)) else {
            return (None, None);
        };
        let g = &self.grammar[i];
        (
            Some(g.gender.as_str()).filter(|s| !s.is_empty()),
            Some(g.plurality.as_str()).filter(|s| !s.is_empty()),
        )
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// A row of `IconFontMapping`: `[ICON_<name>]` is glyph `glyph` of the font `texture` names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiIconMapping {
    pub name: String,
    pub glyph: u32,
    /// `IconFontTexture` key (`ICON_FONT_TEXTURE_DEFAULT`).
    pub texture: String,
}

/// A row of `Languages`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiLanguage {
    pub lang: String,
    pub plural_rule: u32,
}

/// Game tables the UI text needs (`C5TB`, manifest id `ui:tables`), merged over every layer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UiTables {
    /// `IconFontTextures`: key to the `.ggxml` file stem, sorted by key.
    pub icon_textures: Vec<(String, String)>,
    /// Sorted by name.
    pub icons: Vec<UiIconMapping>,
    /// `Colors`: `COLOR_*` to red, green, blue, alpha in 0..=1 as written, sorted by name.
    pub colors: Vec<(String, [f32; 4])>,
    /// Sorted by language.
    pub languages: Vec<UiLanguage>,
}

impl UiTables {
    pub const VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.icon_textures.sort_by(|a, b| a.0.cmp(&b.0));
        t.icons.sort_by(|a, b| a.name.cmp(&b.name));
        t.colors.sort_by(|a, b| a.0.cmp(&b.0));
        t.languages.sort_by(|a, b| a.lang.cmp(&b.lang));
        blob::encode_v(UI_TABLES_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, t): (u16, Self) =
            blob::decode_range(UI_TABLES_MAGIC, Self::VERSION, Self::VERSION, bytes)?;
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, layer: u32) -> UiFileInfo {
        UiFileInfo {
            name: name.into(),
            id: ui_file_id(name),
            kind: UiFileKind::Lua,
            layer,
            size: 3,
            path: format!("UI/{name}"),
            hidden: 0,
            index_of: None,
        }
    }

    fn index() -> UiIndex {
        UiIndex {
            layers: vec![UiLayerInfo {
                name: "base".into(),
                dir: String::new(),
                priority: 0,
                order: 0,
                packages: vec![],
                priority_defaulted: false,
            }],
            files: vec![file("topPanel.lua", 0), file("iconsupport.lua", 0)],
            groups: vec![
                UiGroup {
                    id: "ui:win:b".into(),
                    members: vec!["ui:tx:b".into(), "ui:f:a".into(), "ui:f:a".into()],
                    bytes: 5,
                    requires: vec!["ui:boot".into()],
                },
                UiGroup {
                    id: "ui:boot".into(),
                    members: vec![],
                    bytes: 0,
                    requires: vec![],
                },
            ],
            ..UiIndex::default()
        }
    }

    #[test]
    fn round_trips_sorted_and_deterministic() {
        let bytes = index().encode();
        assert_eq!(&bytes[..4], b"C5UI");
        let back = UiIndex::decode(&bytes).unwrap();
        assert_eq!(back.files[0].name, "iconsupport.lua");
        assert_eq!(back.groups[0].id, "ui:boot");
        assert_eq!(
            back.group("ui:win:b").unwrap().members,
            ["ui:f:a", "ui:tx:b"]
        );
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn lookup_by_bare_name_any_case_and_stem() {
        // `topPanel.lua` is keyed lower case by the pipeline; the index sorts what it is given.
        let mut idx = index();
        idx.files[0].name = "toppanel.lua".into();
        let idx = UiIndex::decode(&idx.encode()).unwrap();
        assert!(idx.find("TopPanel.LUA").is_some());
        assert!(idx.find_path("Assets\\UI\\InGame\\TOPPANEL.lua").is_some());
        assert_eq!(
            idx.find_stem("IconSupport", &["xml", "lua"]).unwrap().name,
            "iconsupport.lua"
        );
        assert!(idx.find("nothing.lua").is_none());
    }

    #[test]
    fn bad_layer_unsorted_files_and_unknown_requirement_are_refused() {
        let mut bad = index();
        bad.files.sort_by(|a, b| a.name.cmp(&b.name));
        bad.groups.sort_by(|a, b| a.id.cmp(&b.id));
        for g in &mut bad.groups {
            g.members.sort();
            g.members.dedup();
        }
        bad.files[0].layer = 4;
        let bytes = blob::encode_v(UI_INDEX_MAGIC, UiIndex::VERSION, &bad);
        assert!(
            UiIndex::decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("layer")
        );

        let mut bad = index();
        bad.files.sort_by(|a, b| b.name.cmp(&a.name));
        let bytes = blob::encode_v(UI_INDEX_MAGIC, UiIndex::VERSION, &bad);
        assert!(UiIndex::decode(&bytes).is_err());

        let mut bad = index();
        bad.groups[0].requires = vec!["ui:nope".into()];
        bad.groups.sort_by(|a, b| a.id.cmp(&b.id));
        for g in &mut bad.groups {
            g.members.sort();
            g.members.dedup();
        }
        bad.files.sort_by(|a, b| a.name.cmp(&b.name));
        let bytes = blob::encode_v(UI_INDEX_MAGIC, UiIndex::VERSION, &bad);
        assert!(
            UiIndex::decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("ui:nope")
        );
    }

    #[test]
    fn texture_sizes_round_trip_and_a_v1_index_reads_without_them() {
        let mut idx = index();
        idx.tex_dims = vec![
            UiTexDim {
                file: "b.dds".into(),
                width: 8,
                height: 4,
            },
            UiTexDim {
                file: "a.dds".into(),
                width: 2,
                height: 2,
            },
        ];
        let back = UiIndex::decode(&idx.encode()).unwrap();
        assert_eq!(back.tex_dim("A.DDS"), Some((2, 2)));
        assert_eq!(back.tex_dim("b.dds"), Some((8, 4)));
        assert_eq!(back.tex_dim("c.dds"), None);
        // The layout of version 1: the same fields without `tex_dims`.
        #[derive(Serialize)]
        struct V1<'a> {
            layers: &'a [UiLayerInfo],
            files: &'a [UiFileInfo],
            fonts: &'a [UiFont],
            atlases: &'a [UiAtlas],
            skin_styles: &'a [String],
            skin_colors: &'a [String],
            groups: &'a [UiGroup],
        }
        let src = index();
        let sorted = UiIndex::decode(&src.encode()).unwrap();
        let v1 = blob::encode_v(
            UI_INDEX_MAGIC,
            1,
            &V1 {
                layers: &sorted.layers,
                files: &sorted.files,
                fonts: &sorted.fonts,
                atlases: &sorted.atlases,
                skin_styles: &sorted.skin_styles,
                skin_colors: &sorted.skin_colors,
                groups: &sorted.groups,
            },
        );
        let old = UiIndex::decode(&v1).unwrap();
        assert!(old.tex_dims.is_empty());
        assert_eq!(old.files, sorted.files);
    }

    #[test]
    fn locale_round_trip_and_lookup() {
        let mut t = LocaleTable::new("ru_RU");
        t.entries.push(("TXT_KEY_B".into(), "б".into()));
        t.entries.push(("TXT_KEY_A".into(), "а".into()));
        let bytes = t.encode();
        assert_eq!(&bytes[..4], b"C5LT");
        let back = LocaleTable::decode(&bytes).unwrap();
        assert_eq!(back.get("TXT_KEY_A"), Some("а"));
        assert_eq!(back.get("TXT_KEY_C"), None);
        assert_eq!(back.entries[0].0, "TXT_KEY_A");
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn locale_grammar_and_v1_blobs() {
        let mut t = LocaleTable::new("en_US");
        t.entries.push(("TXT_B".into(), "b".into()));
        t.grammar.push(LocaleGrammar {
            tag: "TXT_B".into(),
            gender: "neuter:an".into(),
            plurality: String::new(),
        });
        let back = LocaleTable::decode(&t.encode()).unwrap();
        assert_eq!(back.grammar_of("TXT_B"), (Some("neuter:an"), None));
        assert_eq!(back.grammar_of("TXT_A"), (None, None));
        // A v1 blob (lang, entries) still reads.
        let v1 = blob::encode_v(
            LOCALE_MAGIC,
            1,
            &(
                String::from("en_US"),
                vec![("A".to_string(), "1".to_string())],
            ),
        );
        let old = LocaleTable::decode(&v1).unwrap();
        assert_eq!(old.get("A"), Some("1"));
        assert!(old.grammar.is_empty());
    }

    #[test]
    fn tables_round_trip_sorted() {
        let t = UiTables {
            icon_textures: vec![("B".into(), "FontB".into()), ("A".into(), "FontA".into())],
            icons: vec![UiIconMapping {
                name: "ICON_GOLD".into(),
                glyph: 3,
                texture: "A".into(),
            }],
            colors: vec![("COLOR_RED".into(), [1.0, 0.0, 0.0, 1.0])],
            languages: vec![UiLanguage {
                lang: "en_US".into(),
                plural_rule: 2,
            }],
        };
        let bytes = t.encode();
        assert_eq!(&bytes[..4], b"C5TB");
        let back = UiTables::decode(&bytes).unwrap();
        assert_eq!(back.icon_textures[0].0, "A");
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn locale_with_repeats_is_refused() {
        let mut t = LocaleTable::new("en_US");
        t.entries = vec![("A".into(), "1".into()), ("A".into(), "2".into())];
        let bytes = blob::encode_v(LOCALE_MAGIC, LocaleTable::VERSION, &t);
        assert!(LocaleTable::decode(&bytes).is_err());
    }

    #[test]
    fn ids_are_lower_case() {
        assert_eq!(ui_file_id("TopPanel.LUA"), "ui:f:toppanel.lua");
        assert_eq!(ui_texture_id("A.DDS"), "ui:tx:a.dds");
        assert_eq!(ui_text_id("ru_RU"), "ui:text:ru_ru");
        assert_eq!(ui_sound_file_id("X.wav"), "ui:au:x.wav");
    }
}
