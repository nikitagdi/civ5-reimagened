//! What the client gives the UI core: textures by name, button sounds, the cursor, the world
//! projection, and the files.

use std::collections::HashMap;

use audio::{ButtonEvent, ButtonFlags};
use draw_list::TextureId;

/// A texture the UI can draw. The size is in pixels; layout needs it at once
/// (`SetTextureAndResize`, UV normalisation, an `Image` without a `Size`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureInfo {
    pub id: TextureId,
    pub width: u32,
    pub height: u32,
}

pub trait UiHost {
    /// The texture a name stands for (`Assets/UI/Art/X.dds`, `X.dds`), or `None` while unknown.
    fn texture(&mut self, name: &str) -> Option<TextureInfo>;
    /// A button sound (`audio::UiAudio::on_button`).
    fn button_sound(&mut self, event: ButtonEvent, flags: ButtonFlags);
    /// `UIManager:SetUICursor`.
    fn set_cursor(&mut self, _cursor: i32) {}
    /// World point to UI pixels for `WorldAnchor`; `None` when it is not on the screen.
    fn project(&mut self, _world: [f32; 3]) -> Option<[f32; 2]> {
        None
    }
    /// Seconds since the start of the program.
    fn now(&self) -> f64 {
        0.0
    }
}

/// Where the bytes of a UI file come from. `path` is the path an entry of the UI file index names.
pub trait FileSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// Files in memory.
#[derive(Debug, Default)]
pub struct MemoryFiles {
    pub files: HashMap<String, Vec<u8>>,
}

impl FileSource for MemoryFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }
}

/// A host for tests and tools: textures from a table, a log of button sounds.
#[derive(Debug, Default)]
pub struct StubHost {
    pub textures: HashMap<String, (u32, u32)>,
    /// Size given to a texture that is not in the table (`None`: unknown textures are missing).
    pub default_size: Option<(u32, u32)>,
    pub sounds: std::rc::Rc<std::cell::RefCell<Vec<(ButtonEvent, ButtonFlags)>>>,
    pub cursor: i32,
    ids: HashMap<String, u64>,
}

impl StubHost {
    pub fn with_default_size(w: u32, h: u32) -> StubHost {
        StubHost {
            default_size: Some((w, h)),
            ..StubHost::default()
        }
    }

    pub fn add_texture(&mut self, name: &str, w: u32, h: u32) -> &mut Self {
        self.textures.insert(crate::bare_lower(name), (w, h));
        self
    }
}

impl UiHost for StubHost {
    fn texture(&mut self, name: &str) -> Option<TextureInfo> {
        let key = crate::bare_lower(name);
        let (w, h) = self.textures.get(&key).copied().or(self.default_size)?;
        let next = self.ids.len() as u64 + 1;
        let id = *self.ids.entry(key).or_insert(next);
        Some(TextureInfo {
            id: TextureId(id),
            width: w,
            height: h,
        })
    }

    fn button_sound(&mut self, event: ButtonEvent, flags: ButtonFlags) {
        self.sounds.borrow_mut().push((event, flags));
    }

    fn set_cursor(&mut self, cursor: i32) {
        self.cursor = cursor;
    }
}
