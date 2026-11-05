//! The facade of the UI core: what the client and `exe-api` hold.

use std::rc::Rc;

use assets::UiFiles;
use draw_list::UiDrawList;
use lua_sys::{Lua, Runtime, RuntimeConfig, Table};
use platform::{InputEvent, Key, MouseButton, NamedKey, PointerPhase, WheelDelta};

use crate::config::UiConfig;
use crate::error::UiError;
use crate::host::{FileSource, UiHost};
use crate::input::*;
use crate::shared::{Files, Shared, style_book};
use crate::state::State;
use crate::text::TextEngine;
use crate::tree::ControlId;

/// Did the UI take the event?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputResult {
    /// The UI used the event; the world must not see it.
    Consumed,
    /// Not used by the UI; the world handles it (camera, selection).
    PassToWorld,
}

/// Counters for the debug panel (`ui.*`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiStats {
    pub controls: usize,
    pub draw_cmds: usize,
    pub lua_errors: u64,
    pub xml_warnings: u64,
    pub layout_us: u64,
    pub lua_us: u64,
}

pub struct UiCore {
    sh: Rc<Shared>,
    last_pos: (i32, i32),
    buttons: u32,
    scale: f32,
    layout_us: u64,
}

impl UiCore {
    /// `base_setup` puts the exe API (`UI`, `Events`, ...) into the base globals of the Lua state.
    pub fn new(
        cfg: UiConfig,
        files: UiFiles,
        source: Rc<dyn FileSource>,
        text: Box<dyn TextEngine>,
        host: Box<dyn UiHost>,
        base_setup: impl Fn(&Lua, &Table<'_>) -> lua_sys::Result<()> + 'static,
    ) -> Result<UiCore, UiError> {
        let scale = cfg.scale;
        let files = Rc::new(Files {
            ui: files,
            src: source,
        });
        let styles = style_book(&files);
        let state = State::new(cfg, styles, host, text);
        let sh = Shared::new(state, files, RuntimeConfig::default(), Box::new(base_setup))
            .map_err(|e| UiError::Lua(e.to_string()))?;
        Ok(UiCore {
            sh,
            last_pos: (i32::MIN, i32::MIN),
            buttons: 0,
            scale,
            layout_us: 0,
        })
    }

    /// Loads a root context by name (`InGame`) and shows it.
    /// Sets the context filter (see Shared::context_filter); None lets every context load.
    pub fn set_context_filter(&self, f: Option<crate::shared::ContextFilter>) {
        *self.sh.context_filter.borrow_mut() = f;
    }

    /// Controls (context bare name, control ID) that stay hidden: the original screens' controls that do nothing here
    /// (SIMP-exe-955). Set before the contexts load.
    pub fn set_removed_controls(&self, list: Vec<(String, String)>) {
        *self.sh.removed_controls.borrow_mut() = list;
    }

    /// Contexts the filter refused so far.
    pub fn filtered_contexts(&self) -> Vec<String> {
        self.sh.filtered.borrow().clone()
    }

    pub fn load_root(&self, name: &str) -> ControlId {
        self.sh.load_root(name)
    }

    pub fn shared(&self) -> &Rc<Shared> {
        &self.sh
    }

    pub fn runtime(&self) -> &Runtime {
        &self.sh.rt
    }

    pub fn stats(&self) -> UiStats {
        let st = self.sh.st.borrow();
        UiStats {
            controls: st.tree.live(),
            draw_cmds: 0,
            lua_errors: self.sh.rt.error_count(),
            xml_warnings: st.stats.xml_warnings,
            layout_us: self.layout_us,
            lua_us: 0,
        }
    }

    /// The window changed size: `Full` sizes and anchors follow (`OnSystemUpdateUI`).
    pub fn resize(&mut self, physical_w: u32, physical_h: u32) {
        let scale = UiConfig::scale_for_height(physical_h as f32);
        self.scale = scale;
        let (w, h) = (
            (physical_w as f32 / scale) as i32,
            (physical_h as f32 / scale) as i32,
        );
        self.sh.run(|st| {
            st.cfg.screen_width = w;
            st.cfg.screen_height = h;
            st.cfg.scale = scale;
            for r in st.roots.clone() {
                st.apply_screen_size(r);
                st.reprocess_anchoring(r);
            }
        });
    }

    pub fn update(&mut self, dt_secs: f32) {
        // SIMPLIFIED(SIMP-uic-5): no hot reload of XML or Lua, and no PerspectiveRender pass.
        self.sh.update(dt_secs);
    }

    fn msg(&self, msg: u32, w: u32, l: i32) -> bool {
        self.sh.process_message(msg, w, l)
    }

    fn pos_param(x: i32, y: i32) -> i32 {
        ((y as i16 as u16 as u32) << 16 | (x as i16 as u16 as u32)) as i32
    }

    fn mk_flags(&self) -> u32 {
        self.buttons
    }

    pub fn handle_input(&mut self, ev: &InputEvent) -> InputResult {
        let handled = match ev {
            InputEvent::Pointer(p) => {
                let x = (p.position.x / f64::from(self.scale)).floor() as i32;
                let y = (p.position.y / f64::from(self.scale)).floor() as i32;
                match p.phase {
                    PointerPhase::Move => self.mouse_move(x, y),
                    PointerPhase::Left => {
                        self.last_pos = (-1, -1);
                        self.msg(WM_MOUSEMOVE, self.mk_flags(), Self::pos_param(-1, -1))
                    }
                    PointerPhase::Down | PointerPhase::Up | PointerPhase::Cancel => {
                        let moved = if (x, y) != self.last_pos {
                            self.mouse_move(x, y)
                        } else {
                            false
                        };
                        let button = p.button.unwrap_or(MouseButton::Left);
                        let down = p.phase == PointerPhase::Down;
                        let (dn, up, bit) = match button {
                            MouseButton::Left => (WM_LBUTTONDOWN, WM_LBUTTONUP, 0x1),
                            MouseButton::Right => (WM_RBUTTONDOWN, WM_RBUTTONUP, 0x2),
                            MouseButton::Middle => (WM_MBUTTONDOWN, WM_MBUTTONUP, 0x10),
                            _ => return InputResult::PassToWorld,
                        };
                        if down {
                            self.buttons |= bit;
                        } else {
                            self.buttons &= !bit;
                        }
                        let w = self.mk_flags() | if down { bit } else { 0 };
                        let h = self.msg(if down { dn } else { up }, w, Self::pos_param(x, y));
                        let _ = moved;
                        h
                    }
                }
            }
            InputEvent::Wheel(wh) => {
                let x = (wh.position.x / f64::from(self.scale)).floor() as i32;
                let y = (wh.position.y / f64::from(self.scale)).floor() as i32;
                if (x, y) != self.last_pos {
                    self.mouse_move(x, y);
                }
                let notches = match wh.delta {
                    WheelDelta::Lines { y, .. } => y,
                    WheelDelta::Pixels { y, .. } => (y / 40.0) as f32,
                };
                let delta = (notches * 120.0) as i32 as i16;
                self.msg(
                    WM_MOUSEWHEEL,
                    (delta as u16 as u32) << 16 | self.mk_flags(),
                    Self::pos_param(x, y),
                )
            }
            InputEvent::Key(k) => {
                // Typed characters reach a focused edit box as WM_CHAR (the browser platform sends keys, not text,
                // and the exe's window procedure turns them into characters).
                let mut typed = false;
                if k.pressed
                    && !k.modifiers.ctrl
                    && !k.modifiers.meta
                    && self.sh.st.borrow().focus.is_some()
                {
                    match &k.key {
                        Key::Character(s) if !s.chars().all(char::is_control) => {
                            for c in s.chars() {
                                self.msg(WM_CHAR, c as u32, 0);
                            }
                            typed = true;
                        }
                        Key::Named(NamedKey::Space) => {
                            self.msg(WM_CHAR, 0x20, 0);
                            typed = true;
                        }
                        _ => {}
                    }
                }
                let Some(vk) = key_vk(k) else {
                    return if typed {
                        InputResult::Consumed
                    } else {
                        InputResult::PassToWorld
                    };
                };
                let h = self.msg(if k.pressed { WM_KEYDOWN } else { WM_KEYUP }, vk, 0);
                if k.pressed && matches!(vk, 8 | 13 | 27) && self.sh.st.borrow().focus.is_some() {
                    self.msg(WM_CHAR, vk, 0);
                }
                h || typed
            }
            InputEvent::Text(s) => {
                let mut h = false;
                for c in s.chars() {
                    h |= self.msg(WM_CHAR, c as u32, 0);
                }
                h
            }
            InputEvent::Resized(sz) => {
                self.resize(sz.width, sz.height);
                false
            }
            InputEvent::ScaleFactor(_) | InputEvent::Focused(_) => false,
        };
        if handled {
            InputResult::Consumed
        } else {
            InputResult::PassToWorld
        }
    }

    fn mouse_move(&mut self, x: i32, y: i32) -> bool {
        self.last_pos = (x, y);
        self.msg(WM_MOUSEMOVE, self.mk_flags(), Self::pos_param(x, y))
    }

    pub fn draw(&self, out: &mut UiDrawList) {
        out.clear();
        self.sh.st.borrow_mut().draw_all(out, self.scale);
    }

    /// A text dump of the live tree, for the debug panel and for the M1 output.
    pub fn dump_tree(&self) -> String {
        crate::debug::dump(&self.sh.st.borrow())
    }
}

/// Windows virtual-key code of a key event. A character key whose character has no code of its own (a Cyrillic
/// layout gives "а" for the F key) takes the code of its place on the keyboard, as Windows gives the letter and digit
/// keys of such layouts their Latin codes.
pub fn key_vk(k: &platform::KeyEvent) -> Option<u32> {
    vk_of(&k.key).or_else(|| match &k.key {
        Key::Character(_) => k.code.as_deref().and_then(vk_of_code),
        _ => None,
    })
}

/// Windows virtual-key code of a DOM `KeyboardEvent.code` of a letter, digit or punctuation key.
pub fn vk_of_code(code: &str) -> Option<u32> {
    if let Some(l) = code.strip_prefix("Key")
        && let [c] = l.as_bytes()
        && c.is_ascii_uppercase()
    {
        return Some(u32::from(*c));
    }
    if let Some(d) = code.strip_prefix("Digit")
        && let [c] = d.as_bytes()
        && c.is_ascii_digit()
    {
        return Some(u32::from(*c));
    }
    Some(match code {
        "Semicolon" => 0xba,
        "Equal" => 0xbb,
        "Comma" => 0xbc,
        "Minus" => 0xbd,
        "Period" => 0xbe,
        "Slash" => 0xbf,
        "Backquote" => 0xc0,
        "BracketLeft" => 0xdb,
        "Backslash" => 0xdc,
        "BracketRight" => 0xdd,
        "Quote" => 0xde,
        _ => return None,
    })
}

/// Windows virtual-key code of a key.
pub fn vk_of(key: &Key) -> Option<u32> {
    Some(match key {
        Key::Named(n) => match n {
            NamedKey::Enter => 13,
            NamedKey::Escape => 27,
            NamedKey::Tab => 9,
            NamedKey::Backspace => 8,
            NamedKey::Delete => 0x2e,
            NamedKey::Insert => 0x2d,
            NamedKey::Space => 0x20,
            NamedKey::ArrowUp => 0x26,
            NamedKey::ArrowDown => 0x28,
            NamedKey::ArrowLeft => 0x25,
            NamedKey::ArrowRight => 0x27,
            NamedKey::Home => 0x24,
            NamedKey::End => 0x23,
            NamedKey::PageUp => 0x21,
            NamedKey::PageDown => 0x22,
            NamedKey::Shift => 0x10,
            NamedKey::Control => 0x11,
            NamedKey::Alt => 0x12,
            NamedKey::Meta => 0x5b,
            NamedKey::F1 => 0x70,
            NamedKey::F2 => 0x71,
            NamedKey::F3 => 0x72,
            NamedKey::F4 => 0x73,
            NamedKey::F5 => 0x74,
            NamedKey::F6 => 0x75,
            NamedKey::F7 => 0x76,
            NamedKey::F8 => 0x77,
            NamedKey::F9 => 0x78,
            NamedKey::F10 => 0x79,
            NamedKey::F11 => 0x7a,
            NamedKey::F12 => 0x7b,
        },
        Key::Character(s) => {
            let c = s.chars().next()?;
            if c.is_ascii_alphabetic() {
                c.to_ascii_uppercase() as u32
            } else if c.is_ascii_digit() {
                c as u32
            } else {
                match c {
                    ';' => 0xba,
                    '=' => 0xbb,
                    ',' => 0xbc,
                    '-' => 0xbd,
                    '.' => 0xbe,
                    '/' => 0xbf,
                    '`' => 0xc0,
                    '[' => 0xdb,
                    '\\' => 0xdc,
                    ']' => 0xdd,
                    '\'' => 0xde,
                    _ => return None,
                }
            }
        }
        Key::Unidentified => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(key: Key, code: &str) -> platform::KeyEvent {
        platform::KeyEvent {
            key,
            code: Some(code.into()),
            pressed: true,
            repeat: false,
            modifiers: platform::Modifiers::default(),
        }
    }

    #[test]
    fn a_letter_of_another_layout_has_the_code_of_its_place() {
        // The F key under a Cyrillic layout gives "а": Windows gives it VK_F.
        assert_eq!(key_vk(&key(Key::Character("а".into()), "KeyF")), Some(0x46));
        assert_eq!(key_vk(&key(Key::Character("f".into()), "KeyF")), Some(0x46));
        // Shift+1 gives "!": the key is still VK_1.
        assert_eq!(
            key_vk(&key(Key::Character("!".into()), "Digit1")),
            Some(0x31)
        );
        assert_eq!(
            key_vk(&key(Key::Character("ж".into()), "Semicolon")),
            Some(0xba)
        );
        assert_eq!(
            key_vk(&key(Key::Named(NamedKey::Space), "Space")),
            Some(0x20)
        );
        assert_eq!(
            key_vk(&key(Key::Character("§".into()), "IntlBackslash")),
            None
        );
    }
}
