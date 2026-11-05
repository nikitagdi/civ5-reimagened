//! Tools for running the original files from a game install on disk (native only): a file
//! source over the folders, an index built from them, and the M1 run.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use asset_format::{UiFileInfo, UiFileKind, UiIndex, UiLayerInfo};
use assets::UiFiles;

use crate::config::UiConfig;
use crate::core::UiCore;
pub use crate::fixture::M1_ROOT_XML;
use crate::host::{FileSource, StubHost};
use crate::text::StubText;

/// The UI folders of the install in override order, lowest first: base game, the expansions,
/// Lekmod, then the three EUI packs.
pub const UI_LAYERS: [&str; 6] = [
    "UI",
    "Gameplay/Lua",
    "DLC/Expansion/UI",
    "DLC/Expansion2/UI",
    "DLC/LEKMOD_v34.15/Lua/UI",
    "DLC/UI_bc1",
];

/// Files by the path the index gives (an absolute path), plus a few files in memory.
#[derive(Default)]
pub struct DirFiles {
    pub memory: std::collections::HashMap<String, Vec<u8>>,
}

impl FileSource for DirFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(b) = self.memory.get(path) {
            return Some(b.clone());
        }
        std::fs::read(path).ok()
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// An index of the files in the layers (a later layer hides an earlier file of the same bare
/// name) with the skin style and colour files of the install.
pub fn build_index(assets: &Path, layers: &[&str], extra: &[(String, UiFileKind)]) -> UiIndex {
    let mut by_name: std::collections::BTreeMap<String, UiFileInfo> = Default::default();
    let mut layer_infos = Vec::new();
    for (li, layer) in layers.iter().enumerate() {
        layer_infos.push(UiLayerInfo {
            name: (*layer).to_owned(),
            dir: (*layer).to_owned(),
            priority: li as i32,
            order: li as u32,
            packages: vec![],
            priority_defaulted: false,
        });
        let mut files = Vec::new();
        walk(&assets.join(layer), &mut files);
        for f in files {
            let name = crate::bare_lower(&f.to_string_lossy());
            let kind = match f
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("xml") => UiFileKind::Xml,
                Some("lua") => UiFileKind::Lua,
                Some("ggxml") => UiFileKind::Font,
                Some("dds") => UiFileKind::Texture,
                _ => UiFileKind::Other,
            };
            let size = std::fs::metadata(&f).map(|m| m.len()).unwrap_or(0);
            by_name.insert(
                name.clone(),
                UiFileInfo {
                    name: name.clone(),
                    id: format!("ui:f:{name}"),
                    kind,
                    layer: li as u32,
                    size,
                    path: f.to_string_lossy().into_owned(),
                    hidden: 0,
                    index_of: None,
                },
            );
        }
    }
    for (path, kind) in extra {
        let name = crate::bare_lower(path);
        by_name.insert(
            name.clone(),
            UiFileInfo {
                name: name.clone(),
                id: format!("ui:f:{name}"),
                kind: *kind,
                layer: 0,
                size: 0,
                path: path.clone(),
                hidden: 0,
                index_of: None,
            },
        );
    }
    UiIndex {
        layers: layer_infos,
        files: by_name.into_values().collect(),
        skin_styles: vec![
            "styles_expansion1.xml".into(),
            "styles_expansion2.xml".into(),
        ],
        skin_colors: vec!["coloratlas_expansion2.xml".into()],
        ..UiIndex::default()
    }
}

/// What the M1 run found.
#[derive(Debug, Default)]
pub struct M1Report {
    pub lua_errors: u64,
    pub errors: Vec<String>,
    pub log: Vec<String>,
    pub controls: usize,
    pub xml_warnings: usize,
    pub warnings: Vec<String>,
    pub draw_cmds: usize,
    pub draw_vertices: usize,
    pub tree: String,
    pub zero_sized: Vec<String>,
}

fn open_core(assets: &Path, stubs: &str, memory: &[(&str, &[u8], UiFileKind)]) -> UiCore {
    let mut src = DirFiles::default();
    let mut extra = Vec::new();
    for (name, bytes, kind) in memory {
        src.memory.insert((*name).to_owned(), bytes.to_vec());
        extra.push(((*name).to_owned(), *kind));
    }
    let index = build_index(assets, &UI_LAYERS, &extra);
    let files = UiFiles::open(&index.encode()).expect("index");
    let stubs = stubs.to_owned();
    let mut host = StubHost::with_default_size(256, 256);
    host.add_texture("Assets/UI/Art/Controls/9 Grids/SelectedUp.dds", 32, 32);
    UiCore::new(
        UiConfig::default(),
        files,
        Rc::new(src),
        Box::new(StubText::default()),
        Box::new(host),
        move |lua, g| {
            lua.load(&stubs)
                .set_name("=m1_stubs")
                .set_environment(g.clone())
                .exec()
        },
    )
    .expect("ui core")
}

/// What building every XML of the install found.
#[derive(Debug, Default)]
pub struct BuildAllReport {
    pub files: usize,
    pub controls: usize,
    pub warnings: Vec<String>,
}

