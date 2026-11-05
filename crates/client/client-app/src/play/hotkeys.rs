//! Keyboard bindings. Pure: a key event in, an action out.

use platform::{Key, KeyEvent, NamedKey};

/// What a key does in the game view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotKey {
    /// Enter / Shift+Enter.
    EndTurn { force: bool },
    /// G.
    ToggleGrid,
    /// C: centre the camera on the selected unit.
    Center,
    /// W.
    NextUnit,
    /// Esc.
    Escape,
    /// F.
    Fortify,
    /// S.
    Sleep,
    /// Space.
    Skip,
    /// B: the ranged attack mode for a unit with a ranged strength, else found a city (both are B in
    /// the game's InterfaceModes / Missions tables).
    FoundCity,
    /// H.
    Heal,
    /// A (AUTOMATE_BUILD).
    Automate,
    /// E (AUTOMATE_EXPLORE).
    Explore,
    /// Backspace (COMMAND_CANCEL).
    Cancel,
}

/// `None` for releases, repeats, chords with Ctrl / Alt / Meta and unbound keys.
pub fn map(k: &KeyEvent) -> Option<HotKey> {
    if !k.pressed || k.repeat || k.modifiers.ctrl || k.modifiers.alt || k.modifiers.meta {
        return None;
    }
    match &k.key {
        Key::Named(NamedKey::Enter) => Some(HotKey::EndTurn {
            force: k.modifiers.shift,
        }),
        Key::Named(NamedKey::Escape) => Some(HotKey::Escape),
        Key::Named(NamedKey::Space) => Some(HotKey::Skip),
        Key::Named(NamedKey::Backspace) => Some(HotKey::Cancel),
        Key::Character(c) => match c.to_ascii_lowercase().as_str() {
            " " => Some(HotKey::Skip),
            "g" => Some(HotKey::ToggleGrid),
            "c" => Some(HotKey::Center),
            "w" => Some(HotKey::NextUnit),
            "f" => Some(HotKey::Fortify),
            "s" => Some(HotKey::Sleep),
            "b" => Some(HotKey::FoundCity),
            "h" => Some(HotKey::Heal),
            "a" => Some(HotKey::Automate),
            "e" => Some(HotKey::Explore),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use platform::Modifiers;

    use super::*;

    fn ev(key: Key, pressed: bool, modifiers: Modifiers) -> KeyEvent {
        KeyEvent {
            key,
            code: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    fn ch(c: &str) -> KeyEvent {
        ev(Key::Character(c.into()), true, Modifiers::default())
    }

    #[test]
    fn the_table() {
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(
            map(&ev(Key::Named(NamedKey::Enter), true, Modifiers::default())),
            Some(HotKey::EndTurn { force: false })
        );
        assert_eq!(
            map(&ev(Key::Named(NamedKey::Enter), true, shift)),
            Some(HotKey::EndTurn { force: true })
        );
        assert_eq!(map(&ch("g")), Some(HotKey::ToggleGrid));
        assert_eq!(map(&ch("G")), Some(HotKey::ToggleGrid));
        assert_eq!(map(&ch("c")), Some(HotKey::Center));
        assert_eq!(map(&ch("w")), Some(HotKey::NextUnit));
        assert_eq!(map(&ch("f")), Some(HotKey::Fortify));
        assert_eq!(map(&ch("s")), Some(HotKey::Sleep));
        assert_eq!(map(&ch("b")), Some(HotKey::FoundCity));
        assert_eq!(map(&ch("h")), Some(HotKey::Heal));
        assert_eq!(map(&ch("a")), Some(HotKey::Automate));
        assert_eq!(map(&ch("e")), Some(HotKey::Explore));
        assert_eq!(
            map(&ev(
                Key::Named(NamedKey::Backspace),
                true,
                Modifiers::default()
            )),
            Some(HotKey::Cancel)
        );
        assert_eq!(map(&ch(" ")), Some(HotKey::Skip));
        assert_eq!(
            map(&ev(Key::Named(NamedKey::Space), true, Modifiers::default())),
            Some(HotKey::Skip)
        );
        assert_eq!(
            map(&ev(
                Key::Named(NamedKey::Escape),
                true,
                Modifiers::default()
            )),
            Some(HotKey::Escape)
        );
        assert_eq!(map(&ch("x")), None);
    }

    #[test]
    fn releases_repeats_and_chords_are_ignored() {
        let mut k = ch("g");
        k.pressed = false;
        assert_eq!(map(&k), None);
        let mut k = ch("g");
        k.repeat = true;
        assert_eq!(map(&k), None);
        let mut k = ch("s");
        k.modifiers.ctrl = true;
        assert_eq!(map(&k), None);
    }
}
