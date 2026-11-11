//! Reconciling the local state with the model after the source rebuilt it. The local state (selection, open
//! windows, focus) is kept across a new snapshot or a new turn; what the new model no longer has is dropped.

use ui2_model::generated::{Map, Note, Production, Ui};
use ui2_model::{KeyedList, ModelStore};

use crate::UiActions;
use crate::ui::UiState;

impl UiActions {
    /// Drop what the model no longer has: a selected unit that is gone (and the map mode with it), a production
    /// window of a city that is not the player's, a focus or hover on an option that is gone, dismissed notes
    /// that no longer exist. Roots the source has not written yet are not judged. Call it after the source
    /// wrote the frame's roots.
    pub fn reconcile(&mut self, store: &mut ModelStore, now_ms: u64) {
        self.tick(now_ms);
        self.state.expire(now_ms);
        reconcile_state(&mut self.state, store);
        self.publish(store);
    }
}

fn reconcile_state(state: &mut UiState, store: &ModelStore) {
    let mut ui = state.ui().clone();
    let before = ui.clone();
    let production = store.get::<Option<Production>>();
    drop_gone(
        &mut ui,
        store.get::<Map>(),
        production,
        store.get::<KeyedList<Note>>(),
        state,
    );
    if ui != before {
        *state.edit() = ui;
    }
}

fn drop_gone(
    ui: &mut Ui,
    map: Option<&Map>,
    production: Option<&Option<Production>>,
    notes: Option<&KeyedList<Note>>,
    state: &mut UiState,
) {
    if let Some(map) = map {
        if let Some(unit) = ui.selection.unit
            && map.units.get(unit).is_none()
        {
            ui.selection.unit = None;
            ui.map_mode = ui2_model::generated::MapMode::Select;
        }
        if let Some(city) = ui.windows.production.city
            && !map.cities.get(city).is_some_and(|c| c.mine)
        {
            ui.windows.production.open = false;
            ui.windows.production.city = None;
        }
    }
    if let Some(production) = production {
        let has = |item| {
            production.as_ref().is_some_and(|p| {
                p.groups
                    .iter()
                    .any(|g| g.options.iter().any(|o| o.item == item))
            })
        };
        if let Some(k) = ui.production.focus
            && !has(k)
        {
            ui.production.focus = None;
        }
        if let Some(k) = ui.production.hover
            && !has(k)
        {
            ui.production.hover = None;
        }
    }
    if let Some(notes) = notes {
        state.dismissed.retain(|k| notes.get(*k).is_some());
    }
}

#[cfg(test)]
mod tests {
    use ui2_model::generated::{MapMode, UnitFlag};
    use ui2_model::{Key, Owner, Schema};

    use super::*;

    fn store_with(units: &[u32], notes: &[u32]) -> ModelStore {
        let mut store = ModelStore::new(Schema::v1());
        let map = Map {
            units: KeyedList::from_vec(
                units
                    .iter()
                    .map(|i| UnitFlag {
                        key: Key::new(0, *i),
                        ..UnitFlag::default()
                    })
                    .collect(),
            )
            .unwrap(),
            ..Map::default()
        };
        let notes = KeyedList::from_vec(
            notes
                .iter()
                .map(|i| Note {
                    id: Key::from_u32(*i),
                    ..Note::default()
                })
                .collect(),
        )
        .unwrap();
        let mut w = store.writer(Owner::Source);
        w.put(map).unwrap();
        w.put(notes).unwrap();
        store
    }

    #[test]
    fn a_selected_unit_that_is_gone_is_deselected_with_the_mode() {
        let mut state = UiState::new();
        state.edit().selection.unit = Some(Key::new(0, 2));
        state.edit().map_mode = MapMode::Move;

        reconcile_state(&mut state, &store_with(&[1, 2], &[]));
        assert_eq!(state.ui().selection.unit, Some(Key::new(0, 2)));
        assert_eq!(state.ui().map_mode, MapMode::Move);

        reconcile_state(&mut state, &store_with(&[1], &[]));
        assert_eq!(state.ui().selection.unit, None);
        assert_eq!(state.ui().map_mode, MapMode::Select);
    }

    #[test]
    fn dismissed_notes_the_core_forgot_are_forgotten() {
        let mut state = UiState::new();
        state.dismissed.extend([Key::from_u32(1), Key::from_u32(2)]);
        reconcile_state(&mut state, &store_with(&[], &[2, 3]));
        assert_eq!(
            state.dismissed.iter().copied().collect::<Vec<_>>(),
            vec![Key::from_u32(2)]
        );
    }
}
