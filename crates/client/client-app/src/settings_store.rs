//! Where the quality settings live and how they are chosen at start-up:
//! the ini text in the `BlobStore` under `settings/graphics.ini`, then the preset, the key
//! overrides and the switches from the URL or the command line on top.
//!
//! `RenderSettings` stay in `render`; this module only keeps their text and merges the sources.

use platform::{BlobError, BlobOp, BlobStore};
use render::{BackendPref, LayerSwitches, Preset, RenderSettings};

/// The name the ini text is kept under.
pub const SETTINGS_BLOB: &str = "settings/graphics.ini";

/// What the player (or a script) asked for on the URL or the command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GraphicsOptions {
    /// `?preset=low|medium|high`, `--preset`: replaces the stored settings.
    pub preset: Option<String>,
    /// `?gfx=ShadowLevel:0,FOWLevel:1`, `--gfx`: on top of the preset or the stored settings.
    pub overrides: Option<String>,
    /// `?disable=FOW,FluidFOW`, `--disable`: `[DisableSystems]` names.
    pub disable: Option<String>,
    /// `?debug=shadow`, `--debug`: debug views, a comma list; `shadow` shows the shadow map in
    /// place of the picture.
    pub debug: Option<String>,
}

impl GraphicsOptions {
    /// `view` is in the `debug` list (case does not matter).
    pub fn debug_has(&self, view: &str) -> bool {
        self.debug.as_deref().is_some_and(|list| {
            list.split(',')
                .any(|word| word.trim().eq_ignore_ascii_case(view))
        })
    }
}

/// The two kinds of words `?gfx=` takes: backend names and `Key:value` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GfxParam {
    pub backend: BackendPref,
    /// The `Key:value` words, joined again, for [`GraphicsOptions::overrides`].
    pub overrides: Option<String>,
    /// Words that are neither.
    pub unknown: Vec<String>,
    /// A backend word was given (`auto`, `webgpu`, `webgl2`): the page does not pick for the browser.
    pub backend_given: bool,
}

/// The backend the page asks for: the one `?gfx=` names, else WebGL2 on Firefox and the best one
/// elsewhere. Firefox's WebGPU (158, Windows) runs its GPU process out of memory while the game
/// draws: the GPU memory of the browser grows by about 1 GB in 5 s at 180 frames a second (a bare
/// canvas cleared every frame grows the same), the device reports "Out of Memory" after a minute or
/// two of play and the whole browser stops; with WebGL2 the memory stays flat. `?gfx=webgpu` (or
/// `auto`) still asks for WebGPU there.
pub fn backend_for(gfx: &GfxParam, user_agent: &str) -> BackendPref {
    if gfx.backend_given || !is_firefox(user_agent) {
        return gfx.backend;
    }
    BackendPref::WebGl2
}

/// Firefox (and the browsers built on Gecko that say so in the user agent).
pub fn is_firefox(user_agent: &str) -> bool {
    user_agent.contains("Firefox/") && !user_agent.contains("Seamonkey/")
}

/// Splits `webgl2,ShadowLevel:0` into the backend and the key overrides.
pub fn split_gfx(value: &str) -> GfxParam {
    let mut backend = BackendPref::Auto;
    let mut pairs = Vec::new();
    let mut unknown = Vec::new();
    let mut backend_given = false;
    for word in value.split(',').map(str::trim).filter(|w| !w.is_empty()) {
        if word.contains([':', '=']) {
            pairs.push(word);
        } else {
            match word.to_ascii_lowercase().as_str() {
                "auto" | "webgpu" => {
                    backend = BackendPref::Auto;
                    backend_given = true;
                }
                "gl" | "webgl" | "webgl2" => {
                    backend = BackendPref::WebGl2;
                    backend_given = true;
                }
                _ => unknown.push(word.to_owned()),
            }
        }
    }
    GfxParam {
        backend,
        overrides: (!pairs.is_empty()).then(|| pairs.join(",")),
        unknown,
        backend_given,
    }
}

/// The settings and switches in force at start, and what was wrong with the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub settings: RenderSettings,
    pub layers: LayerSwitches,
    pub warnings: Vec<String>,
}

