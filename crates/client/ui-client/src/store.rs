//! The loaded files of the `ui` set: raw bytes by manifest id, decoded textures, and the lookups
//! `ui` asks for (file by path, texture by name with its size).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use asset_format::{FormatError, TextureBlob, UiFileInfo, UiFileKind, UiIndex};
use assets::UiFiles;
use draw_list::TextureId;
use ui::{FileSource, TextureInfo};

/// A file the client adds to the index (the fixture root of M1): it is not in the asset set.
#[derive(Debug, Clone)]
pub struct MemoryFile {
    /// Path as the index gives it (`fixture/M1Root.xml`).
    pub path: String,
    pub kind: UiFileKind,
    pub bytes: Vec<u8>,
}

pub struct UiStore {
    files: UiFiles,
    /// Manifest id to bytes, for everything that is not a texture.
    blobs: RefCell<HashMap<String, Rc<Vec<u8>>>>,
    /// Decoded textures by id. The size is needed at once by layout, so a texture is decoded when it arrives.
    textures: RefCell<HashMap<TextureId, Rc<TextureBlob>>>,
    /// Index path to manifest id.
    path_ids: HashMap<String, String>,
    tex_by_manifest: HashMap<String, TextureId>,
    /// Textures of the fonts: their one channel is coverage, which the glyph shader reads as alpha.
    font_textures: HashSet<TextureId>,
    missing: RefCell<BTreeSet<String>>,
    /// Pixel size of the textures by bare lower-case file name: the sizes of the index, or for an index without them
    /// (version 1) those of the icon atlases that the atlas rows give (`icon size * icons per row` by `icon size *
    /// icons per column`).
    tex_dims: HashMap<String, (u32, u32)>,
    lazy: RefCell<Lazy>,
}

/// Textures the UI named that are in the index and not in the store: fetched in the background (M3 WO-I).
#[derive(Default)]
struct Lazy {
    /// Manifest ids waiting for a request.
    want: BTreeSet<String>,
    /// Requested and not arrived.
    in_flight: BTreeSet<String>,
    failed: BTreeSet<String>,
    requested: u64,
    loaded: u64,
}

/// Counters of the background texture loading (`ui.tex_lazy_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LazyStats {
    pub requested: u64,
    pub loaded: u64,
    pub failed: u64,
    /// Waiting for a request or in flight.
    pub pending: u64,
}

impl UiStore {
    /// `index` plus the in-memory files. The ids of textures are the positions in the index (stable for one index).
    pub fn new(mut index: UiIndex, memory: Vec<MemoryFile>) -> Result<Rc<UiStore>, FormatError> {
        let mut put = Vec::new();
        for m in memory {
            let name = ui::bare_lower(&m.path);
            let id = format!("ui:f:{name}");
            index.files.retain(|f| f.name != name);
            index.files.push(UiFileInfo {
                name,
                id: id.clone(),
                kind: m.kind,
                layer: 0,
                size: m.bytes.len() as u64,
                path: m.path,
                hidden: 0,
                index_of: None,
            });
            put.push((id, m.bytes));
        }
        index.files.sort_by(|a, b| a.name.cmp(&b.name));
        let files = UiFiles::open(&index.encode())?;
        let idx = files.index();
        let path_ids = idx
            .files
            .iter()
            .map(|f| (f.path.clone(), f.id.clone()))
            .collect();
        let tex_by_manifest: HashMap<String, TextureId> = idx
            .files
            .iter()
            .enumerate()
            .filter(|(_, f)| f.kind == UiFileKind::Texture)
            .map(|(i, f)| (f.id.clone(), TextureId(i as u64 + 1)))
            .collect();
        let font_textures = idx
            .fonts
            .iter()
            .flat_map(|f| f.textures.iter())
            .filter_map(|t| {
                let info = idx.find(t)?;
                tex_by_manifest.get(&info.id).copied()
            })
            .collect();
        let mut tex_dims: HashMap<String, (u32, u32)> = idx
            .tex_dims
            .iter()
            .map(|d| (d.file.to_ascii_lowercase(), (d.width, d.height)))
            .collect();
        for a in &idx.atlases {
            tex_dims.entry(a.file.to_ascii_lowercase()).or_insert((
                a.icon_size * a.icons_per_row,
                a.icon_size * a.icons_per_column,
            ));
        }
        let store = Rc::new(UiStore {
            files,
            blobs: RefCell::default(),
            textures: RefCell::default(),
            path_ids,
            tex_by_manifest,
            font_textures,
            missing: RefCell::default(),
            tex_dims,
            lazy: RefCell::default(),
        });
        for (id, bytes) in put {
            store.blobs.borrow_mut().insert(id, Rc::new(bytes));
        }
        Ok(store)
    }

