//! Test doubles for client mods: [`MockClientCtx`] is a headless host (no wgpu, no window) that records what mods do.
//!
//! It is the host side: capabilities are not checked by it. Tests of a mod alone wrap it in
//! `Guarded::new(&mut mock, &caps)`; tests of the dispatcher pass it as the `ClientHost`.

use std::collections::BTreeMap;

use draw_list::TextureId;
use mod_api::{Cap, ChannelId, LogLevel, MapShape, PlayerId, PlotClass, PlotIdx};

use crate::ctx::{
    ClientCtx, ClientHost, CtxError, CutoutMask, Hotkeys, HudTextures, ModAssets, ModAudio, ModKv,
    ModTerrain, ModWorld,
};
use crate::types::{
    CutoutRegion, HotkeySpec, PackState, PlayOpts, PlayerInfo, PlotView, ScreenInfo,
};

/// A message `send` recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentMsg {
    pub mod_idx: u16,
    pub ch: ChannelId,
    pub payload: Vec<u8>,
}

/// A replica of a small map. Plots start unrevealed.
#[derive(Debug, Clone)]
pub struct MockWorld {
    shape: MapShape,
    me: Option<PlayerId>,
    turn: i32,
    revision: u64,
    players: Vec<PlayerInfo>,
    plots: Vec<PlotView>,
}

impl MockWorld {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            shape: MapShape {
                width,
                height,
                wrap_x: true,
                wrap_y: false,
            },
            me: None,
            turn: 0,
            revision: 1,
            players: Vec::new(),
            plots: vec![PlotView::UNKNOWN; usize::from(width) * usize::from(height)],
        }
    }

    pub fn set_me(&mut self, me: PlayerId) -> &mut Self {
        self.me = Some(me);
        self.revision += 1;
        self
    }

    pub fn set_turn(&mut self, turn: i32) -> &mut Self {
        self.turn = turn;
        self.revision += 1;
        self
    }

    pub fn add_player(&mut self, id: PlayerId, nick: &str) -> &mut Self {
        self.players.push(PlayerInfo {
            id,
            nick: nick.to_string(),
            color: [200, 200, 200],
        });
        self
    }

    /// Reveals the plot at `(x, y)` with a class and an owner (visible).
    pub fn reveal(
        &mut self,
        x: i16,
        y: i16,
        class: PlotClass,
        owner: Option<PlayerId>,
    ) -> &mut Self {
        if let Some(p) = self.shape.index(x, y) {
            self.plots[p as usize] = PlotView {
                owner,
                class: Some(class),
                revealed: true,
                visible: true,
            };
            self.revision += 1;
        }
        self
    }

    /// Changes the owner of a revealed plot (a border change).
    pub fn set_owner(&mut self, x: i16, y: i16, owner: Option<PlayerId>) -> &mut Self {
        if let Some(p) = self.shape.index(x, y) {
            self.plots[p as usize].owner = owner;
            self.revision += 1;
        }
        self
    }
}

impl ModWorld for MockWorld {
    fn me(&self) -> Option<PlayerId> {
        self.me
    }
    fn map(&self) -> MapShape {
        self.shape
    }
    fn turn(&self) -> i32 {
        self.turn
    }
    fn revision(&self) -> u64 {
        self.revision
    }
    fn players(&self) -> Vec<PlayerInfo> {
        self.players.clone()
    }
    fn plot_at(&self, x: i16, y: i16) -> Option<PlotIdx> {
        self.shape.index(x, y)
    }
    fn plot(&self, p: PlotIdx) -> PlotView {
        self.plots
            .get(p as usize)
            .copied()
            .unwrap_or(PlotView::UNKNOWN)
    }
}

/// Terrain: a plane `h = base + slope_x * x`, built everywhere, with a hex size of 1 world unit for `hex_at`.
#[derive(Debug, Clone, Default)]
pub struct MockTerrain {
    pub base: f32,
    pub slope_x: f32,
    pub revision: u64,
}

impl ModTerrain for MockTerrain {
    fn height(&self, x: f32, _z: f32) -> Option<f32> {
        Some(self.base + self.slope_x * x)
    }
    fn height_nearest(&self, x: f32, z: f32) -> Option<f32> {
        self.height(x.round(), z.round())
    }
    fn height_revision(&self) -> u64 {
        self.revision
    }
    fn hex_at(&self, x: f32, z: f32) -> Option<(i16, i16)> {
        Some((x.floor() as i16, z.floor() as i16))
    }
    fn is_built(&self, _p: PlotIdx) -> bool {
        true
    }
}

