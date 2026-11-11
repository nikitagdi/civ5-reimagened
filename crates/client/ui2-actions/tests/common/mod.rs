//! A rig for the route tests: a model store, the actions module and a recording sink, with builders for the
//! roots the routes read.
#![allow(dead_code)]

use std::sync::Arc;

use protocol::Command;
use ui2_actions::{MemorySettings, RecordingSink, Sent, UiActions};
use ui2_model::generated::{
    CityBanner, Game, Map, Meta, Phase, Plot, Toast, Turn, Ui, Unit, UnitActionRow, UnitFlag,
};
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{Key, KeyedList, ModelStore, Op, Owner, Root, Schema, Value};

pub fn intent(name: &str, args: &[(&str, Value)]) -> WireIntent {
    WireIntent {
        name: Arc::from(name),
        args: args
            .iter()
            .map(|(k, v)| (Arc::from(*k), v.clone()))
            .collect(),
        origin: Origin::Markup { node: 3 },
    }
}

pub fn s(text: &str) -> Value {
    Value::Str(Arc::from(text))
}

pub fn n(v: i32) -> Value {
    Value::I32(v)
}

pub fn plot(x: i32, y: i32) -> Value {
    Value::Struct(vec![Value::I32(x), Value::I32(y)])
}

/// The key of a unit or city of the player's seat 0.
pub fn key(id: u32) -> Key {
    Key::new(0, id)
}

pub fn kstr(id: u32) -> Value {
    s(&key(id).to_js())
}

pub struct Rig {
    pub store: ModelStore,
    pub actions: UiActions,
    pub sink: RecordingSink,
    pub now: u64,
}

impl Rig {
    /// A rig with a live game at turn 42, humans moving, connected.
    pub fn new() -> Rig {
        let schema = Schema::v1();
        let mut store = ModelStore::new(schema);
        let mut actions = UiActions::new(schema, Box::new(MemorySettings::default()));
        actions.publish(&mut store);
        let mut rig = Rig {
            store,
            actions,
            sink: RecordingSink::new(),
            now: 1000,
        };
        rig.put(Meta {
            connected: true,
            live: true,
            ..Meta::default()
        });
        rig.put(Game {
            turn: 42,
            phase: Phase::Humans,
            ..Game::default()
        });
        rig.store.take_frame();
        rig
    }

    pub fn put<R: Root>(&mut self, root: R) {
        self.store.writer(Owner::Source).put(root).unwrap();
    }

    /// Run the intents; the delta of the frame, if any.
    pub fn send(&mut self, intents: Vec<WireIntent>) -> Option<Vec<Op>> {
        let mut intents = intents;
        self.actions
            .apply(&mut intents, &mut self.store, &mut self.sink, self.now);
        assert!(intents.is_empty(), "apply drains the intents");
        self.store.take_frame().map(|f| f.ops)
    }

    pub fn one(&mut self, name: &str, args: &[(&str, Value)]) -> Option<Vec<Op>> {
        self.send(vec![intent(name, args)])
    }

    pub fn ui(&self) -> &Ui {
        self.store.get::<Ui>().unwrap()
    }

    pub fn toasts(&self) -> Vec<Toast> {
        self.ui().errors.iter().cloned().collect()
    }

    /// The reasons of the toasts shown, by name.
    pub fn why(&self) -> Vec<&'static str> {
        self.ui().errors.iter().map(|t| t.why.name()).collect()
    }

    pub fn commands(&self) -> Vec<Command> {
        self.sink.commands()
    }

    pub fn sent(&self) -> &[Sent] {
        &self.sink.sent
    }

    /// Nothing left the module and exactly one toast says why.
    pub fn assert_refused(&self, why: &str) {
        assert!(
            self.sink.sent.is_empty(),
            "nothing may be sent, got {:?}",
            self.sink.sent
        );
        assert_eq!(self.why(), vec![why]);
    }

    pub fn units(&mut self, units: Vec<UnitFlag>) {
        let mut map = self.store.get::<Map>().cloned().unwrap_or_default();
        map.units = KeyedList::from_vec(units).unwrap();
        self.put(map);
    }

    pub fn cities(&mut self, cities: Vec<CityBanner>) {
        let mut map = self.store.get::<Map>().cloned().unwrap_or_default();
        map.cities = KeyedList::from_vec(cities).unwrap();
        self.put(map);
    }

    pub fn turn(&mut self, turn: Turn) {
        self.put(turn);
    }
}

pub fn unit_at(id: u32, x: i16, y: i16, mine: bool, waiting: bool) -> UnitFlag {
    UnitFlag {
        key: key(id),
        mine,
        waiting,
        plot: Plot { x, y },
        ..UnitFlag::default()
    }
}

pub fn city_at(id: u32, x: i16, y: i16, mine: bool) -> CityBanner {
    CityBanner {
        key: key(id),
        mine,
        plot: Plot { x, y },
        ..CityBanner::default()
    }
}

/// The `unit` root of the selected unit with the given action rows `(kind, enabled, supported)`.
pub fn selected(
    id: u32,
    rows: &[(ui2_model::generated::UnitActionKind, bool, bool)],
) -> Option<Unit> {
    Some(Unit {
        key: key(id),
        actions: KeyedList::from_vec(
            rows.iter()
                .map(|(kind, enabled, supported)| UnitActionRow {
                    kind: *kind,
                    enabled: *enabled,
                    reason: None,
                    supported: *supported,
                })
                .collect(),
        )
        .unwrap(),
        ..Unit::default()
    })
}
