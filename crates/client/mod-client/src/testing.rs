//! Test doubles for client mods: [`MockClientCtx`] is a headless host (no wgpu, no window) that records what mods do.
//!
//! It is the host side: capabilities are not checked by it. Tests of a mod alone wrap it in
//! `Guarded::new(&mut mock, &caps)`; tests of the dispatcher pass it as the `ClientHost`.

use std::collections::BTreeMap;

use draw_list::TextureId;
use mod_api::{Cap, ChannelId, LogLevel, MapShape, PlayerId, PlotClass, PlotIdx};
use mod_manifest::ModId;
use render::cutout::{CutoutGrid, CutoutRegion};

use crate::ctx::{
    ClientCtx, ClientHost, CtxError, CutoutMask, Hotkeys, HudTextures, ModAssets, ModAudio, ModKv,
    ModTerrain, ModUiModel, ModWorld,
};
use crate::types::{
    HotkeySpec, PackState, PlayOpts, PlayerInfo, PlotView, ScreenInfo, StrategicCamera, UiValue,
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
    /// The column grid the mock reports (`None`: no map yet; regions are not range-checked then).
    pub grid: Option<CutoutGrid>,
}

impl CutoutMask for MockCutout {
    fn grid(&self) -> Option<CutoutGrid> {
        self.grid
    }
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

/// One write of a mod into its UI v2 model fields, as [`MockUiModel`] records it.
#[derive(Debug, Clone, PartialEq)]
pub enum UiOp {
    Set {
        field: String,
        v: UiValue,
    },
    ListPut {
        list: String,
        key: i64,
        rec: Vec<(String, UiValue)>,
    },
    ListRemove {
        list: String,
        key: i64,
    },
    ListClear {
        list: String,
    },
}

/// One mod's UI v2 model fields in memory: the writes in order and the resulting values. With `declared` set, a write
/// to a field or list that is not declared is refused the way a host refuses it (`CtxError::Rejected` with the name).
#[derive(Debug, Clone, Default)]
pub struct MockUiModel {
    pub ops: Vec<UiOp>,
    pub fields: BTreeMap<String, UiValue>,
    pub lists: BTreeMap<String, BTreeMap<i64, Vec<(String, UiValue)>>>,
    /// The names writes may use (fields and lists); `None` accepts every name.
    pub declared: Option<Vec<String>>,
}

impl MockUiModel {
    /// A model that accepts only these field and list names.
    pub fn declared(names: &[&str]) -> Self {
        Self {
            declared: Some(names.iter().map(|s| (*s).to_string()).collect()),
            ..Self::default()
        }
    }

    fn check(&self, name: &str) -> Result<(), CtxError> {
        match &self.declared {
            Some(d) if !d.iter().any(|n| n == name) => Err(CtxError::Rejected(format!(
                "`{name}` is not a field of the mod's model"
            ))),
            _ => Ok(()),
        }
    }

    /// The value of a scalar field.
    pub fn get(&self, field: &str) -> Option<&UiValue> {
        self.fields.get(field)
    }

    /// The keys of a list, in key order.
    pub fn keys(&self, list: &str) -> Vec<i64> {
        self.lists
            .get(list)
            .map(|l| l.keys().copied().collect())
            .unwrap_or_default()
    }
}

impl ModUiModel for MockUiModel {
    fn set(&mut self, field: &str, v: UiValue) -> Result<(), CtxError> {
        self.check(field)?;
        self.fields.insert(field.to_string(), v.clone());
        self.ops.push(UiOp::Set {
            field: field.to_string(),
            v,
        });
        Ok(())
    }

    fn list_put(&mut self, list: &str, key: i64, rec: &[(&str, UiValue)]) -> Result<(), CtxError> {
        self.check(list)?;
        let rec: Vec<(String, UiValue)> = rec
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect();
        self.lists
            .entry(list.to_string())
            .or_default()
            .insert(key, rec.clone());
        self.ops.push(UiOp::ListPut {
            list: list.to_string(),
            key,
            rec,
        });
        Ok(())
    }