/// Merges the sources: the stored ini (High when there is none), replaced by the preset if one was
/// given, then the key overrides, and the switches. Bad input is a warning, never an error.
pub fn resolve(stored_ini: Option<&str>, options: &GraphicsOptions) -> Resolved {
    let mut warnings = Vec::new();
    let mut settings = match stored_ini {
        Some(text) => {
            let (settings, w) = RenderSettings::from_ini(text);
            warnings.extend(w.iter().map(|w| format!("{SETTINGS_BLOB}: {w}")));
            settings
        }
        None => RenderSettings::default(),
    };
    if let Some(name) = options.preset.as_deref() {
        match Preset::from_name(name) {
            Some(p) => settings = RenderSettings::preset(p),
            None => warnings.push(format!("preset {name:?} is not low, medium or high")),
        }
    }
    if let Some(text) = options.overrides.as_deref() {
        warnings.extend(settings.apply_overrides(text));
    }
    let layers = match options.disable.as_deref() {
        Some(text) => {
            let (layers, w) = LayerSwitches::from_names(text);
            warnings.extend(w.iter().map(ToString::to_string));
            layers
        }
        None => LayerSwitches::new(),
    };
    Resolved {
        settings,
        layers,
        warnings,
    }
}

/// The ini text in the blob store: one read at start, and writes when the player changes the
/// settings. A store that is not there (`None`, or one that is unavailable) means "nothing stored";
/// settings are then not remembered.
pub struct SettingsStore {
    store: Option<Box<dyn BlobStore>>,
    load: Option<Box<dyn BlobOp<Option<Vec<u8>>>>>,
    saves: Vec<Box<dyn BlobOp<()>>>,
}

impl SettingsStore {
    /// Starts the read.
    pub fn new(store: Option<Box<dyn BlobStore>>) -> Self {
        let load = store.as_ref().map(|s| s.get_named(SETTINGS_BLOB));
        Self {
            store,
            load,
            saves: Vec::new(),
        }
    }

    /// `None` while the read is under way; then `Some(text)` once, `Some(None)` of that being
    /// "nothing stored" (also when the store failed: the reason is logged).
    pub fn poll_load(&mut self) -> Option<Option<String>> {
        let Some(op) = self.load.as_mut() else {
            return Some(None);
        };
        let result = op.poll()?;
        self.load = None;
        Some(match result {
            Ok(Some(bytes)) => match String::from_utf8(bytes) {
                Ok(text) => Some(text),
                Err(_) => {
                    tracing::warn!("{SETTINGS_BLOB} is not text, ignored");
                    None
                }
            },
            Ok(None) => None,
            Err(BlobError::Unavailable(why)) => {
                tracing::debug!("settings are not kept: {why}");
                None
            }
            Err(e) => {
                tracing::warn!("cannot read {SETTINGS_BLOB}: {e}");
                None
            }
        })
    }

    /// Writes `settings` as ini text. Fire and forget; call [`Self::poll_saves`] to log failures.
    pub fn save(&mut self, settings: &RenderSettings) {
        if let Some(store) = &self.store {
            self.saves
                .push(store.put_named(SETTINGS_BLOB, settings.to_ini().into_bytes()));
        }
    }

    /// Drops the writes that finished; a failed one is logged.
    pub fn poll_saves(&mut self) {
        self.saves.retain_mut(|op| match op.poll() {
            None => true,
            Some(Ok(())) => false,
            Some(Err(BlobError::Unavailable(why))) => {
                tracing::debug!("settings are not kept: {why}");
                false
            }
            Some(Err(e)) => {
                tracing::warn!("cannot write {SETTINGS_BLOB}: {e}");
                false
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    struct Ready<T>(Option<Result<T, BlobError>>);

    impl<T> BlobOp<T> for Ready<T> {
        fn poll(&mut self) -> Option<Result<T, BlobError>> {
            self.0.take()
        }
    }

    /// An in-memory store that answers at once; a named value is replaced on every write.
    #[derive(Clone, Default)]
    struct Memory {
        named: Rc<RefCell<HashMap<String, Vec<u8>>>>,
    }

    impl BlobStore for Memory {
        fn get(&self, _key: &platform::BlobKey) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
            Box::new(Ready(Some(Ok(None))))
        }
        fn put(&self, _key: &platform::BlobKey, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
            Box::new(Ready(Some(Ok(()))))
        }
        fn get_named(&self, name: &str) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
            Box::new(Ready(Some(Ok(self.named.borrow().get(name).cloned()))))
        }
        fn put_named(&self, name: &str, bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
            self.named.borrow_mut().insert(name.to_owned(), bytes);
            Box::new(Ready(Some(Ok(()))))
        }
    }

    struct Down;

    impl BlobStore for Down {
        fn get(&self, _key: &platform::BlobKey) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
            Box::new(Ready(Some(Err(BlobError::Unavailable("down".into())))))
        }
        fn put(&self, _key: &platform::BlobKey, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
            Box::new(Ready(Some(Err(BlobError::Unavailable("down".into())))))
        }
        fn get_named(&self, _name: &str) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
            Box::new(Ready(Some(Err(BlobError::Unavailable("down".into())))))
        }
        fn put_named(&self, _name: &str, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
            Box::new(Ready(Some(Err(BlobError::Unavailable("down".into())))))
        }
    }

