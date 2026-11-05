//! Helpers of the tests: a host over a shared store, a bare Lua state with the globals of `ExeApi`, and a
//! `ui` core on files in memory.
#![allow(dead_code)]

use std::rc::Rc;

use asset_format::{UiFileInfo, UiFileKind, UiIndex, UiLayerInfo};
use assets::UiFiles;
use exe_api::{ExeApi, ExeConfig, ExeHost, SeatInfo};
use lua_sys::{Lua, LuaValue};
use platform::{KeyValueStore, MemoryStore};
use ui::{MemoryFiles, StubHost, StubText, UiConfig, UiCore};

pub struct TestHost {
    pub store: MemoryStore,
    pub seat: SeatInfo,
}

impl TestHost {
    pub fn new(store: &MemoryStore) -> Rc<TestHost> {
        Rc::new(TestHost {
            store: store.clone(),
            seat: SeatInfo {
                nick: "Alice".to_owned(),
                local_id: 1,
                host_id: 0,
                pings_ms: vec![(0, 40)],
            },
        })
    }
}

impl ExeHost for TestHost {
    fn kv(&self) -> &dyn KeyValueStore {
        &self.store
    }

    fn look_at(&self, _x: i32, _y: i32) {}

    fn seat_info(&self) -> SeatInfo {
        self.seat.clone()
    }
}

pub fn api(store: &MemoryStore) -> ExeApi {
    ExeApi::new(ExeConfig::default(), TestHost::new(store))
}

/// A Lua state with the globals of `api`.
pub fn lua_with(api: &ExeApi) -> Lua {
    let lua = Lua::new().expect("lua");
    {
        let g = lua.globals();
        api.base_setup()(&lua, &g).expect("base setup");
    }
    lua
}

/// Runs `code` (which returns one value) and gives it as a number.
pub fn eval_num(lua: &Lua, code: &str) -> f64 {
    match eval(lua, code) {
        LuaValue::Number(n) => n,
        other => panic!("{code}: not a number: {}", other.type_name()),
    }
}

pub fn eval_bool(lua: &Lua, code: &str) -> bool {
    match eval(lua, code) {
        LuaValue::Boolean(b) => b,
        other => panic!("{code}: not a boolean: {}", other.type_name()),
    }
}

pub fn eval_str(lua: &Lua, code: &str) -> String {
    match eval(lua, code) {
        LuaValue::String(s) => s.to_string_lossy().into_owned(),
        other => panic!("{code}: not a string: {}", other.type_name()),
    }
}

pub fn eval<'l>(lua: &'l Lua, code: &str) -> LuaValue<'l> {
    lua.load(&format!("return ({code})"))
        .set_name("=test")
        .eval::<LuaValue>()
        .unwrap_or_else(|e| panic!("{code}: {e}"))
}

pub fn run(lua: &Lua, code: &str) {
    lua.load(code)
        .set_name("=test")
        .exec()
        .unwrap_or_else(|e| panic!("{code}: {e}"));
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

/// A UI core on `files` whose Lua has the globals of `api`.
pub fn ui_with(api: &ExeApi, files: &[(&str, &str)]) -> UiCore {
    let (ui_files, src) = files_index(files);
    UiCore::new(
        UiConfig::default(),
        ui_files,
        src,
        Box::new(StubText::default()),
        Box::new(StubHost::with_default_size(64, 64)),
        api.base_setup(),
    )
    .expect("ui core")
}