    pub fn files(&self) -> &UiFiles {
        &self.files
    }

    pub fn index(&self) -> &UiIndex {
        self.files.index()
    }

    /// Stores the bytes of a manifest id. A texture is decoded at once.
    pub fn put(&self, id: &str, bytes: Vec<u8>) -> Result<(), FormatError> {
        if let Some(&tex) = self.tex_by_manifest.get(id) {
            let blob = TextureBlob::decode(&bytes)?;
            // A compressed variant may be padded to whole blocks or (`Astc`) stored smaller: its
            // image part is what must match.
            if let Some(name) = id.strip_prefix("ui:tx:")
                && let Some(&(w, h)) = self.tex_dims.get(name)
                && asset_format::stored_shrink((w, h), (blob.width, blob.height)).is_none()
            {
                // The placeholder size the controls took was wrong: the quads of this texture are cut at the wrong places.
                tracing::warn!(
                    texture = name,
                    "the texture is {}x{}, the index says {w}x{h}",
                    blob.width,
                    blob.height
                );
            }
            self.textures.borrow_mut().insert(tex, Rc::new(blob));
        } else {
            self.blobs
                .borrow_mut()
                .insert(id.to_owned(), Rc::new(bytes));
        }
        Ok(())
    }

    pub fn has(&self, id: &str) -> bool {
        self.blobs.borrow().contains_key(id)
            || self
                .tex_by_manifest
                .get(id)
                .is_some_and(|t| self.textures.borrow().contains_key(t))
    }

    pub fn bytes(&self, id: &str) -> Option<Rc<Vec<u8>>> {
        self.blobs.borrow().get(id).cloned()
    }

    /// A file by bare name (`twcenmt16.ggxml`), as UTF-8 without the byte order mark.
    pub fn text_of_name(&self, name: &str) -> Option<String> {
        let info = self.files.index().find(name)?;
        let bytes = self.bytes(&info.id)?;
        Some(
            String::from_utf8_lossy(&bytes)
                .trim_start_matches('\u{feff}')
                .to_owned(),
        )
    }

    /// The id a texture file has for the renderer.
    pub fn texture_id_of_name(&self, name: &str) -> Option<TextureId> {
        let info = self.files.find_path(name)?;
        self.tex_by_manifest.get(&info.id).copied()
    }

    /// `UiHost::texture`: the id and the pixel size of a texture that has arrived.
    ///
    /// A texture that is in the index and has not arrived is fetched in the background (`take_wanted_fetches`). Its
    /// size is in the index, so the answer is the id and that size: the control takes the texture, draws nothing until
    /// the renderer has it (the original shows nothing for a frame or two) and shows it once it arrives.
    pub fn texture_info(&self, name: &str) -> Option<TextureInfo> {
        let found = self.files.find_path(name).and_then(|info| {
            let id = *self.tex_by_manifest.get(&info.id)?;
            if let Some(blob) = self.textures.borrow().get(&id) {
                // The size the UI lays out with is the image's, not the stored blob's (a
                // compressed variant may be padded or smaller).
                let stored = (blob.width, blob.height);
                let (width, height) = match self.tex_dims.get(&info.name) {
                    Some(&l) if asset_format::stored_shrink(l, stored).is_some() => l,
                    _ => stored,
                };
                return Some(TextureInfo { id, width, height });
            }
            let mut lazy = self.lazy.borrow_mut();
            if lazy.failed.contains(&info.id) {
                return None;
            }
            if !lazy.in_flight.contains(&info.id) {
                lazy.want.insert(info.id.clone());
            }
            let &(width, height) = self.tex_dims.get(&info.name)?;
            Some(TextureInfo { id, width, height })
        });
        if found.is_none() {
            self.missing.borrow_mut().insert(ui::bare_lower(name));
        }
        found
    }

