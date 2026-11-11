//! The mods in UI v2, the application's side: the store of the mods' UI fields ([`ModUiLink`]) and the running UI
//! v2 meet here once per frame.
//!
//! Before a UI v2 frame its writes come over: a [`snapshot`](crate::mod_host::ModUiStore::snapshot) of every mod
//! the first time (and after the UI slept or was rebuilt), then each frame's changes. After the frame the intents
//! of the mods' panels and scripts go back to the store, for the hook of the mods to deliver to their Rust halves
//! in its next frame.

use std::sync::Arc;

use mod_client::UiValue;
use ui2_host::{ModValue, ModWrite, ModelValue, V2Ui};

use crate::mod_host::{ModUiLink, ModUiOp};

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

/// What UI v2 keeps of the mods' store.
#[derive(Default)]
pub(crate) struct V2Mods {
    /// The store the app shares with the hook of the mods; `None`: no mods in this client.
    link: Option<ModUiLink>,
}

impl V2Mods {
    /// The store the app shares with the hook of the mods.
    pub(crate) fn set_link(&mut self, link: ModUiLink) {
        self.link = Some(link);
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
        let mut m = V2Mods::default();
        m.set_link(Rc::clone(&link));
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
        let mut m = V2Mods::default();
        m.set_link(Rc::clone(&link));
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
}
