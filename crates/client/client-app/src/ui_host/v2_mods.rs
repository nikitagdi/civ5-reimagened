//! The mods in UI v2, the application's side: the store of the mods' UI fields ([`ModUiLink`]) and the running UI
//! v2 meet here once per frame.
//!
//! Before a UI v2 frame its writes come over: a [`snapshot`](crate::mod_host::ModUiStore::snapshot) of every mod
//! the first time (and after the UI slept or was rebuilt), then each frame's changes. After the frame the intents
//! of the mods' panels and scripts go back to the store, for the hook of the mods to deliver to their Rust halves
//! in its next frame.

use std::rc::Rc;
use std::sync::Arc;

use mod_client::UiValue;
use platform::{HttpClient, HttpError, HttpOp, HttpResponse};
use ui2_host::mods::{MODS_INDEX_PATH, ModIndexEntry, ModUi, parse_mods_index};
use ui2_host::{ModValue, ModWrite, ModelValue, V2Ui};

use super::v2::ScriptHostFactory;
use crate::mod_host::{ModUiLink, ModUiOp};

/// The body of a successful answer, or why there is none.
fn body(url: &str, answer: Result<HttpResponse, HttpError>) -> Result<Vec<u8>, String> {
    match answer {
        Ok(r) if r.is_success() => Ok(r.body),
        Ok(r) => Err(format!("{url}: HTTP {}", r.status)),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

/// A mod's value in UI v2's terms.
pub(crate) fn mod_value(v: &UiValue) -> ModValue {
    match v {
        UiValue::Bool(b) => ModValue::Bool(*b),
        UiValue::Int(n) => ModValue::Int(*n),
        UiValue::Num(x) => ModValue::Num(*x),
        UiValue::Str(s) => ModValue::Str(Arc::from(s.as_str())),
        _ => ModValue::Null,
    }
}

/// A model value as a mod sees it in the arguments of an intent (lists and structs are not passed: null).
pub(crate) fn ui_value(v: &ModelValue) -> UiValue {
    match v {
        ModelValue::Bool(b) => UiValue::Bool(*b),
        ModelValue::I32(n) => UiValue::Int(i64::from(*n)),
        ModelValue::U32(n) => UiValue::Int(i64::from(*n)),
        ModelValue::F32(x) => UiValue::Num(f64::from(*x)),
        ModelValue::Enum(n) => UiValue::Int(i64::from(*n)),
        ModelValue::Key(k) => UiValue::Str(k.to_string()),
        ModelValue::Str(s) => UiValue::Str(s.to_string()),
        ModelValue::Text(ui2_host::ModelText::Raw(s)) => UiValue::Str(s.to_string()),
        _ => UiValue::Null,
    }
}

/// A store write as a UI v2 write.
pub(crate) fn mod_write(op: &ModUiOp) -> ModWrite {
    let rec =
        |rec: &[(String, UiValue)]| rec.iter().map(|(n, v)| (n.clone(), mod_value(v))).collect();
    match op {
        ModUiOp::Set { field, value } => ModWrite::Set {
            field: field.clone(),
            value: mod_value(value),
        },
        ModUiOp::Put { list, key, rec: r } => ModWrite::Put {
            list: list.clone(),
            key: *key,
            rec: rec(r),
        },
        ModUiOp::Remove { list, key } => ModWrite::Remove {
            list: list.clone(),
            key: *key,
        },
        ModUiOp::Clear { list } => ModWrite::Clear { list: list.clone() },
    }
}

/// Where the loading of the mods' fragments is.
#[derive(Default)]
enum Load {
    /// Nothing to load (no mod set, or no mod of it has a fragment).
    #[default]
    Idle,
    /// Waiting for `ui2/mods-ui.json`.
    Index(Box<dyn HttpOp>),
    /// Waiting for the packages of the set's mods: `(position in the set, entry, request)`.
    Packages(Vec<(usize, ModIndexEntry, Box<dyn HttpOp>)>),
}

/// What UI v2 keeps of the mods: the store of their fields, and their fragments (fetched with the client build:
/// `ui2/mods-ui.json`, then `ui2/<file>` of each mod of the game's set, in load order).
#[derive(Default)]
pub(crate) struct V2Mods {
    /// The store the app shares with the hook of the mods; `None`: no mods in this client.
    link: Option<ModUiLink>,
    http: Option<Rc<dyn HttpClient>>,
    /// The workers of the mods' scripts (a host of their own per UI v2).
    scripts: Option<ScriptHostFactory>,
    /// The game's mods with a client side: `(id, version)` in load order.
    set: Vec<(Arc<str>, String)>,
    /// The index, once read (`Err`: it could not be; the mods have no panels in this session).
    index: Option<Result<Vec<ModIndexEntry>, ()>>,
    load: Load,
    /// The packages that arrived: position in the set and the mod's UI.
    got: Vec<(usize, ModUi)>,
    /// The mods' UI is loaded and waits to be mounted (`Some`), and stays for a UI v2 built anew.
    ready: Option<Vec<ModUi>>,
    /// The UI v2 frame count the panels were mounted at (`None`: not mounted in the running UI).
    mounted: bool,
}

impl V2Mods {
    /// The store the app shares with the hook of the mods, the HTTP client the fragments come with, and the script
    /// workers of the mods.
    pub(crate) fn set_link(
        &mut self,
        link: ModUiLink,
        http: Rc<dyn HttpClient>,
        scripts: Option<ScriptHostFactory>,
    ) {
        self.link = Some(link);
        self.http = Some(http);
        self.scripts = scripts;
    }

    /// The game's mod set: its fragments are loaded and mounted once a UI v2 runs.
    pub(crate) fn set_mods(&mut self, set: &protocol::ModSetView) {
        let mut mods: Vec<_> = set.mods.iter().filter(|m| m.available).collect();
        mods.sort_by_key(|m| m.load_idx);
        let set: Vec<(Arc<str>, String)> = mods
            .into_iter()
            .map(|m| (Arc::from(m.id.as_str()), m.version.clone()))
            .collect();
        if set == self.set {
            return;
        }
        self.set = set;
        self.load = Load::Idle;
        self.got.clear();
        self.ready = None;
        // The panels of the old set go at the next frame.
        self.mounted = false;
    }

    /// Advances the loading; mounts the mods' UI in `ui` once every package of the set settled (and again in a UI
    /// v2 built anew).
    fn load(&mut self, ui: &mut V2Ui) {
        let Some(http) = self.http.clone() else {
            return;
        };
        if ui.frames() == 0 {
            self.mounted = false;
        }
        if self.set.is_empty() && self.ready.is_none() {
            if !self.mounted {
                self.mounted = true;
                ui.mount_mods(Vec::new());
            }
            return;
        }
        self.load = match std::mem::take(&mut self.load) {
            Load::Idle if self.ready.is_none() => match &self.index {
                None => {
                    tracing::info!(url = MODS_INDEX_PATH, "UI v2: fetching the mods' panels");
                    Load::Index(http.get(MODS_INDEX_PATH))
                }
                Some(Ok(index)) => self.fetch(&*http, index.clone()),
                Some(Err(())) => {
                    self.ready = Some(Vec::new());
                    Load::Idle
                }
            },
            Load::Index(mut op) => match op.poll() {
                None => Load::Index(op),
                Some(answer) => {
                    let index = body(MODS_INDEX_PATH, answer)
                        .and_then(|b| String::from_utf8(b).map_err(|_| "not UTF-8".to_string()))
                        .and_then(|t| parse_mods_index(&t));
                    match index {
                        Ok(index) => {
                            self.index = Some(Ok(index.clone()));
                            self.fetch(&*http, index)
                        }
                        Err(why) => {
                            tracing::warn!("UI v2: the mods have no panels in this session: {why}");
                            self.index = Some(Err(()));
                            self.ready = Some(Vec::new());
                            Load::Idle
                        }
                    }
                }
            },
            Load::Packages(ops) => {
                let mut left = Vec::new();
                for (pos, e, mut op) in ops {
                    match op.poll() {
                        None => left.push((pos, e, op)),
                        Some(answer) => {
                            match body(&e.path(), answer).and_then(|b| e.open(Arc::from(b))) {
                                Ok(m) => self.got.push((pos, m)),
                                Err(why) => {
                                    tracing::warn!("UI v2: mod {} has no panels: {why}", e.id)
                                }
                            }
                        }
                    }
                }
                if left.is_empty() {
                    self.got.sort_by_key(|(pos, _)| *pos);
                    self.ready = Some(self.got.drain(..).map(|(_, m)| m).collect());
                    Load::Idle
                } else {
                    Load::Packages(left)
                }
            }
            other => other,
        };
        if self.mounted {
            return;
        }
        let Some(ready) = self.ready.clone() else {
            return;
        };
        self.mounted = true;
        if let Some(make) = &self.scripts {
            ui.set_mod_script_host(make());
        }
        for m in ui.mount_mods(ready) {
            match &m.refused {
                Some(why) => tracing::warn!("UI v2: mod {}: {why}", m.id),
                None => tracing::info!(
                    "UI v2: mod {}: panels {:?}, left out {:?}",
                    m.id,
                    m.panels,
                    m.skipped
                ),
            }
        }
        // The models are mounted: the mods' writes are checked when made, and the next frame starts from a
        // snapshot (what was written before the fields existed reaches them).
        if let Some(link) = &self.link {
            let mut s = link.borrow_mut();
            let ids: Vec<(u16, Arc<str>)> = s.ids().map(|(i, id)| (i, Arc::clone(id))).collect();
            for (idx, id) in ids {
                if let Some(check) = ui.mod_model_check(&id) {
                    s.set_check(idx, Box::new(move |op| check.check(&mod_write(op))));
                }
            }
            s.set_consumer(false);
        }
    }

    /// Asks for the packages of the set's mods that the index lists with the same version.
    fn fetch(&mut self, http: &dyn HttpClient, index: Vec<ModIndexEntry>) -> Load {
        let mut ops = Vec::new();
        for (pos, (id, version)) in self.set.iter().enumerate() {
            match index.iter().find(|e| e.id == **id) {
                Some(e) if e.version == *version => {
                    let url = e.path();
                    ops.push((pos, e.clone(), http.get(&url)));
                }
                Some(e) => tracing::warn!(
                    "UI v2: mod {id} {version} has no panels: this build has them for {}",
                    e.version
                ),
                // A mod without `[ui]` (or one this build was made without): nothing to say.
                None => {}
            }
        }
        if ops.is_empty() {
            self.ready = Some(Vec::new());
            return Load::Idle;
        }
        Load::Packages(ops)
    }

    /// The UI sleeps or is gone: the store stops recording changes (the next frame starts from a snapshot).
    pub(crate) fn sleep(&mut self) {
        if let Some(l) = &self.link {
            l.borrow_mut().set_consumer(false);
        }
    }
    /// The mods' writes for the frame of `ui`: a snapshot of every mod when the UI starts taking them (its first
    /// frame, or after it slept), else the changes since the last frame.
    pub(crate) fn before_frame(&mut self, ui: &mut V2Ui) {
        self.load(ui);
        let Some(link) = &self.link else {
            return;
        };
        let mut s = link.borrow_mut();
        if ui.frames() == 0 {
            // A new UI v2 (a skin built anew has an empty model): everything again.
            s.set_consumer(false);
        }
        if !s.consumer() {
            s.set_consumer(true);
            let ids: Vec<(u16, Arc<str>)> = s.ids().map(|(i, id)| (i, Arc::clone(id))).collect();
            for (idx, id) in ids {
                let ops = s.snapshot(idx);
                if !ops.is_empty() {
                    ui.queue_mod_writes(&id, ops.iter().map(mod_write));
                }
            }
            return;
        }
        for (idx, op) in s.take_changes() {
            let Some(id) = s
                .ids()
                .find(|(i, _)| *i == idx)
                .map(|(_, id)| Arc::clone(id))
            else {
                continue;
            };
            ui.queue_mod_writes(&id, [mod_write(&op)]);
        }
    }

    /// The intents the frame of `ui` accepted for the mods go to the store, for the hook of the mods.
    pub(crate) fn after_frame(&mut self, ui: &mut V2Ui) {
        let intents = ui.take_mod_intents();
        let Some(link) = &self.link else {
            return;
        };
        if intents.is_empty() {
            return;
        }
        let mut s = link.borrow_mut();
        for i in intents {
            let Some(idx) = s.idx_of(&i.mod_id) else {
                tracing::debug!(mod_id = %i.mod_id, "a UI intent for a mod that is not in the game");
                continue;
            };
            let args = i
                .args
                .iter()
                .map(|(n, v)| (n.to_string(), ui_value(v)))
                .collect();
            s.push_intent(idx, i.name.to_string(), args);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use protocol::{ModSetView, ModSync, ModView};
    use ui2_host::{ModIdx, Origin, V2Frame, V2Init, WireIntent};
    use ui2_package::Layer;
    use ui2_package::style::StyleRecord;
    use ui2_runtime::testkit::{Kit, TNode};

    use super::*;
    use crate::mod_host::{ModUiOp, ModUiStore};

    /// A UI v2 over an empty HUD layer.
    fn ui() -> V2Ui {
        let mut kit = Kit::new();
        let r = kit.record(StyleRecord::initial());
        let set = kit.plain(r);
        kit.template("doc", vec![TNode::layer(Layer::Hud, set)]);
        V2Ui::new(kit.finish().expect("skin"), V2Init::default()).expect("ui")
    }

    fn frame(ui: &mut V2Ui, mods: Option<Vec<Arc<str>>>) {
        ui.frame(V2Frame {
            replica: None,
            steps: &[],
            net: None,
            events: &[],
            mods,
            now_ms: 16.0,
        });
    }

    fn link() -> ModUiLink {
        let mut s = ModUiStore::new();
        let view = |id: &str, idx| ModView {
            id: id.into(),
            version: "1.0.0".into(),
            load_idx: idx,
            sync: ModSync::Cosmetic,
            client_required: false,
            available: true,
            pack: None,
        };
        s.start(&ModSetView {
            modset_hash: 1,
            mods: vec![view("hello", 0), view("other", 1)],
        });
        Rc::new(RefCell::new(s))
    }

    fn set(n: i64) -> ModUiOp {
        ModUiOp::Set {
            field: "count".into(),
            value: UiValue::Int(n),
        }
    }

    fn count(ui: &V2Ui) -> Option<ModelValue> {
        match ui.mod_model_value("hello")? {
            ModelValue::Struct(f) => f.first().cloned(),
            _ => None,
        }
    }

    #[test]
    fn mod_ui_writes_reach_ui_v2_from_a_snapshot_then_as_changes() {
        let link = link();
        // Written while the classic UI was on the screen.
        link.borrow_mut().write(0, set(4)).unwrap();
        let mut ui = ui();
        ui.mount_mod_model("hello", "[root]\ncount = \"i32\"\n")
            .expect("model");
        let mut m = V2Mods {
            link: Some(Rc::clone(&link)),
            ..V2Mods::default()
        };
        m.before_frame(&mut ui);
        frame(&mut ui, None);
        m.after_frame(&mut ui);
        assert!(
            matches!(count(&ui), Some(ModelValue::I32(4))),
            "{:?}",
            count(&ui)
        );
        link.borrow_mut().write(0, set(5)).unwrap();
        m.before_frame(&mut ui);
        frame(&mut ui, None);
        assert!(matches!(count(&ui), Some(ModelValue::I32(5))));
        // Asleep: the next frame starts from a snapshot again.
        m.sleep();
        link.borrow_mut().write(0, set(6)).unwrap();
        assert!(link.borrow_mut().take_changes().is_empty());
        m.before_frame(&mut ui);
        frame(&mut ui, None);
        assert!(matches!(count(&ui), Some(ModelValue::I32(6))));
    }

    #[test]
    fn mod_intents_reach_the_mod_they_name_and_a_foreign_prefix_does_not() {
        let link = link();
        let mut ui = ui();
        let mut m = V2Mods {
            link: Some(Rc::clone(&link)),
            ..V2Mods::default()
        };
        let table = vec![Arc::from("hello"), Arc::from("other")];
        let say = |name: &str, idx| WireIntent {
            name: name.into(),
            args: vec![(Arc::from("0"), ModelValue::I32(7))],
            origin: Origin::Mod(ModIdx(idx)),
        };
        // `hello` says its own; it cannot speak for `other`.
        ui.push_intent(say("mods.hello.say", 0));
        ui.push_intent(say("mods.other.say", 0));
        m.before_frame(&mut ui);
        frame(&mut ui, Some(table));
        m.after_frame(&mut ui);
        let intents = link.borrow_mut().take_intents();
        assert_eq!(
            intents,
            [(
                0,
                "say".to_string(),
                vec![("0".to_string(), UiValue::Int(7))]
            )]
        );
    }

    /// A fragment of mod `hello` with one panel `echo` and the model `count: i32`.
    fn hello_fragment() -> Vec<u8> {
        use ui2_package::{Kind, Node, NodeKind, PackageBuilder, StyleSet, Tag, Template};
        let mut b = PackageBuilder::new();
        let record = b.style_record(StyleRecord::initial());
        let style = b.style_set(StyleSet::plain(vec![record]));
        let part = b.str("echo");
        let name = b.str("doc");
        b.template(Template {
            name,
            nodes: vec![Node {
                kind: NodeKind::Box { tag: Tag::Div },
                subtree: 1,
                style,
                part: Some(part),
                flags: 0,
            }],
        });
        b.script(ui2_host::mods::MODEL_FILE, b"[root]\ncount = \"i32\"\n");
        let mut m = Kit::manifest();
        m.kind = Kind::Fragment {
            slots: vec!["mods".into()],
        };
        m.id = "hello".into();
        b.finish(m).expect("fragment")
    }

    #[test]
    fn mod_fragments_are_fetched_mounted_and_their_models_get_the_writes_and_the_checks() {
        use crate::mod_assets::tests::FakeHttp;
        let bytes = hello_fragment();
        let hash = ui2_host::hash_hex(
            &ui2_package::Package::read(Arc::from(bytes.clone()))
                .expect("readable")
                .hash(),
        );
        let http = Rc::new(FakeHttp::default());
        let ok = |body: Vec<u8>| Ok(platform::HttpResponse { status: 200, body });
        http.routes.borrow_mut().insert(
            MODS_INDEX_PATH.into(),
            ok(format!(
                "[{{\"id\": \"hello\", \"version\": \"1.0.0\", \"file\": \"mods/hello.ui2\", \"hash\": \"{hash}\", \
                 \"api\": \"^1\", \"panels\": [{{\"id\": \"echo\", \"point\": \"hud.right\", \"mode\": \"append\", \
                 \"fallback\": \"mods\"}}]}}]"
            )
            .into_bytes()),
        );
        http.routes
            .borrow_mut()
            .insert("ui2/mods/hello.ui2".into(), ok(bytes));
        let link = link();
        link.borrow_mut().write(0, set(7)).unwrap();
        let mut ui = {
            let mut kit = Kit::new();
            let r = kit.record(StyleRecord::initial());
            let s = kit.plain(r);
            kit.template(
                "doc",
                vec![TNode::layer(Layer::Hud, s).child(TNode::el(s).slot("mods"))],
            );
            V2Ui::new(kit.finish().expect("skin"), V2Init::default()).expect("ui")
        };
        let mut m = V2Mods::default();
        m.set_link(Rc::clone(&link), http.clone(), None);
        m.set_mods(&ModSetView {
            modset_hash: 1,
            mods: vec![ModView {
                id: "hello".into(),
                version: "1.0.0".into(),
                load_idx: 0,
                sync: ModSync::Cosmetic,
                client_required: false,
                available: true,
                pack: None,
            }],
        });
        for _ in 0..4 {
            m.before_frame(&mut ui);
            frame(&mut ui, None);
            m.after_frame(&mut ui);
        }
        let panels = ui.runtime().panels();
        let echo = panels.iter().find(|p| p.owner.as_deref() == Some("hello"));
        // The skin has no `hud.right`: the panel is in `mods`.
        assert_eq!(
            echo.and_then(|p| p.slot.as_deref()),
            Some("mods"),
            "{panels:?}"
        );
        assert!(
            matches!(count(&ui), Some(ModelValue::I32(7))),
            "{:?}",
            count(&ui)
        );
        // The model's check refuses a field it does not have when the mod writes it.
        let e = link
            .borrow_mut()
            .write(
                0,
                ModUiOp::Set {
                    field: "ghost".into(),
                    value: UiValue::Int(1),
                },
            )
            .unwrap_err();
        assert!(format!("{e:?}").contains("ghost"), "{e:?}");
        assert_eq!(
            http.requests.borrow().as_slice(),
            [MODS_INDEX_PATH, "ui2/mods/hello.ui2"]
        );
    }
}