    /// Up to `max` manifest ids of textures the UI asked for and nobody has requested yet, the small ones first (the
    /// icons of the first frames come before the big sheets); they count as in flight until `fetch_done` or
    /// `fetch_failed`.
    pub fn take_wanted_fetches(&self, max: usize) -> Vec<String> {
        let mut lazy = self.lazy.borrow_mut();
        let files = &self.files.index().files;
        let size_of = |id: &str| {
            self.tex_by_manifest
                .get(id)
                .and_then(|t| files.get(t.0 as usize - 1))
                .map_or(0, |f| f.size)
        };
        let mut order: Vec<(u64, String)> = lazy
            .want
            .iter()
            .map(|id| (size_of(id), id.clone()))
            .collect();
        order.sort();
        let ids: Vec<String> = order.into_iter().take(max).map(|(_, id)| id).collect();
        for id in &ids {
            lazy.want.remove(id);
            lazy.in_flight.insert(id.clone());
        }
        lazy.requested += ids.len() as u64;
        ids
    }

    /// The bytes of a requested texture are in the store.
    pub fn fetch_done(&self, id: &str) {
        let mut lazy = self.lazy.borrow_mut();
        if lazy.in_flight.remove(id) {
            lazy.loaded += 1;
        }
    }

    /// A requested texture will not come: its asks answer `None` from now on.
    pub fn fetch_failed(&self, id: &str) {
        let mut lazy = self.lazy.borrow_mut();
        lazy.in_flight.remove(id);
        lazy.failed.insert(id.to_owned());
    }

    /// Whether the texture of the renderer's id is on its way (asked for by the UI, not here yet).
    pub fn is_loading(&self, id: TextureId) -> bool {
        let Some(info) = (id.0 as usize)
            .checked_sub(1)
            .and_then(|i| self.files.index().files.get(i))
        else {
            return false;
        };
        let lazy = self.lazy.borrow();
        lazy.want.contains(&info.id) || lazy.in_flight.contains(&info.id)
    }

    pub fn lazy_stats(&self) -> LazyStats {
        let lazy = self.lazy.borrow();
        LazyStats {
            requested: lazy.requested,
            loaded: lazy.loaded,
            failed: lazy.failed.len() as u64,
            pending: (lazy.want.len() + lazy.in_flight.len()) as u64,
        }
    }

    /// The decoded texture an id stands for, if it has arrived.
    pub fn texture_blob(&self, id: TextureId) -> Option<Rc<TextureBlob>> {
        self.textures.borrow().get(&id).cloned()
    }

    /// The part of the stored `blob` of `id` that holds the image: all of it, unless the variant is
    /// padded to whole blocks (`asset_format::stored_content` against the size of the index).
    pub fn texture_content(&self, id: TextureId, blob: &TextureBlob) -> (u32, u32) {
        let stored = (blob.width, blob.height);
        let logical = (id.0 as usize)
            .checked_sub(1)
            .and_then(|i| self.files.index().files.get(i))
            .and_then(|f| self.tex_dims.get(&f.name));
        logical.map_or(stored, |&l| asset_format::stored_content(l, stored))
    }

    /// Whether the texture is a glyph atlas of a font.
    pub fn is_font_texture(&self, id: TextureId) -> bool {
        self.font_textures.contains(&id)
    }

    /// Textures that `ui` asked for by name and that are not here (lower-case bare names).
    pub fn missing_textures(&self) -> Vec<String> {
        self.missing.borrow().iter().cloned().collect()
    }

    pub fn texture_count(&self) -> usize {
        self.textures.borrow().len()
    }
}

/// `FileSource` over the store: a path of the index is a manifest id.
pub struct StoreFiles(pub Rc<UiStore>);

impl FileSource for StoreFiles {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let id = self.0.path_ids.get(path)?;
        self.0.bytes(id).map(|b| b.as_ref().clone())
    }
}