/// One mod's key-value namespace in memory.
#[derive(Debug, Clone, Default)]
pub struct MockKv(pub BTreeMap<String, String>);

impl ModKv for MockKv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
    fn set(&mut self, key: &str, value: &str) {
        self.0.insert(key.to_string(), value.to_string());
    }
    fn remove(&mut self, key: &str) {
        self.0.remove(key);
    }
}

/// Registered hotkeys; a chord can be taken by the game (`reserve`) or by another binding.
#[derive(Debug, Clone, Default)]
pub struct MockHotkeys {
    current: u16,
    pub registered: Vec<(u16, HotkeySpec)>,
    reserved: Vec<String>,
}

impl MockHotkeys {
    /// Marks a key code as used by the game: registering it fails.
    pub fn reserve(&mut self, code: &str) {
        self.reserved.push(code.to_string());
    }

    /// The `(mod_idx, id)` bound to the chord, if any.
    pub fn lookup(&self, code: &str, ctrl: bool, shift: bool, alt: bool) -> Option<(u16, &str)> {
        self.registered
            .iter()
            .find(|(_, h)| h.code == code && h.ctrl == ctrl && h.shift == shift && h.alt == alt)
            .map(|(m, h)| (*m, h.id.as_str()))
    }
}

impl Hotkeys for MockHotkeys {
    fn register(&mut self, spec: HotkeySpec) -> Result<(), CtxError> {
        let m = self.current;
        if self
            .registered
            .iter()
            .any(|(o, h)| *o == m && h.id == spec.id)
        {
            return Err(CtxError::Rejected(format!(
                "hotkey id `{}` is already registered",
                spec.id
            )));
        }
        let plain = !spec.ctrl && !spec.shift && !spec.alt;
        if plain && self.reserved.contains(&spec.code) {
            return Err(CtxError::Rejected(format!(
                "`{}` is used by the game",
                spec.code
            )));
        }
        if self
            .lookup(&spec.code, spec.ctrl, spec.shift, spec.alt)
            .is_some()
        {
            return Err(CtxError::Rejected(format!(
                "`{}` is already bound",
                spec.code
            )));
        }
        self.registered.push((m, spec));
        Ok(())
    }

    fn unregister(&mut self, id: &str) {
        let m = self.current;
        self.registered.retain(|(o, h)| !(*o == m && h.id == id));
    }
}

/// Records the sounds played.
#[derive(Debug, Clone, Default)]
pub struct MockAudio {
    pub played: Vec<(String, PlayOpts)>,
    pub strategic_muted: bool,
}

impl ModAudio for MockAudio {
    fn play(&mut self, sound: &str, opts: PlayOpts) -> Result<(), CtxError> {
        self.played.push((sound.to_string(), opts));
        Ok(())
    }
    fn mute_strategic(&mut self, mute: bool) {
        self.strategic_muted = mute;
    }
}

/// An asset pack in memory.
#[derive(Debug, Clone)]
pub struct MockAssets {
    pub state: PackState,
    pub files: BTreeMap<String, Vec<u8>>,
}

impl ModAssets for MockAssets {
    fn state(&self) -> PackState {
        self.state.clone()
    }
    fn get(&self, id: &str) -> Option<&[u8]> {
        if self.state != PackState::Ready {
            return None;
        }
        self.files.get(id).map(Vec::as_slice)
    }
}

/// Records HUD texture uploads as `(width, height)`.
#[derive(Debug, Clone, Default)]
pub struct MockHud {
    pub uploads: Vec<(u32, u32)>,
}

