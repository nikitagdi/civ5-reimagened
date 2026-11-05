//! The hook for the sounds of buttons.
//!
//! The rules of the exe for click and mouse-over sounds are not known yet (they come from the
//! reverse of the UI). The engine therefore plays nothing for a button until the rules are set.

use std::collections::BTreeMap;

/// What happened to a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ButtonEvent {
    Click,
    MouseOver,
    MouseExit,
    Press,
}

/// Kinds of control the rules may tell apart. The names are the XML tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ControlKind {
    Button,
    GridButton,
    TextButton,
    CheckBox,
    RadioButton,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonFlags {
    /// The `NoDefaultSound` attribute of the control.
    pub no_default_sound: bool,
    pub disabled: bool,
    pub kind: ControlKind,
}

/// Script per event, with an optional table per control kind that wins over it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ButtonSoundRules {
    pub default: BTreeMap<ButtonEvent, String>,
    pub per_kind: BTreeMap<(ControlKind, ButtonEvent), String>,
    /// Play for a disabled control too.
    pub play_when_disabled: bool,
}

impl ButtonSoundRules {
    /// No rules: buttons are silent.
    pub fn none() -> Self {
        Self::default()
    }

    /// SIMPLIFIED(SIMP-uiau-1): the click script is `AS2D_IF_SELECT` (`SND_INTERFACE_SELECT`,
    /// `InterfaceButtonClick_1`); nothing is played for mouse-over. The real rules of the exe are
    /// not read yet. Remove when `re-ui` has them.
    pub fn provisional() -> Self {
        let mut r = Self::none();
        r.default
            .insert(ButtonEvent::Click, "AS2D_IF_SELECT".to_owned());
        r
    }

    /// The script to play, if any.
    pub fn script_for(&self, ev: ButtonEvent, flags: ButtonFlags) -> Option<&str> {
        if flags.disabled && !self.play_when_disabled {
            return None;
        }
        self.per_kind
            .get(&(flags.kind, ev))
            .or_else(|| {
                // `NoDefaultSound` turns off the default table, not a per-kind rule.
                (!flags.no_default_sound)
                    .then(|| self.default.get(&ev))
                    .flatten()
            })
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flags(no_default: bool) -> ButtonFlags {
        ButtonFlags {
            no_default_sound: no_default,
            disabled: false,
            kind: ControlKind::Button,
        }
    }

    #[test]
    fn no_rules_means_silence() {
        let r = ButtonSoundRules::none();
        assert_eq!(r.script_for(ButtonEvent::Click, flags(false)), None);
    }

    #[test]
    fn provisional_clicks_but_does_not_hover() {
        let r = ButtonSoundRules::provisional();
        assert_eq!(
            r.script_for(ButtonEvent::Click, flags(false)),
            Some("AS2D_IF_SELECT")
        );
        assert_eq!(r.script_for(ButtonEvent::MouseOver, flags(false)), None);
    }

    #[test]
    fn no_default_sound_and_disabled_and_per_kind() {
        let mut r = ButtonSoundRules::provisional();
        assert_eq!(r.script_for(ButtonEvent::Click, flags(true)), None);
        r.per_kind
            .insert((ControlKind::Button, ButtonEvent::Click), "AS2D_X".into());
        assert_eq!(
            r.script_for(ButtonEvent::Click, flags(true)),
            Some("AS2D_X")
        );
        let mut f = flags(false);
        f.disabled = true;
        assert_eq!(r.script_for(ButtonEvent::Click, f), None);
        r.play_when_disabled = true;
        assert_eq!(r.script_for(ButtonEvent::Click, f), Some("AS2D_X"));
    }
}
