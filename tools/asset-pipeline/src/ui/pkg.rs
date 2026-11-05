//! `*.Civ5Pkg` content packages as data.
//!
//! The list of packages is whatever `*.Civ5Pkg` files the install holds; nothing here names a
//! package. Only the fields the UI file set needs are read.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("{path}: {msg}")]
    Parse { path: PathBuf, msg: String },
    #[error("i/o error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// `<Skin>` (front end) or `<GameplaySkin>` (in game) inside a `<UISkin>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkinSection {
    pub gameplay: bool,
    pub directories: Vec<String>,
    pub files: Vec<String>,
    pub styles: Vec<String>,
    pub colors: Vec<String>,
    pub paks: Vec<String>,
}

/// `<UISkin name set platform>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiSkin {
    pub name: String,
    /// `BaseGame`, `Expansion1`, `Expansion2`; empty when the attribute is absent.
    pub set: String,
    /// `Common` or `Tablet`; empty when absent.
    pub platform: String,
    pub sections: Vec<SkinSection>,
}

/// One parsed `*.Civ5Pkg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageInfo {
    pub name: String,
    /// `<Priority>`; `None` when the package has none.
    pub priority: Option<i32>,
    pub skins: Vec<UiSkin>,
    /// Every `<Paks><File>` of the package, wherever it stands, in file order.
    pub paks: Vec<String>,
    pub text_data: Vec<String>,
    pub audio_scripts_2d: Vec<String>,
    pub audio_defines: Vec<String>,
}

/// A package found in the install: the parsed file and where it lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    /// Path of the `.Civ5Pkg` file.
    pub file: PathBuf,
    /// Folder of the package (the folder of the file).
    pub dir: PathBuf,
    /// File name without the extension.
    pub stem: String,
    pub info: PackageInfo,
}

/// Which UI skins count as active: sets by name (lower case) and one platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activation {
    pub sets: BTreeSet<String>,
    /// Lower case.
    pub platform: String,
}

impl Activation {
    /// `BaseGame` < `Expansion1` < `Expansion2`: the named set and all below it.
    pub fn up_to(set: &str) -> Option<Self> {
        let order = ["basegame", "expansion1", "expansion2"];
        let at = order.iter().position(|s| s.eq_ignore_ascii_case(set))?;
        Some(Self {
            sets: order[..=at].iter().map(|s| (*s).to_owned()).collect(),
            platform: "common".to_owned(),
        })
    }

    fn skin_is_active(&self, skin: &UiSkin) -> bool {
        let set_ok = skin.set.is_empty() || self.sets.contains(&skin.set.to_ascii_lowercase());
        let platform_ok =
            skin.platform.is_empty() || skin.platform.eq_ignore_ascii_case(&self.platform);
        set_ok && platform_ok
    }
}

impl PackageInfo {
    /// A package without any `<UISkin>` is always active; otherwise one skin must be.
    pub fn is_active(&self, act: &Activation) -> bool {
        self.skins.is_empty() || self.skins.iter().any(|s| act.skin_is_active(s))
    }

    fn active_skins<'a>(&'a self, act: &'a Activation) -> impl Iterator<Item = &'a UiSkin> {
        self.skins.iter().filter(|s| act.skin_is_active(s))
    }

    /// Paths (lower case, `/`) that the skins of one activity state name.
    fn named_paths(&self, act: &Activation, active: bool) -> BTreeSet<String> {
        self.skins
            .iter()
            .filter(|s| act.skin_is_active(s) == active)
            .flat_map(|s| s.sections.iter())
            .flat_map(|sec| sec.directories.iter().chain(sec.files.iter()))
            .map(|p| norm_rel(p))
            .filter(|p| !p.is_empty())
            .collect()
    }