    fn list_remove(&mut self, list: &str, key: i64) -> Result<(), CtxError> {
        self.check(list)?;
        if let Some(l) = self.lists.get_mut(list) {
            l.remove(&key);
        }
        self.ops.push(UiOp::ListRemove {
            list: list.to_string(),
            key,
        });
        Ok(())
    }

    fn list_clear(&mut self, list: &str) -> Result<(), CtxError> {
        self.check(list)?;
        self.lists.remove(list);
        self.ops.push(UiOp::ListClear {
            list: list.to_string(),
        });
        Ok(())
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
    /// The plot `hover_plot` answers.
    pub hover: Option<PlotIdx>,
    /// The pose `strategic_camera` answers.
    pub strategic: Option<StrategicCamera>,
    /// Every pointer-lock request, in order.
    pub pointer_lock_requests: Vec<bool>,
    /// Who holds the camera and the input (API 1.1). Set it to another id to play a second mod in its mode:
    /// `acquire_control` then fails with `CtxError::Busy` (used when the mock is a mod's context; inside a dispatcher
    /// the dispatcher arbitrates).
    pub control_holder: Option<ModId>,
    /// The id the mock answers for the mod under test when it acquires (`mock` unless set).
    pub control_id: ModId,
    /// The UI v2 model fields of each mod, by mod index (API 1.3). A mod without an entry gets an accepting one.
    pub ui: BTreeMap<u16, MockUiModel>,
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
            hover: None,
            strategic: None,
            pointer_lock_requests: Vec::new(),
            control_holder: None,
            control_id: ModId::new("mock").unwrap_or_else(|_| unreachable!("a valid id")),
            ui: BTreeMap::new(),
        }
    }

    /// The UI v2 model fields mod `mod_idx` wrote (an empty model if it wrote none).
    pub fn ui_of(&self, mod_idx: u16) -> MockUiModel {
        self.ui.get(&mod_idx).cloned().unwrap_or_default()
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

    fn hover_plot(&self) -> Result<Option<PlotIdx>, CtxError> {
        Ok(self.hover)
    }

    fn strategic_camera(&self) -> Option<StrategicCamera> {
        self.strategic
    }

    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        if lock
            && let Some(h) = self
                .control_holder
                .as_ref()
                .filter(|h| **h != self.control_id)
        {
            return Err(CtxError::Busy { holder: h.clone() });
        }
        self.pointer_lock_requests.push(lock);
        Ok(())
    }

    fn acquire_control(&mut self) -> Result<(), CtxError> {
        match &self.control_holder {
            Some(h) if *h != self.control_id => Err(CtxError::Busy { holder: h.clone() }),
            _ => {
                self.control_holder = Some(self.control_id.clone());
                Ok(())
            }
        }
    }

    fn release_control(&mut self) {
        if self.control_holder.as_ref() == Some(&self.control_id) {
            self.control_holder = None;
        }
    }

    fn control_holder(&self) -> Option<ModId> {
        self.control_holder.clone()
    }

    fn ui_model(&mut self) -> Result<&mut dyn ModUiModel, CtxError> {
        Ok(self.ui.entry(self.current).or_default())
    }
}

#[cfg(test)]
mod tests {
    use mod_api::Caps;

    use super::*;
    use crate::ctx::Guarded;

    #[test]
    fn hover_plot_and_the_strategic_pose_reach_a_mod_and_hover_needs_world_read() {
        let mut host = MockClientCtx::new();
        host.hover = Some(9);
        host.strategic = Some(StrategicCamera {
            eye: [1.0, 2.0, 3.0],
            look_at: [1.0, 0.0, 0.0],
            yaw: 0.0,
            pitch: -0.5,
            fov_y: 1.0,
        });
        let none = Caps::none();
        let g = Guarded::new(&mut host, &none);
        assert!(g.hover_plot().is_err());
        assert!(g.strategic_camera().is_some());
        let caps = Caps::of(&[Cap::WorldRead]);
        let g = Guarded::new(&mut host, &caps);
        assert_eq!(g.hover_plot(), Ok(Some(9)));
    }
}