impl HudTextures for MockHud {
    fn upload_rgba8(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<TextureId, CtxError> {
        if rgba.len() != width as usize * height as usize * 4 {
            return Err(CtxError::Rejected(
                "pixel data does not match the size".into(),
            ));
        }
        self.uploads.push((width, height));
        Ok(TextureId(1000 + self.uploads.len() as u64))
    }
}

/// The recorded cutout writes.
#[derive(Debug, Clone, Default)]
pub struct MockCutout {
    pub writes: Vec<(CutoutRegion, Vec<u8>)>,
    pub cleared: u32,
}

impl CutoutMask for MockCutout {
    fn set_cutout_mask(&mut self, region: CutoutRegion, bytes: &[u8]) -> Result<(), CtxError> {
        if bytes.len() != region.byte_len() {
            return Err(CtxError::Rejected(format!(
                "mask of {} bytes for a region of {}",
                bytes.len(),
                region.byte_len()
            )));
        }
        self.writes.push((region, bytes.to_vec()));
        Ok(())
    }
    fn clear_cutout(&mut self) {
        self.cleared += 1;
    }
}

/// A headless client host. All parts are public: set up the world, run the code under test, read what was recorded.
pub struct MockClientCtx {
    /// Index of the mod the dispatcher is currently calling (set by `ClientHost::ctx`).
    pub current: u16,
    pub sent: Vec<SentMsg>,
    pub logs: Vec<(LogLevel, String)>,
    pub world: MockWorld,
    pub terrain: MockTerrain,
    pub kv: BTreeMap<u16, MockKv>,
    pub hotkeys: MockHotkeys,
    pub audio: MockAudio,
    pub assets: MockAssets,
    pub hud: MockHud,
    pub cutout: MockCutout,
    pub screen: ScreenInfo,
    /// Every pointer-lock request, in order.
    pub pointer_lock_requests: Vec<bool>,
}

impl MockClientCtx {
    pub fn new() -> Self {
        Self {
            current: 0,
            sent: Vec::new(),
            logs: Vec::new(),
            world: MockWorld::new(8, 8),
            terrain: MockTerrain::default(),
            kv: BTreeMap::new(),
            hotkeys: MockHotkeys::default(),
            audio: MockAudio::default(),
            assets: MockAssets {
                state: PackState::Missing,
                files: BTreeMap::new(),
            },
            hud: MockHud::default(),
            cutout: MockCutout::default(),
            screen: ScreenInfo {
                width: 1280,
                height: 720,
                scale: 1.0,
            },
            pointer_lock_requests: Vec::new(),
        }
    }

    /// Makes the pack available with these files.
    pub fn with_pack(mut self, files: &[(&str, &[u8])]) -> Self {
        self.assets.state = PackState::Ready;
        self.assets.files = files
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.to_vec()))
            .collect();
        self
    }

    /// The payloads sent by mod `mod_idx` on `ch`.
    pub fn sent_on(&self, mod_idx: u16, ch: ChannelId) -> Vec<&[u8]> {
        self.sent
            .iter()
            .filter(|m| m.mod_idx == mod_idx && m.ch == ch)
            .map(|m| m.payload.as_slice())
            .collect()
    }

    /// Whether any logged line contains `needle`.
    pub fn logged(&self, needle: &str) -> bool {
        self.logs.iter().any(|(_, m)| m.contains(needle))
    }
}

impl Default for MockClientCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl ClientHost for MockClientCtx {
    fn ctx(&mut self, mod_idx: u16) -> &mut dyn ClientCtx {
        self.current = mod_idx;
        self.hotkeys.current = mod_idx;
        self
    }
}

impl ClientCtx for MockClientCtx {
    fn has(&self, _cap: Cap) -> bool {
        true
    }

    fn log(&mut self, level: LogLevel, msg: &str) {
        self.logs.push((level, msg.to_string()));
    }

    fn send(&mut self, ch: ChannelId, payload: &[u8]) -> Result<(), CtxError> {
        self.sent.push(SentMsg {
            mod_idx: self.current,
            ch,
            payload: payload.to_vec(),
        });
        Ok(())
    }

    fn world(&self) -> Result<&dyn ModWorld, CtxError> {
        Ok(&self.world)
    }

    fn terrain(&self) -> Result<&dyn ModTerrain, CtxError> {
        Ok(&self.terrain)
    }

    fn cutout(&mut self) -> Result<&mut dyn CutoutMask, CtxError> {
        Ok(&mut self.cutout)
    }

    fn kv(&mut self) -> Result<&mut dyn ModKv, CtxError> {
        Ok(self.kv.entry(self.current).or_default())
    }

    fn hotkeys(&mut self) -> Result<&mut dyn Hotkeys, CtxError> {
        self.hotkeys.current = self.current;
        Ok(&mut self.hotkeys)
    }

    fn audio(&mut self) -> Result<&mut dyn ModAudio, CtxError> {
        Ok(&mut self.audio)
    }

    fn assets(&self) -> Result<&dyn ModAssets, CtxError> {
        Ok(&self.assets)
    }

    fn hud_textures(&mut self) -> Result<&mut dyn HudTextures, CtxError> {
        Ok(&mut self.hud)
    }

    fn screen(&self) -> ScreenInfo {
        self.screen
    }

    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        self.pointer_lock_requests.push(lock);
        Ok(())
    }
}