    /// The old content-key `put` is not used for settings: a store that has only `put`/`get`
    /// reports "unavailable" and the settings are simply not kept.
    #[test]
    fn a_store_without_named_values_keeps_nothing() {
        struct ContentOnly;
        impl BlobStore for ContentOnly {
            fn get(&self, _key: &platform::BlobKey) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
                Box::new(Ready(Some(Ok(None))))
            }
            fn put(&self, _key: &platform::BlobKey, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
                Box::new(Ready(Some(Ok(()))))
            }
        }
        let mut s = SettingsStore::new(Some(Box::new(ContentOnly)));
        assert_eq!(s.poll_load(), Some(None));
        s.save(&RenderSettings::default());
        s.poll_saves();
        assert!(s.saves.is_empty());
    }

    /// A second save replaces the first (replace semantics of `put_named`).
    #[test]
    fn a_second_save_replaces_the_first() {
        let memory = Memory::default();
        let mut store = SettingsStore::new(Some(Box::new(memory.clone())));
        store.save(&RenderSettings::preset(Preset::Low));
        store.save(&RenderSettings::preset(Preset::Medium));
        let text = String::from_utf8(memory.named.borrow()[SETTINGS_BLOB].clone()).unwrap();
        assert_eq!(
            RenderSettings::from_ini(&text).0,
            RenderSettings::preset(Preset::Medium)
        );
    }

    #[test]
    fn settings_survive_a_save_and_a_load() {
        let memory = Memory::default();
        let mut first = SettingsStore::new(Some(Box::new(memory.clone())));
        assert_eq!(first.poll_load(), Some(None), "nothing stored yet");
        let mut low = RenderSettings::preset(Preset::Low);
        low.set("TerrainDetailLevel", 0).unwrap();
        first.save(&low);
        first.poll_saves();
        assert!(first.saves.is_empty());

        let mut second = SettingsStore::new(Some(Box::new(memory)));
        let text = second.poll_load().unwrap().expect("the text is back");
        assert_eq!(RenderSettings::from_ini(&text).0, low);
        // Read once: the next poll says "nothing under way".
        assert_eq!(second.poll_load(), Some(None));
    }

    #[test]
    fn a_missing_or_failing_store_means_nothing_stored() {
        assert_eq!(SettingsStore::new(None).poll_load(), Some(None));
        let mut down = SettingsStore::new(Some(Box::new(Down)));
        assert_eq!(down.poll_load(), Some(None));
        down.save(&RenderSettings::default());
        down.poll_saves();
        assert!(down.saves.is_empty());
    }

    #[test]
    fn a_slow_store_is_waited_for() {
        struct Slow(Option<Result<Option<Vec<u8>>, BlobError>>, u32);
        impl BlobOp<Option<Vec<u8>>> for Slow {
            fn poll(&mut self) -> Option<Result<Option<Vec<u8>>, BlobError>> {
                if self.1 > 0 {
                    self.1 -= 1;
                    return None;
                }
                self.0.take()
            }
        }
        struct SlowStore;
        impl BlobStore for SlowStore {
            fn get(&self, _key: &platform::BlobKey) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
                Box::new(Ready(Some(Ok(None))))
            }
            fn put(&self, _key: &platform::BlobKey, _bytes: Vec<u8>) -> Box<dyn BlobOp<()>> {
                Box::new(Ready(Some(Ok(()))))
            }
            fn get_named(&self, _name: &str) -> Box<dyn BlobOp<Option<Vec<u8>>>> {
                Box::new(Slow(
                    Some(Ok(Some(b"[UserSettings]\nMSAASamples = 1\n".to_vec()))),
                    2,
                ))
            }
        }
        let mut store = SettingsStore::new(Some(Box::new(SlowStore)));
        assert_eq!(store.poll_load(), None);
        assert_eq!(store.poll_load(), None);
        let text = store.poll_load().unwrap().unwrap();
        assert_eq!(RenderSettings::from_ini(&text).0.msaa_samples, 1);
    }

    fn options(preset: Option<&str>, gfx: Option<&str>, disable: Option<&str>) -> GraphicsOptions {
        GraphicsOptions {
            preset: preset.map(str::to_owned),
            overrides: gfx.map(str::to_owned),
            disable: disable.map(str::to_owned),
            debug: None,
        }
    }

    #[test]
    fn the_debug_list_names_views() {
        let o = GraphicsOptions {
            debug: Some("fog, Shadow".into()),
            ..GraphicsOptions::default()
        };
        assert!(o.debug_has("shadow") && o.debug_has("fog"));
        assert!(!o.debug_has("sha"));
        assert!(!GraphicsOptions::default().debug_has("shadow"));
    }

    #[test]
    fn nothing_given_is_high_and_nothing_off() {
        let r = resolve(None, &GraphicsOptions::default());
        assert_eq!(r.settings, RenderSettings::preset(Preset::High));
        assert!(r.layers.is_empty() && r.warnings.is_empty());
    }

    #[test]
    fn the_preset_replaces_the_stored_ini_and_the_overrides_go_on_top() {
        let stored = RenderSettings::preset(Preset::Medium).to_ini();
        // Stored alone.
        let r = resolve(Some(&stored), &GraphicsOptions::default());
        assert_eq!(r.settings, RenderSettings::preset(Preset::Medium));
        // The preset of the URL wins over what is stored; `gfx` is on top of it.
        let r = resolve(
            Some(&stored),
            &options(Some("LOW"), Some("FOWLevel:3,MSAASamples:4"), None),
        );
        let mut want = RenderSettings::preset(Preset::Low);
        want.set("FOWLevel", 3).unwrap();
        want.set("MSAASamples", 4).unwrap();
        assert_eq!(r.settings, want);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        // Overrides alone sit on the stored settings.
        let r = resolve(Some(&stored), &options(None, Some("ShadowLevel:0"), None));
        assert_eq!(r.settings.shadow_level, 0);
        assert_eq!(r.settings.fow_level, 2);
    }

    #[test]
    fn bad_input_warns_and_keeps_going() {
        let r = resolve(
            Some("[GraphicsDetailSettings]\nShadowLevel = 9\n"),
            &options(Some("ultra"), Some("Nope:1"), Some("Bogus,Unit")),
        );
        assert_eq!(r.settings, RenderSettings::default());
        assert_eq!(r.warnings.len(), 4, "{:?}", r.warnings);
        assert!(r.layers.is_disabled(render::frame::SystemId::Unit));
    }

    #[test]
    fn gfx_splits_into_backend_and_overrides() {
        let p = split_gfx("webgl2");
        assert_eq!(p.backend, BackendPref::WebGl2);
        assert!(p.overrides.is_none() && p.unknown.is_empty());
        let p = split_gfx("gl, ShadowLevel:0,FOWLevel=1");
        assert_eq!(p.backend, BackendPref::WebGl2);
        assert_eq!(p.overrides.as_deref(), Some("ShadowLevel:0,FOWLevel=1"));
        let p = split_gfx("");
        assert_eq!(p.backend, BackendPref::Auto);
        let p = split_gfx("vulkan");
        assert_eq!(p.unknown, ["vulkan"]);
        assert_eq!(split_gfx("auto").backend, BackendPref::Auto);
    }

    #[test]
    fn firefox_gets_webgl2_unless_the_url_names_a_backend() {
        const FIREFOX: &str =
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:158.0) Gecko/20100101 Firefox/158.0";
        const CHROME: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                              (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
        assert!(is_firefox(FIREFOX) && !is_firefox(CHROME));
        let none = split_gfx("ShadowLevel:0");
        assert!(!none.backend_given);
        assert_eq!(backend_for(&none, FIREFOX), BackendPref::WebGl2);
        assert_eq!(backend_for(&none, CHROME), BackendPref::Auto);
        // `?gfx=webgpu` (or `auto`) keeps WebGPU on Firefox; `webgl2` is WebGL2 everywhere.
        assert_eq!(
            backend_for(&split_gfx("webgpu"), FIREFOX),
            BackendPref::Auto
        );
        assert_eq!(backend_for(&split_gfx("auto"), FIREFOX), BackendPref::Auto);
        assert_eq!(
            backend_for(&split_gfx("webgl2"), CHROME),
            BackendPref::WebGl2
        );
    }
}
