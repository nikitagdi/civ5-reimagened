//! UI files by bare name: the client side of the `ui` set.
//!
//! The pipeline resolved the layers (`Civ5Pkg` priority) when it built `ui:index`; the client
//! only looks a name up. Directories in a path are ignored and case does not matter, as in
//! the original. The bytes come through the usual loader and streamer by the returned id.

use asset_format::{
    FormatError, LocaleTable, LogicalId, UI_INDEX_ID, UiFileInfo, UiIndex, ui_text_id,
};

/// The decoded UI index with id-returning lookups.
#[derive(Debug, Clone)]
pub struct UiFiles {
    index: UiIndex,
}

impl UiFiles {
    /// Manifest id of the index blob.
    pub const INDEX_ID: &'static str = UI_INDEX_ID;
    /// The group to load before the first UI frame.
    pub const BOOT_GROUP: &'static str = "ui:boot";
    /// The group of the windows shown on the map screen.
    pub const HUD_GROUP: &'static str = "ui:hud";

    pub fn open(index_bytes: &[u8]) -> Result<Self, FormatError> {
        Ok(Self {
            index: UiIndex::decode(index_bytes)?,
        })
    }

    pub fn index(&self) -> &UiIndex {
        &self.index
    }

    /// `assets\UI\Art\X.dds`, `x.dds`, `Assets/UI/InGame/TopPanel.lua`.
    pub fn find_path(&self, path: &str) -> Option<&UiFileInfo> {
        self.index.find_path(path)
    }

    /// Manifest id of the file a path names.
    pub fn id_of_path(&self, path: &str) -> Option<LogicalId> {
        self.find_path(path).map(|f| LogicalId::new(f.id.clone()))
    }

    /// `include("IconSupport")`: the first extension that exists.
    pub fn id_of_stem(&self, stem: &str, exts: &[&str]) -> Option<LogicalId> {
        self.index
            .find_stem(stem, exts)
            .map(|f| LogicalId::new(f.id.clone()))
    }

    /// Ids of a group and of everything it requires, each once, requirements first.
    pub fn group_closure(&self, group: &str) -> Option<Vec<LogicalId>> {
        self.index.group(group)?;
        // Requirements before the group that needs them.
        let mut order: Vec<&str> = Vec::new();
        fn post<'a>(
            idx: &'a UiIndex,
            g: &'a str,
            seen: &mut Vec<&'a str>,
            order: &mut Vec<&'a str>,
        ) {
            if seen.contains(&g) {
                return;
            }
            seen.push(g);
            if let Some(info) = idx.group(g) {
                for r in &info.requires {
                    post(idx, r, seen, order);
                }
            }
            order.push(g);
        }
        post(&self.index, group, &mut Vec::new(), &mut order);
        let mut seen = std::collections::BTreeSet::new();
        let mut out = Vec::new();
        for g in order {
            if let Some(info) = self.index.group(g) {
                for m in &info.members {
                    if seen.insert(m.as_str()) {
                        out.push(LogicalId::new(m.clone()));
                    }
                }
            }
        }
        Some(out)
    }

    /// Manifest id of the text table of a language.
    pub fn text_id(lang: &str) -> LogicalId {
        LogicalId::new(ui_text_id(lang))
    }

    /// Group id of a window by its context name (`TopPanel` or a path).
    pub fn window_group(context: &str) -> String {
        let bare = asset_format::bare_name(context);
        let stem = bare
            .strip_suffix(".xml")
            .or_else(|| bare.strip_suffix(".lua"))
            .unwrap_or(&bare);
        format!("ui:win:{stem}")
    }
}

/// Texts of one language.
pub fn decode_texts(bytes: &[u8]) -> Result<LocaleTable, FormatError> {
    LocaleTable::decode(bytes)
}

#[cfg(test)]
mod tests {
    use asset_format::{UiFileKind, UiGroup, UiLayerInfo};

    use super::*;

    fn index() -> Vec<u8> {
        let f = |n: &str, id: &str| UiFileInfo {
            name: n.into(),
            id: id.into(),
            kind: UiFileKind::Lua,
            layer: 0,
            size: 1,
            path: n.into(),
            hidden: 0,
            index_of: None,
        };
        UiIndex {
            layers: vec![UiLayerInfo {
                name: "base".into(),
                dir: String::new(),
                priority: 0,
                order: 0,
                packages: vec![],
                priority_defaulted: false,
            }],
            files: vec![
                f("iconsupport.lua", "ui:f:iconsupport.lua"),
                f("x.dds", "ui:tx:x.dds"),
            ],
            groups: vec![
                UiGroup {
                    id: "ui:boot".into(),
                    members: vec!["ui:f:iconsupport.lua".into()],
                    bytes: 1,
                    requires: vec![],
                },
                UiGroup {
                    id: "ui:atlas:A".into(),
                    members: vec!["ui:tx:x.dds".into()],
                    bytes: 1,
                    requires: vec![],
                },
                UiGroup {
                    id: "ui:win:top".into(),
                    members: vec!["ui:f:top.xml".into(), "ui:tx:x.dds".into()],
                    bytes: 2,
                    requires: vec!["ui:boot".into(), "ui:atlas:A".into()],
                },
            ],
            ..UiIndex::default()
        }
        .encode()
    }

    #[test]
    fn path_lookup_ignores_directories_and_case() {
        let u = UiFiles::open(&index()).unwrap();
        assert_eq!(
            u.id_of_path("Assets\\UI\\Art\\X.DDS"),
            Some(LogicalId::new("ui:tx:x.dds"))
        );
        assert_eq!(
            u.id_of_stem("IconSupport", &["xml", "lua"]),
            Some(LogicalId::new("ui:f:iconsupport.lua"))
        );
        assert_eq!(u.id_of_path("nope.lua"), None);
    }

    #[test]
    fn a_group_closure_lists_requirements_first_without_repeats() {
        let u = UiFiles::open(&index()).unwrap();
        let ids: Vec<String> = u
            .group_closure("ui:win:top")
            .unwrap()
            .iter()
            .map(|i| i.as_str().to_owned())
            .collect();
        // Requirements are kept sorted by id: the atlas comes before `ui:boot`.
        assert_eq!(ids, ["ui:tx:x.dds", "ui:f:iconsupport.lua", "ui:f:top.xml"]);
        assert!(u.group_closure("ui:win:none").is_none());
    }

    #[test]
    fn window_group_names() {
        assert_eq!(
            UiFiles::window_group("Assets/UI/InGame/TopPanel"),
            "ui:win:toppanel"
        );
        assert_eq!(UiFiles::window_group("TopPanel.lua"), "ui:win:toppanel");
        assert_eq!(UiFiles::text_id("ru_RU").as_str(), "ui:text:ru_ru");
    }
}