    /// `Directory` entries (lower case, `/`) of the active skins.
    pub fn active_directories(&self, act: &Activation) -> BTreeSet<String> {
        self.active_skins(act)
            .flat_map(|s| s.sections.iter())
            .flat_map(|sec| sec.directories.iter())
            .map(|p| norm_rel(p))
            .filter(|p| !p.is_empty())
            .collect()
    }

    /// Paths (lower case, `/`) that active skins name.
    pub fn active_paths(&self, act: &Activation) -> BTreeSet<String> {
        self.named_paths(act, true)
    }

    /// Paths (lower case, `/`) that only inactive skins name: they leave the package folder.
    /// A path an active skin names too stays.
    pub fn excluded_paths(&self, act: &Activation) -> BTreeSet<String> {
        let keep = self.named_paths(act, true);
        self.named_paths(act, false)
            .into_iter()
            .filter(|p| !keep.contains(p))
            .collect()
    }

    /// Packs named by the package, except those only an inactive skin names.
    pub fn active_paks(&self, act: &Activation) -> Vec<String> {
        let mut inactive_only: BTreeSet<String> = BTreeSet::new();
        for s in self.skins.iter().filter(|s| !act.skin_is_active(s)) {
            for sec in &s.sections {
                inactive_only.extend(sec.paks.iter().map(|p| p.to_ascii_lowercase()));
            }
        }
        for s in self.active_skins(act) {
            for sec in &s.sections {
                for p in &sec.paks {
                    inactive_only.remove(&p.to_ascii_lowercase());
                }
            }
        }
        let mut seen = BTreeSet::new();
        self.paks
            .iter()
            .filter(|p| !inactive_only.contains(&p.to_ascii_lowercase()))
            .filter(|p| seen.insert(p.to_ascii_lowercase()))
            .cloned()
            .collect()
    }

    /// `Style` and `Color` files of the active skins, in file order (sections `Skin`, then
    /// `GameplaySkin` as written).
    pub fn active_style_files(&self, act: &Activation) -> (Vec<String>, Vec<String>) {
        let (mut styles, mut colors) = (Vec::new(), Vec::new());
        for s in self.active_skins(act) {
            for sec in &s.sections {
                for f in &sec.styles {
                    if !styles.contains(f) {
                        styles.push(f.clone());
                    }
                }
                for f in &sec.colors {
                    if !colors.contains(f) {
                        colors.push(f.clone());
                    }
                }
            }
        }
        (styles, colors)
    }
}

/// Lower case, `\` to `/`, no leading or trailing `/`.
pub fn norm_rel(p: &str) -> String {
    p.trim()
        .replace('\\', "/")
        .trim_matches('/')
        .to_ascii_lowercase()
}

fn text_of(n: Node<'_, '_>) -> String {
    n.text().unwrap_or("").trim().to_owned()
}

fn children_text(n: Node<'_, '_>, tag: &str) -> Vec<String> {
    n.children()
        .filter(|c| c.is_element() && c.tag_name().name() == tag)
        .map(text_of)
        .filter(|s| !s.is_empty())
        .collect()
}

fn read_section(n: Node<'_, '_>, gameplay: bool) -> SkinSection {
    let mut sec = SkinSection {
        gameplay,
        ..SkinSection::default()
    };
    sec.directories = children_text(n, "Directory");
    sec.files = children_text(n, "File");
    sec.styles = children_text(n, "Style");
    sec.colors = children_text(n, "Color");
    for paks in n
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "Paks")
    {
        sec.paks.extend(children_text(paks, "File"));
    }
    sec
}