/// Loads every XML file of the layers as a root context (the Lua beside it runs on `stubs`)
/// and gathers the XML warnings.
pub fn build_all_xml(assets: &Path, stubs: &str) -> BuildAllReport {
    let ui = open_core(assets, stubs, &[]);
    let mut names = std::collections::BTreeSet::new();
    for layer in UI_LAYERS {
        let mut files = Vec::new();
        walk(&assets.join(layer), &mut files);
        for f in files {
            if f.extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
                == Some("xml")
            {
                names.insert(crate::bare_lower(&f.to_string_lossy()));
            }
        }
    }
    let mut files = 0;
    for n in &names {
        // The style and colour books are not contexts.
        if n.starts_with("styles") || n.starts_with("coloratlas") {
            continue;
        }
        let stem = n.strip_suffix(".xml").unwrap_or(n);
        ui.load_root(stem);
        files += 1;
    }
    let sh = ui.shared();
    let st = sh.st.borrow();
    BuildAllReport {
        files,
        controls: ui.stats().controls,
        warnings: st
            .warnings
            .iter()
            .map(|w| format!("{}:{}: <{}> {}", w.file, w.line, w.tag, w.what))
            .collect(),
    }
}

/// Runs M1: `stubs` is the Lua of the fixture (the parts of the exe API and the game core that
/// the panels call). Builds the contexts, fires the start events and draws one frame.
pub fn run_m1(assets: &Path, stubs: &str, want_tree: bool) -> M1Report {
    let mut ui = open_core(
        assets,
        stubs,
        &[
            (
                "fixture/M1Root.xml",
                M1_ROOT_XML.as_bytes(),
                UiFileKind::Xml,
            ),
            ("fixture/M1Root.lua", b"", UiFileKind::Lua),
        ],
    );
    ui.load_root("M1Root");
    // The panels wait for this event.
    let driver = ui
        .runtime()
        .create_context("m1_driver", |_, _| Ok(()))
        .expect("driver context");
    let _ = driver.exec("__fire_event('SequenceGameInitComplete')", "=m1_driver");
    ui.update(0.016);
    ui.update(0.016);
    let mut list = draw_list::UiDrawList::new();
    ui.draw(&mut list);
    let stats = ui.stats();
    let sh = ui.shared();
    let st = sh.st.borrow();
    let zero_sized = crate::debug::zero_sized_visible(&st)
        .iter()
        .map(|&i| format!("{} {}", st.node(i).kind.name(), st.node(i).id))
        .collect();
    let warnings = st
        .warnings
        .iter()
        .map(|w| format!("{}:{}: <{}> {}", w.file, w.line, w.tag, w.what))
        .collect();
    M1Report {
        lua_errors: stats.lua_errors,
        errors: ui
            .runtime()
            .recent_errors()
            .iter()
            .map(|e| format!("[{}] {}", e.context, e.error.message))
            .collect(),
        log: ui.runtime().take_log(),
        controls: stats.controls,
        xml_warnings: st.warnings.len(),
        warnings,
        draw_cmds: list.commands().len(),
        draw_vertices: list.vertices().len(),
        tree: if want_tree {
            crate::debug::dump(&st)
        } else {
            String::new()
        },
        zero_sized,
    }
}

/// The end-game replay tab as the end-game menu holds it: a deferred, hidden `EndGameReplay` context on `ReplayViewer`, shown
/// by the event `ShowReplayTab` (`stubs` run first, as in [`run_m1`]). Returns what the player would have: the Lua errors, the log.
pub fn show_replay_tab(assets: &Path, stubs: &str) -> M1Report {
    let xml = br#"<Context ColorSet="Beige_Black" Font="TwCenMT20" FontStyle="Shadow">
<LuaContext FileName="Assets/UI/InGame/Popups/ReplayViewer" ID="EndGameReplay" DeferLoad="1" Hidden="True" />
</Context>"#;
    let lua = b"Events.ShowReplayTab.Add(function() Controls.EndGameReplay:DoDeferredLoad(); Controls.EndGameReplay:SetHide(false) end)";
    let mut ui = open_core(
        assets,
        stubs,
        &[
            ("fixture/ReplayRoot.xml", xml, UiFileKind::Xml),
            ("fixture/ReplayRoot.lua", lua, UiFileKind::Lua),
        ],
    );
    ui.load_root("ReplayRoot");
    let driver = ui
        .runtime()
        .create_context("replay_driver", |_, _| Ok(()))
        .expect("driver context");
    let _ = driver.exec("__fire_event('ShowReplayTab')", "=replay_driver");
    ui.update(0.016);
    ui.update(0.016);
    let mut list = draw_list::UiDrawList::new();
    ui.draw(&mut list);
    let stats = ui.stats();
    let sh = ui.shared();
    let st = sh.st.borrow();
    M1Report {
        lua_errors: stats.lua_errors,
        errors: ui
            .runtime()
            .recent_errors()
            .iter()
            .map(|e| format!("[{}] {}", e.context, e.error.message))
            .collect(),
        log: ui.runtime().take_log(),
        controls: stats.controls,
        xml_warnings: st.warnings.len(),
        draw_cmds: list.commands().len(),
        draw_vertices: list.vertices().len(),
        ..M1Report::default()
    }
}
