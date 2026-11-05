//! A UI with Lua over files in memory: the index, the file source, the stub services.
#![allow(dead_code)]

use std::rc::Rc;

use asset_format::{UiFileInfo, UiFileKind, UiIndex, UiLayerInfo};
use assets::UiFiles;
use ui::{MemoryFiles, StubHost, StubText, UiConfig, UiCore};

pub struct Fixture {
    pub ui: UiCore,
    pub sounds: std::rc::Rc<std::cell::RefCell<Vec<(audio::ButtonEvent, audio::ButtonFlags)>>>,
}

pub fn files_index(files: &[(&str, &str)]) -> (UiFiles, Rc<MemoryFiles>) {
    let mut mem = MemoryFiles::default();
    let mut infos = Vec::new();
    for (path, body) in files {
        let name = ui::bare_lower(path);
        let kind = if name.ends_with(".xml") {
            UiFileKind::Xml
        } else if name.ends_with(".lua") {
            UiFileKind::Lua
        } else {
            UiFileKind::Other
        };
        infos.push(UiFileInfo {
            name: name.clone(),
            id: format!("ui:f:{name}"),
            kind,
            layer: 0,
            size: body.len() as u64,
            path: (*path).to_owned(),
            hidden: 0,
            index_of: None,
        });
        mem.files
            .insert((*path).to_owned(), body.as_bytes().to_vec());
    }
    let index = UiIndex {
        layers: vec![UiLayerInfo {
            name: "base".into(),
            dir: String::new(),
            priority: 0,
            order: 0,
            packages: vec![],
            priority_defaulted: false,
        }],
        files: infos,
        ..UiIndex::default()
    };
    (UiFiles::open(&index.encode()).expect("index"), Rc::new(mem))
}

pub fn fixture(files: &[(&str, &str)]) -> Fixture {
    fixture_with(files, |_, _| Ok(()))
}

pub fn fixture_with(
    files: &[(&str, &str)],
    setup: impl Fn(&lua_sys::Lua, &lua_sys::Table<'_>) -> lua_sys::Result<()> + 'static,
) -> Fixture {
    let (ui_files, src) = files_index(files);
    let host = StubHost::with_default_size(64, 64);
    let sounds = host.sounds.clone();
    let ui = UiCore::new(
        UiConfig::default(),
        ui_files,
        src,
        Box::new(StubText::default()),
        Box::new(host),
        setup,
    )
    .expect("ui core");
    Fixture { ui, sounds }
}

impl Fixture {
    pub fn errors(&self) -> Vec<String> {
        self.ui
            .runtime()
            .recent_errors()
            .iter()
            .map(|e| format!("{}: {}", e.context, e.error.message))
            .collect()
    }

    pub fn log(&self) -> Vec<String> {
        self.ui.runtime().take_log()
    }

    pub fn find(&self, id: &str) -> ui::ControlId {
        let st = self.ui.shared().st.borrow();
        st.tree
            .ids()
            .find(|&i| st.node(i).id == id)
            .unwrap_or_else(|| panic!("no control {id}"))
    }
}

/// Stand-ins for the enums `exe-api` provides.
pub fn enums(lua: &lua_sys::Lua, g: &lua_sys::Table<'_>) -> lua_sys::Result<()> {
    lua.load(
        "Mouse = {eLClick=0, eLDblClick=1, eRClick=2, eMClick=3, eMouseEnter=5, eMouseExit=6}\n\
         MouseEvents = {LButtonDown=513, LButtonUp=514, MouseMove=512, RButtonDown=516, MouseWheel=522}\n\
         KeyEvents = {KeyDown=256, KeyUp=257}\n\
         Keys = {VK_ESCAPE=27, VK_RETURN=13, VK_TAB=9}",
    )
    .set_name("=enums")
    .set_environment(g.clone())
    .exec()
}

use platform::{
    InputEvent, Modifiers, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind,
    PointerPhase,
};

pub fn ptr(phase: PointerPhase, x: f64, y: f64, button: Option<MouseButton>) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        id: PointerId::MOUSE,
        kind: PointerKind::Mouse,
        phase,
        position: PhysicalPosition { x, y },
        button,
        modifiers: Modifiers::default(),
    })
}

impl Fixture {
    pub fn mv(&mut self, x: f64, y: f64) -> ui::InputResult {
        self.ui.handle_input(&ptr(PointerPhase::Move, x, y, None))
    }

    pub fn click(&mut self, x: f64, y: f64) -> (ui::InputResult, ui::InputResult) {
        let d = self
            .ui
            .handle_input(&ptr(PointerPhase::Down, x, y, Some(MouseButton::Left)));
        let u = self
            .ui
            .handle_input(&ptr(PointerPhase::Up, x, y, Some(MouseButton::Left)));
        (d, u)
    }
}