/// Parses the text of a `.Civ5Pkg`.
pub fn parse(text: &str) -> Result<PackageInfo, String> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = Document::parse(text).map_err(|e| e.to_string())?;
    let root = doc.root_element();
    if root.tag_name().name() != "Civ5Package" {
        return Err(format!("root element is {}", root.tag_name().name()));
    }
    let mut info = PackageInfo {
        name: String::new(),
        priority: None,
        skins: Vec::new(),
        paks: Vec::new(),
        text_data: Vec::new(),
        audio_scripts_2d: Vec::new(),
        audio_defines: Vec::new(),
    };
    for n in root.children().filter(Node::is_element) {
        match n.tag_name().name() {
            "Priority" => {
                let t = text_of(n);
                info.priority = Some(
                    t.parse::<i32>()
                        .map_err(|_| format!("Priority {t:?} is not a number"))?,
                );
            }
            "Name" => {
                // The en_US value, else the first one.
                let values: Vec<_> = n
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "Value")
                    .collect();
                let pick = values
                    .iter()
                    .find(|v| v.attribute("language") == Some("en_US"))
                    .or(values.first());
                if let Some(v) = pick {
                    info.name = text_of(*v);
                }
            }
            "UISkin" => {
                let mut skin = UiSkin {
                    name: n.attribute("name").unwrap_or("").to_owned(),
                    set: n.attribute("set").unwrap_or("").to_owned(),
                    platform: n.attribute("platform").unwrap_or("").to_owned(),
                    sections: Vec::new(),
                };
                for c in n.children().filter(Node::is_element) {
                    match c.tag_name().name() {
                        "Skin" => skin.sections.push(read_section(c, false)),
                        "GameplaySkin" => skin.sections.push(read_section(c, true)),
                        _ => {}
                    }
                }
                info.skins.push(skin);
            }
            _ => {}
        }
    }
    // Lists that may stand under any element (`Gameplay`, `Common`, `UISkin`).
    for n in root.descendants().filter(Node::is_element) {
        match n.tag_name().name() {
            "Paks" => info.paks.extend(children_text(n, "File")),
            "TextData" => {
                let t = text_of(n);
                if !t.is_empty() {
                    info.text_data.push(t);
                }
            }
            "AudioScripts2D" => {
                let t = text_of(n);
                if !t.is_empty() {
                    info.audio_scripts_2d.push(t);
                }
            }
            "AudioDefines" => {
                let t = text_of(n);
                if !t.is_empty() {
                    info.audio_defines.push(t);
                }
            }
            _ => {}
        }
    }
    Ok(info)
}

