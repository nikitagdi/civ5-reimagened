//! The client's options of UI v2 in the app: the running UI gets the store the classic client keeps its options in
//! (the page's store through [`crate::account_sync::AccountKv`], so the account follows what either client writes),
//! and the volume knobs its options window changes reach the audio buses as the classic options menu's do
//! ([`UiSoundSource::take_knobs`]).

use std::cell::RefCell;
use std::rc::Rc;

use platform::KeyValueStore;
use ui2_host::{KvSettings, V2Ui};

use crate::ui_host::UiSoundSource;

/// What UI v2 needs for the client's options.
#[derive(Default)]
pub(crate) struct V2Client {
    /// The classic client's options store; `None`: the options live in memory for the session.
    kv: Option<Rc<dyn KeyValueStore>>,
    /// The volume of a knob never saved (0 to 1, the page's `?volume=`).
    volume_default: f32,
    /// Knobs the options window moved since the audio last took them.
    knobs: RefCell<Vec<(String, f32)>>,
}

impl V2Client {
    pub(crate) fn set(&mut self, kv: Rc<dyn KeyValueStore>, volume_default: f32) {
        self.kv = Some(kv);
        self.volume_default = volume_default;
    }

    /// A new UI v2 (a skin came up): it reads and writes the options in the store.
    pub(crate) fn attach(&self, ui: &mut V2Ui) {
        if let Some(kv) = &self.kv {
            ui.set_client_store(Box::new(KvSettings(Rc::clone(kv))), self.volume_default);
        }
    }

    /// After a frame of UI v2: the knobs its options window moved.
    pub(crate) fn after_frame(&self, ui: &mut V2Ui) {
        let moved = ui.take_volume_changes();
        if !moved.is_empty() {
            self.knobs
                .borrow_mut()
                .extend(moved.into_iter().map(|(k, v)| (k.to_owned(), v)));
        }
    }
}

impl UiSoundSource for V2Client {
    fn take_knobs(&self) -> Vec<(String, f32)> {
        std::mem::take(&mut *self.knobs.borrow_mut())
    }

    // SIMPLIFIED(SIMP-265): UI v2 plays no interface sounds (button clicks, the original's screen sounds); the
    // world's sounds and the music go on.
    fn take_button_sounds(&self) -> Vec<(audio::ButtonEvent, audio::ButtonFlags)> {
        Vec::new()
    }

    fn take_sound_scripts(&self) -> Vec<String> {
        Vec::new()
    }

    fn take_music_events(&self) -> Vec<exe_api::music::MusicEvent> {
        Vec::new()
    }

    fn music_state(&self) -> Option<exe_api::music::MusicState> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::MemoryStore;

    #[test]
    fn the_knobs_go_once_to_the_audio() {
        let c = V2Client::default();
        c.knobs.borrow_mut().push(("MUSIC".into(), 0.5));
        assert_eq!(c.take_knobs(), vec![("MUSIC".to_owned(), 0.5)]);
        assert!(c.take_knobs().is_empty());
        assert!(c.music_state().is_none() && c.take_button_sounds().is_empty());
    }

    /// UI v2's options window and the classic options menu agree on the keys, the defaults, the knobs and the store.
    #[test]
    fn the_catalogue_is_the_classic_clients() {
        use ui2_actions::options::{CATALOGUE, KEY_PREFIX, KNOBS, OptKind, parse};
        assert_eq!(KEY_PREFIX, exe_api::options::KEY_PREFIX);
        assert_eq!(KNOBS, exe_api::m4_wog::KNOBS);
        assert_eq!(crate::account_sync::LOCAL_PREFIX, KEY_PREFIX);
        assert_eq!(
            ui2_actions::auto_end::OPTION_KEY,
            exe_api::auto_end::OPTION_KEY
        );
        for d in CATALOGUE {
            if d.kind == OptKind::Volume {
                let knob = d.key.strip_prefix("Volume.").expect("a knob");
                assert!(KNOBS.contains(&knob), "{}", d.key);
                continue;
            }
            let classic = exe_api::m4_wog::option_default(d.key)
                .unwrap_or_else(|| panic!("{} is not an option of the classic menu", d.key));
            assert_eq!(parse(d.kind, &classic), Some(d.default), "{}", d.key);
            assert!(
                crate::account_sync::account_key(&format!("{KEY_PREFIX}{}", d.key)).is_some(),
                "the account follows {}",
                d.key
            );
        }
        assert!(
            crate::account_sync::account_key(&format!(
                "{KEY_PREFIX}{}",
                ui2_actions::hotkeys::STORE_KEY
            ))
            .is_some()
        );
    }

    #[test]
    fn the_store_is_the_classic_options_store() {
        let mem = MemoryStore::new();
        mem.set("civ5.options.MultiplayerAutoEndTurn", "1");
        let mut kv = KvSettings(Rc::new(mem.clone()));
        assert_eq!(
            ui2_actions::SettingsStore::get(&kv, "civ5.options.MultiplayerAutoEndTurn").as_deref(),
            Some("1")
        );
        ui2_actions::SettingsStore::set(&mut kv, "civ5.options.Volume.MUSIC", "0.5");
        assert_eq!(mem.get("civ5.options.Volume.MUSIC").as_deref(), Some("0.5"));
    }
}
