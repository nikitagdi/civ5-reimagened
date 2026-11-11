//! The text field of UI v2: the platform edits, the UI draws.
//!
//! A UI v2 skin draws its text fields itself (its own fonts, glyphs, selection and caret), but the editing
//! is the platform's: on the web a hidden `<textarea>` takes the keyboard while a field has the focus, so
//! typing, paste, undo and input methods (IME) behave as in any page. The UI asks for the field with a
//! [`TextInputRequest`]; the host polls the bridge every frame and hands each new [`TextInputState`] to the
//! UI. Offsets are UTF-8 byte offsets into the value on both sides (the web bridge converts from the
//! browser's UTF-16 ones with [`utf16_to_utf8`] and [`utf8_to_utf16`]).
//!
//! The native client has no bridge ([`NullTextInput`]).

/// A selection in a text: `anchor` is where it started, `caret` where it ends and the caret is (UTF-8 byte
/// offsets). Equal offsets are a caret without a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextSel {
    /// Where the selection started.
    pub anchor: u32,
    /// Where it ends: the caret.
    pub caret: u32,
}

impl TextSel {
    /// A caret at `at`, nothing selected.
    pub const fn caret(at: u32) -> Self {
        Self {
            anchor: at,
            caret: at,
        }
    }

    /// The selected range, start first.
    pub fn range(&self) -> (u32, u32) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// Nothing is selected.
    pub fn is_empty(&self) -> bool {
        self.anchor == self.caret
    }

    /// The selection inside `text`: offsets past the end are moved to the end, offsets inside a character
    /// to its start.
    pub fn clamped(&self, text: &str) -> Self {
        let fix = |o: u32| {
            let mut o = (o as usize).min(text.len());
            while !text.is_char_boundary(o) {
                o -= 1;
            }
            o as u32
        };
        Self {
            anchor: fix(self.anchor),
            caret: fix(self.caret),
        }
    }
}

/// The text field as the platform has it, handed to the UI every frame it changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextInputState {
    /// The text.
    pub value: String,
    /// The selection or caret.
    pub sel: TextSel,
    /// The text an input method is composing, as a byte range of `value` (drawn underlined), while it does.
    pub composing: Option<(u32, u32)>,
    /// Enter was pressed (outside a composition) since the last state.
    pub submitted: bool,
    /// Escape was pressed since the last state.
    pub cancelled: bool,
    /// Tab was pressed since the last state: `Some(true)` with Shift. The field keeps the keyboard, so the UI
    /// moves the focus itself.
    pub tab: Option<bool>,
}

/// What the UI asks of the platform's text field.
#[derive(Debug, Clone, PartialEq)]
pub enum TextInputRequest {
    /// A field took the focus (or its text was changed by the UI while it has it): edit this value with
    /// this selection. `rect` is the field on the screen, physical window pixels `[x, y, w, h]`, where an
    /// input method opens its window.
    Focus {
        /// The text.
        value: String,
        /// The selection or caret.
        sel: TextSel,
        /// The field on the screen.
        rect: [f32; 4],
    },
    /// No field has the focus: the keyboard goes back to the game.
    Blur,
}

/// The platform's text field (web: a hidden `<textarea>`).
pub trait TextInputBridge {
    /// Takes a request of the UI.
    fn request(&mut self, req: &TextInputRequest);

    /// The field's state when it changed since the last call (typing, paste, the selection, a composition,
    /// Enter, Escape, Tab), else `None`. Called once per frame.
    fn poll(&mut self) -> Option<TextInputState>;
}

/// The bridge of a platform without one: requests are ignored and nothing is ever typed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullTextInput;

impl TextInputBridge for NullTextInput {
    fn request(&mut self, _req: &TextInputRequest) {}

    fn poll(&mut self) -> Option<TextInputState> {
        None
    }
}

/// A UTF-16 offset of `text` (the browser's) as a UTF-8 byte offset; an offset inside a character or past
/// the end gives the next character boundary or the end.
pub fn utf16_to_utf8(text: &str, utf16: u32) -> u32 {
    let mut units = 0u32;
    for (i, c) in text.char_indices() {
        if units >= utf16 {
            return i as u32;
        }
        units += c.len_utf16() as u32;
    }
    text.len() as u32
}

/// A UTF-8 byte offset of `text` as a UTF-16 offset (the browser's).
pub fn utf8_to_utf16(text: &str, utf8: u32) -> u32 {
    let end = (utf8 as usize).min(text.len());
    text.char_indices()
        .take_while(|(i, _)| *i < end)
        .map(|(_, c)| c.len_utf16() as u32)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_convert_between_utf8_and_utf16() {
        // "a", "é" (2 bytes, 1 unit), "😀" (4 bytes, 2 units), "b".
        let s = "aé😀b";
        assert_eq!(utf16_to_utf8(s, 0), 0);
        assert_eq!(utf16_to_utf8(s, 2), 3);
        assert_eq!(utf16_to_utf8(s, 4), 7);
        assert_eq!(
            utf16_to_utf8(s, 3),
            7,
            "inside a surrogate pair: the next boundary"
        );
        assert_eq!(utf16_to_utf8(s, 9), 8);
        assert_eq!(utf8_to_utf16(s, 3), 2);
        assert_eq!(utf8_to_utf16(s, 7), 4);
        assert_eq!(utf8_to_utf16(s, 8), 5);
        assert_eq!(utf8_to_utf16(s, 99), 5);
    }

    #[test]
    fn selections_clamp_to_the_text() {
        let s = "aé";
        let sel = TextSel {
            anchor: 2,
            caret: 10,
        };
        assert_eq!(
            sel.clamped(s),
            TextSel {
                anchor: 1,
                caret: 3
            }
        );
        assert_eq!(
            TextSel {
                anchor: 3,
                caret: 1
            }
            .range(),
            (1, 3)
        );
        assert!(TextSel::caret(2).is_empty());
        let mut bridge = NullTextInput;
        bridge.request(&TextInputRequest::Blur);
        assert_eq!(bridge.poll(), None);
    }
}