/// Every `*.Civ5Pkg` under `dir`, at any depth, sorted by path (case-insensitive). A file that
/// does not parse is returned in the second list instead of aborting.
pub fn find_packages(dir: &Path) -> Result<(Vec<Package>, Vec<PkgError>), PkgError> {
    let mut files = Vec::new();
    walk(dir, &mut files)?;
    files.sort_by_key(|p| p.to_string_lossy().to_ascii_lowercase());
    let mut ok = Vec::new();
    let mut bad = Vec::new();
    for file in files {
        let bytes = fs::read(&file).map_err(|source| PkgError::Io {
            path: file.clone(),
            source,
        })?;
        let text = String::from_utf8_lossy(&bytes);
        match parse(&text) {
            Ok(info) => {
                let stem = file
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                ok.push(Package {
                    dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
                    file,
                    stem,
                    info,
                });
            }
            Err(msg) => bad.push(PkgError::Parse { path: file, msg }),
        }
    }
    Ok((ok, bad))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), PkgError> {
    let rd = fs::read_dir(dir).map_err(|source| PkgError::Io {
        path: dir.to_path_buf(),
        source,
    })?;
    for e in rd {
        let e = e.map_err(|source| PkgError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        let path = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            walk(&path, out)?;
        } else if ft.is_file()
            && path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("civ5pkg"))
        {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EUI2: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Civ5Package>
  <Priority>250</Priority>
  <Name><Value language="en_US">Enhanced User Interface</Value></Name>
  <UISkin name="Expansion2Primary" set="Expansion2" platform="Common">
    <Skin><Directory>Core</Directory><Directory>CityView</Directory></Skin>
  </UISkin>
</Civ5Package>"#;

    const BNW: &str = r#"<Civ5Package>
  <Priority>200</Priority>
  <UISkin name="P" set="Expansion2" platform="Common">
    <Skin><Directory>UI/Art</Directory><Style>UI/Styles_Expansion2.xml</Style>
      <Color>UI/ColorAtlas_Expansion2.xml</Color>
      <Paks><File>Expansion2UITextures.fpk</File></Paks></Skin>
    <GameplaySkin><Directory>UI/InGame</Directory></GameplaySkin>
  </UISkin>
  <UISkin name="P" set="Expansion2" platform="Tablet">
    <Skin><File>UI_Tablet/Styles_Expansion2_Tablet.xml</File><Paks><File/></Paks></Skin>
    <GameplaySkin><Directory>UI_Tablet/InGame</Directory></GameplaySkin>
  </UISkin>
  <Common><Paks><File>Expansion2Models.fpk</File></Paks></Common>
  <Gameplay><TextData>A.xml</TextData><AudioScripts2D>S.xml</AudioScripts2D></Gameplay>
</Civ5Package>"#;

    #[test]
    fn priority_with_leading_zero_and_name() {
        let p = parse(&EUI2.replace("250", "050")).unwrap();
        assert_eq!(p.priority, Some(50));
        assert_eq!(p.name, "Enhanced User Interface");
        assert_eq!(p.skins.len(), 1);
        assert_eq!(p.skins[0].sections[0].directories, ["Core", "CityView"]);
    }

    #[test]
    fn missing_priority_is_none_and_no_skin_is_always_active() {
        let p = parse("<Civ5Package><Paks><File>a.fpk</File></Paks></Civ5Package>").unwrap();
        assert_eq!(p.priority, None);
        assert!(p.is_active(&Activation::up_to("basegame").unwrap()));
        assert_eq!(p.paks, ["a.fpk"]);
    }

    #[test]
    fn sets_gate_a_package() {
        let p = parse(EUI2).unwrap();
        assert!(p.is_active(&Activation::up_to("expansion2").unwrap()));
        assert!(!p.is_active(&Activation::up_to("expansion1").unwrap()));
        assert!(Activation::up_to("nonsense").is_none());
    }

    #[test]
    fn tablet_directories_leave_the_folder_and_empty_paks_are_dropped() {
        let p = parse(BNW).unwrap();
        let act = Activation::up_to("expansion2").unwrap();
        let ex = p.excluded_paths(&act);
        assert!(ex.contains("ui_tablet/ingame"));
        assert!(ex.contains("ui_tablet/styles_expansion2_tablet.xml"));
        assert!(!ex.contains("ui/art"));
        assert_eq!(
            p.active_paks(&act),
            ["Expansion2UITextures.fpk", "Expansion2Models.fpk"]
        );
        let (styles, colors) = p.active_style_files(&act);
        assert_eq!(styles, ["UI/Styles_Expansion2.xml"]);
        assert_eq!(colors, ["UI/ColorAtlas_Expansion2.xml"]);
        assert_eq!(p.text_data, ["A.xml"]);
        assert_eq!(p.audio_scripts_2d, ["S.xml"]);
    }

    #[test]
    fn a_directory_an_active_skin_names_too_stays() {
        let t = r#"<Civ5Package>
          <UISkin name="a" set="Expansion2" platform="Common"><Skin><Directory>X</Directory></Skin></UISkin>
          <UISkin name="b" set="Expansion2" platform="Tablet"><Skin><Directory>x</Directory><Directory>Y</Directory></Skin></UISkin>
        </Civ5Package>"#;
        let p = parse(t).unwrap();
        let ex = p.excluded_paths(&Activation::up_to("expansion2").unwrap());
        assert_eq!(ex.into_iter().collect::<Vec<_>>(), ["y"]);
    }

    #[test]
    fn bad_xml_and_bad_priority_are_errors() {
        assert!(parse("<Civ5Package>").is_err());
        assert!(parse("<Civ5Package><Priority>x</Priority></Civ5Package>").is_err());
        assert!(parse("<Other/>").is_err());
    }
}
