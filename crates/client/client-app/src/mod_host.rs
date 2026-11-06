//! The host the client mods run against in the app (T36): the services `mod_client::ClientCtx` hands out, over the
//! engine's own state.
//!
//! [`HostState`] lives as long as the hook; [`HostView`] is made for one call into the dispatcher and implements
//! `ClientHost` and `ClientCtx` over it plus what that call can see (the replica, the terrain sampler, the renderer's
//! `ModGpu`). It is unchecked: the capabilities are enforced by `mod_client::Guarded`, which the dispatcher puts around
//! every context it hands to a mod.
//!
//! Most services are queues. A mod's call cannot reach the renderer or the sound output (they are borrowed by the
//! frame), so it records a value (`CutoutCmd`, a texture, a `SoundReq`, a render system) that the hook moves into the
//! `HookCtl`, and the app applies it in its frame. What a call must answer at once is checked against the last known
//! state: a cutout region against the grid of the map, a sound against the mod's pack.
//!
//! What a call sees: `world()`, `terrain()` and `gpu()` answer only inside `frame` and `hud` (the hook is given those
//! borrows there); `attach`, `detach`, `on_msg`, `on_hotkey` and `claim_input` get `Unavailable`. A mod keeps what it
//! needs from the world in its own state and reads it in `frame`.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use client_core::NetHost;
use draw_list::TextureId;
use glam::Vec2;
use mod_api::{Cap, LogLevel, MapShape, PlayerId, PlotClass, PlotIdx};
use mod_assets::Pack;
use mod_client::{
    ClientCtx, ClientHost, CtxError, CutoutGrid, CutoutMask, CutoutRegion, HotkeyTable, Hotkeys,
    HudTextures, ModAssets, ModAudio, ModKv, ModTerrain, ModWorld, NamespacedKv, PlayOpts,
    PlayerInfo, PlotView, ScreenInfo,
};
use mod_manifest::ModId;
use platform::KeyValueStore;
use protocol::{ModSetView, PlotFlags, PlotType, WorldView};
use render::asset_format::TextureFormat;
use render::frame::{
    DrawContext, EncoderContext, InitContext, MaskContext, ModGpu, PassId, PassSlot,
    PrepareContext, RenderSystem, SystemId,
};
use render::world::terrain::TerrainSampler;
use render::{KeySet, RenderSettings, UiTextureData};

use crate::mod_assets::PackSet;
use crate::mod_hooks::{CutoutCmd, SoundReq};

// ---------------------------------------------------------------------------------------------------------------
// W1: the replica

/// The replica of the game as the player sees it (the view the controls take), for `ModWorld`.
pub struct ReplicaWorld<'a> {
    view: &'a WorldView,
    revision: u64,
}

impl<'a> ReplicaWorld<'a> {
    /// `revision` is the core's (`ClientCore::revision`): it grows with every batch that changed the view, so a mod's
    /// cache keyed on it is never stale (it may be refreshed more often than the plots change).
    pub fn new(view: &'a WorldView, revision: u64) -> Self {
        Self { view, revision }
    }
}

impl ModWorld for ReplicaWorld<'_> {
    fn me(&self) -> Option<PlayerId> {
        Some(self.view.human)
    }

    fn map(&self) -> MapShape {
        let m = &self.view.map;
        MapShape {
            width: m.width,
            height: m.height,
            wrap_x: m.wrap_x,
            wrap_y: m.wrap_y,
        }
    }

    fn turn(&self) -> i32 {
        self.view.turn
    }

    fn revision(&self) -> u64 {
        self.revision
    }

    fn players(&self) -> Vec<PlayerInfo> {
        self.view
            .players
            .iter()
            .map(|p| {
                let [r, g, b, _] = p.color_primary.to_be_bytes();
                PlayerInfo {
                    id: p.id,
                    nick: p.name.clone(),
                    color: [r, g, b],
                }
            })
            .collect()
    }

    fn plot_at(&self, x: i16, y: i16) -> Option<PlotIdx> {
        self.map().index(x, y)
    }

    fn plot(&self, p: PlotIdx) -> PlotView {
        let Some(pv) = self.view.map.plots.get(p as usize) else {
            return PlotView::UNKNOWN;
        };
        let revealed = pv.flags.contains(PlotFlags::REVEALED);
        if !revealed {
            // The protocol carries the relief of unrevealed plots too; a mod must not mistake "not seen" for "sea".
            return PlotView::UNKNOWN;
        }
        PlotView {
            owner: (pv.owner >= 0).then_some(pv.owner),
            class: Some(match pv.plot_type {
                PlotType::Land => PlotClass::Land,
                PlotType::Hill => PlotClass::Hills,
                PlotType::Mountain => PlotClass::Mountain,
                PlotType::Water if pv.flags.contains(PlotFlags::IS_LAKE) => PlotClass::Lake,
                PlotType::Water => PlotClass::Water,
            }),
            revealed: true,
            visible: pv.flags.contains(PlotFlags::VISIBLE),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// W2: the ground

/// The renderer's height field for `ModTerrain`. Heights are in the renderer's world units; the field has one texel per
/// grid cell and the nearest-texel value is what is drawn.
pub struct TerrainView<'a>(pub &'a TerrainSampler);

impl ModTerrain for TerrainView<'_> {
    /// Bilinear between the four texels around the point (texel centres at half integers). Outside the field the
    /// sampler answers 0 for a texel, so the edge fades to 0; mods stay on the map.
    fn height(&self, x: f32, z: f32) -> Option<f32> {
        let t = self.0.frame().world_to_tex(Vec2::new(x, z)) - Vec2::splat(0.5);
        let (i, j) = (t.x.floor(), t.y.floor());
        let (fx, fy) = (t.x - i, t.y - j);
        let (i, j) = (i as i32, j as i32);
        let h = |u, v| self.0.texel_height(u, v);
        let top = h(i, j) * (1.0 - fx) + h(i + 1, j) * fx;
        let bottom = h(i, j + 1) * (1.0 - fx) + h(i + 1, j + 1) * fx;
        Some(top * (1.0 - fy) + bottom * fy)
    }

    fn height_nearest(&self, x: f32, z: f32) -> Option<f32> {
        Some(self.0.world_height(Vec2::new(x, z)))
    }

    fn height_revision(&self) -> u64 {
        self.0.height_revision()
    }

    fn hex_at(&self, x: f32, z: f32) -> Option<(i16, i16)> {
        let (hx, hy) = self.0.hex_at(Vec2::new(x, z))?;
        Some((i16::try_from(hx).ok()?, i16::try_from(hy).ok()?))
    }

    /// The heights of the whole map are there once the sampler is (the renderer streams the pages, not the field).
    fn is_built(&self, _p: PlotIdx) -> bool {
        true
    }
}

// ---------------------------------------------------------------------------------------------------------------
// K1: keys and values

/// The per-mod namespaces over the player's settings store (the device store the nick and the session live in).
pub struct KvSet {
    store: Rc<dyn KeyValueStore>,
    mods: BTreeMap<u16, NamespacedKv>,
    cur: u16,
}

impl KvSet {
    pub fn new(store: Rc<dyn KeyValueStore>) -> Self {
        Self {
            store,
            mods: BTreeMap::new(),
            cur: 0,
        }
    }

    /// Opens the namespaces of the mods of a game's set.
    pub fn start(&mut self, set: &ModSetView) {
        self.mods.clear();
        for m in set.mods.iter().filter(|m| m.available) {
            if let Ok(id) = ModId::new(&m.id) {
                self.mods
                    .insert(m.load_idx, NamespacedKv::new(Rc::clone(&self.store), &id));
            }
        }
    }

    pub fn select(&mut self, mod_idx: u16) {
        self.cur = mod_idx;
    }
}

impl ModKv for KvSet {
    fn get(&self, key: &str) -> Option<String> {
        self.mods.get(&self.cur)?.get(key)
    }

    fn set(&mut self, key: &str, value: &str) {
        if let Some(k) = self.mods.get_mut(&self.cur) {
            k.set(key, value);
        }
    }

    fn remove(&mut self, key: &str) {
        if let Some(k) = self.mods.get_mut(&self.cur) {
            k.remove(key);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// D13: the cutout mask

/// Cutout writes of the mods. The mask is one texture of the renderer, shared: a column cut by any mod is cut. Each
/// mod's regions are remembered so that `clear_cutout` takes back exactly its own, and the texture is freed when the
/// last mod has cleared.
#[derive(Default)]
pub struct CutoutQueue {
    cur: u16,
    /// The grid of the last frame that had a map.
    pub grid: Option<CutoutGrid>,
    owners: BTreeMap<u16, Vec<CutoutRegion>>,
    queue: Vec<CutoutCmd>,
}

impl CutoutQueue {
    pub fn select(&mut self, mod_idx: u16) {
        self.cur = mod_idx;
    }

    pub fn take(&mut self) -> Vec<CutoutCmd> {
        std::mem::take(&mut self.queue)
    }

    /// A mod is gone (detached, or the game ended): its cutouts are taken back.
    pub fn release_mod(&mut self, mod_idx: u16) {
        let saved = self.cur;
        self.cur = mod_idx;
        self.clear_cutout();
        self.cur = saved;
    }

    fn check(&self, region: CutoutRegion, len: usize) -> Result<(), CtxError> {
        if len != region.byte_len() {
            return Err(CtxError::Rejected(format!(
                "a mask of {len} bytes for a region of {}",
                region.byte_len()
            )));
        }
        let grid = self
            .grid
            .ok_or(CtxError::Unavailable("cutout (no map yet)"))?;
        let rows_end = i64::from(grid.bz0) + i64::from(grid.rows);
        let inside = region.width <= grid.cols
            && i64::from(region.z0) >= i64::from(grid.bz0)
            && i64::from(region.z0) + i64::from(region.height) <= rows_end;
        if inside {
            Ok(())
        } else {
            Err(CtxError::Rejected(format!(
                "the region is outside the {} x {} columns from block row {}",
                grid.cols, grid.rows, grid.bz0
            )))
        }
    }
}

impl CutoutMask for CutoutQueue {
    fn grid(&self) -> Option<CutoutGrid> {
        self.grid
    }

    fn set_cutout_mask(&mut self, region: CutoutRegion, bytes: &[u8]) -> Result<(), CtxError> {
        self.check(region, bytes.len())?;
        let mine = self.owners.entry(self.cur).or_default();
        if !mine.contains(&region) {
            mine.push(region);
        }
        self.queue.push(CutoutCmd::Set(region, bytes.to_vec()));
        Ok(())
    }

    fn clear_cutout(&mut self) {
        let Some(regions) = self.owners.remove(&self.cur) else {
            return;
        };
        if self.owners.is_empty() {
            // Nobody else cuts: the texture goes (and the terrain is drawn whole, as without the hook).
            self.queue.push(CutoutCmd::Clear);
            return;
        }
        for r in regions {
            self.queue.push(CutoutCmd::Set(r, vec![0; r.byte_len()]));
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// U1: HUD textures

/// Ids of mod textures start here: far from the asset ids of the Lua UI, below the renderer's own marker bit.
const MOD_TEXTURE_BASE: u64 = 0x4D4F_4400_0000_0000;
/// Most textures a mod may upload in a game, and the longest side.
const HUD_TEXTURES_MAX: u32 = 64;
const HUD_SIDE_MAX: u32 = 4096;

#[derive(Default)]
pub struct HudQueue {
    cur: u16,
    next: u64,
    count: BTreeMap<u16, u32>,
    pending: Vec<(TextureId, UiTextureData)>,
}

impl HudQueue {
    pub fn select(&mut self, mod_idx: u16) {
        self.cur = mod_idx;
    }

    pub fn take(&mut self) -> Vec<(TextureId, UiTextureData)> {
        std::mem::take(&mut self.pending)
    }

    pub fn clear(&mut self) {
        self.count.clear();
    }
}

impl HudTextures for HudQueue {
    fn upload_rgba8(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<TextureId, CtxError> {
        if width == 0
            || height == 0
            || width > HUD_SIDE_MAX
            || height > HUD_SIDE_MAX
            || rgba.len() != width as usize * height as usize * 4
        {
            return Err(CtxError::Rejected(format!(
                "a {width}x{height} texture needs {} bytes of RGBA8 and sides of 1 to {HUD_SIDE_MAX}",
                width as usize * height as usize * 4
            )));
        }
        let n = self.count.entry(self.cur).or_default();
        if *n >= HUD_TEXTURES_MAX {
            return Err(CtxError::Rejected(format!(
                "at most {HUD_TEXTURES_MAX} HUD textures per game"
            )));
        }
        *n += 1;
        self.next += 1;
        let id = TextureId(MOD_TEXTURE_BASE + self.next);
        self.pending.push((
            id,
            UiTextureData {
                width,
                height,
                format: TextureFormat::Rgba8,
                mips: vec![rgba.to_vec()],
                content: None,
            },
        ));
        Ok(id)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// A2: sound

/// Sounds of the mods, waiting for the sound output. Sounds are the WAV files of the mod's pack.
#[derive(Default)]
pub struct AudioQueue {
    cur: u16,
    /// The pack of the mod being called (set by the view for each call).
    pack: Option<Rc<Pack>>,
    queue: Vec<SoundReq>,
    mute: Option<bool>,
}

impl AudioQueue {
    pub fn take(&mut self) -> (Vec<SoundReq>, Option<bool>) {
        (std::mem::take(&mut self.queue), self.mute.take())
    }
}

/// Most sounds waiting at once; a mod that plays in a loop must not grow the queue without end.
const SOUNDS_MAX: usize = 64;

impl ModAudio for AudioQueue {
    fn play(&mut self, sound: &str, opts: PlayOpts) -> Result<(), CtxError> {
        let pack = self
            .pack
            .as_ref()
            .ok_or(CtxError::Unavailable("audio (no asset pack)"))?;
        if pack.get(sound).is_none() {
            return Err(CtxError::Rejected(format!(
                "no sound `{sound}` in the pack of the mod"
            )));
        }
        if self.queue.len() >= SOUNDS_MAX {
            return Err(CtxError::Rejected("too many sounds at once".into()));
        }
        // SIMPLIFIED: `opts.pos` is accepted and the sound plays flat (no distance or pan) until positional mixing.
        self.queue.push(SoundReq {
            key: format!("{}:{sound}", self.cur),
            pack: Rc::clone(pack),
            sound: sound.to_owned(),
            volume: opts.volume.clamp(0.0, 4.0),
            pitch: opts.pitch.clamp(0.25, 4.0),
        });
        Ok(())
    }

    fn mute_strategic(&mut self, mute: bool) {
        self.mute = Some(mute);
    }
}

// ---------------------------------------------------------------------------------------------------------------
// R1: the render systems of mods

/// A render system of a mod as the renderer registers it: the mod's own system under an id the host chose (two mods
/// may both say `SystemId::Mod(0)`), switched off for good when the game ends. The renderer has no way to remove a
/// system, so a finished game's systems stay registered and draw nothing.
pub struct ModSystem {
    inner: Box<dyn RenderSystem>,
    id: SystemId,
    alive: Rc<Cell<bool>>,
}

impl ModSystem {
    pub fn new(inner: Box<dyn RenderSystem>, number: u16, alive: Rc<Cell<bool>>) -> Self {
        Self {
            inner,
            id: SystemId::Mod(number),
            alive,
        }
    }
}

impl RenderSystem for ModSystem {
    fn id(&self) -> SystemId {
        self.id
    }

    fn passes(&self) -> &'static [PassSlot] {
        self.inner.passes()
    }

    fn init(&mut self, cx: &mut InitContext<'_>) {
        self.inner.init(cx);
    }

    fn on_settings(
        &mut self,
        settings: &RenderSettings,
        changed: &KeySet,
        cx: &mut InitContext<'_>,
    ) {
        self.inner.on_settings(settings, changed, cx);
    }

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        if self.alive.get() {
            self.inner.prepare(cx);
        }
    }

    fn record(&self, slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if self.alive.get() {
            self.inner.record(slot, cx);
        }
    }

    fn record_offscreen(&mut self, pass: PassId, cx: &mut EncoderContext<'_>) {
        if self.alive.get() {
            self.inner.record_offscreen(pass, cx);
        }
    }

    fn record_mask(&self, slot: PassSlot, cx: &mut MaskContext<'_, '_>) {
        if self.alive.get() {
            self.inner.record_mask(slot, cx);
        }
    }

    fn pending_work(&self) -> u32 {
        self.inner.pending_work()
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        self.inner.counters()
    }
}

// ---------------------------------------------------------------------------------------------------------------
// The host

/// What outlives a call.
pub struct HostState {
    pub net: NetHost,
    /// `None` until the app told the hook where the server is (`ModHook::bind`).
    pub packs: Option<PackSet>,
    pub kv: Option<KvSet>,
    pub hotkeys: HotkeyTable,
    pub audio: AudioQueue,
    pub cutout: CutoutQueue,
    pub hud: HudQueue,
    /// Cleared when the game ends: the render systems registered for it stop drawing.
    pub alive: Rc<Cell<bool>>,
    /// The number the next render system of a mod gets (`SystemId::Mod`).
    pub next_system: u16,
}

impl HostState {
    pub fn new() -> Self {
        Self {
            net: NetHost::new(),
            packs: None,
            kv: None,
            hotkeys: HotkeyTable::new(),
            audio: AudioQueue::default(),
            cutout: CutoutQueue::default(),
            hud: HudQueue::default(),
            alive: Rc::new(Cell::new(true)),
            next_system: 0,
        }
    }

    /// The game ended: what the mods held in the host is let go.
    pub fn end_game(&mut self) {
        if let Some(p) = self.packs.as_mut() {
            p.clear();
        }
        self.hotkeys.clear();
        self.hud.clear();
        let mods: Vec<u16> = self.cutout.owners.keys().copied().collect();
        for m in mods {
            self.cutout.release_mod(m);
        }
        self.alive.set(false);
        self.alive = Rc::new(Cell::new(true));
    }

    /// One mod is gone while the game goes on.
    pub fn release_mod(&mut self, mod_idx: u16) {
        self.hotkeys.remove_mod(mod_idx);
        self.cutout.release_mod(mod_idx);
    }
}

impl Default for HostState {
    fn default() -> Self {
        Self::new()
    }
}

/// What a call can see besides the host's own state.
#[derive(Default)]
pub struct Env<'a> {
    pub world: Option<&'a dyn ModWorld>,
    pub terrain: Option<TerrainView<'a>>,
    pub gpu: Option<&'a ModGpu>,
    pub screen: Option<ScreenInfo>,
}

/// The host for one call: the state, what the call can see, and which mod it speaks for.
pub struct HostView<'a> {
    st: &'a mut HostState,
    env: Env<'a>,
    cur: u16,
}

impl<'a> HostView<'a> {
    pub fn new(st: &'a mut HostState, env: Env<'a>) -> Self {
        Self { st, env, cur: 0 }
    }
}

impl ClientHost for HostView<'_> {
    fn ctx(&mut self, mod_idx: u16) -> &mut dyn ClientCtx {
        self.cur = mod_idx;
        // `NetHost` keeps the index for its log lines and the sender of a message.
        self.st.net.ctx(mod_idx);
        if let Some(p) = self.st.packs.as_mut() {
            p.select(mod_idx);
        }
        if let Some(k) = self.st.kv.as_mut() {
            k.select(mod_idx);
        }
        self.st.hotkeys.select(mod_idx);
        self.st.cutout.select(mod_idx);
        self.st.hud.select(mod_idx);
        self.st.audio.cur = mod_idx;
        self.st.audio.pack = self.st.packs.as_ref().and_then(|p| p.pack_of(mod_idx));
        self
    }
}

impl ClientCtx for HostView<'_> {
    fn has(&self, _cap: Cap) -> bool {
        true
    }

    fn log(&mut self, level: LogLevel, msg: &str) {
        self.st.net.log(level, msg);
    }

    fn send(&mut self, ch: u8, payload: &[u8]) -> Result<(), CtxError> {
        self.st.net.send(ch, payload)
    }

    fn world(&self) -> Result<&dyn ModWorld, CtxError> {
        self.env.world.ok_or(CtxError::Unavailable("world"))
    }

    fn terrain(&self) -> Result<&dyn ModTerrain, CtxError> {
        self.env
            .terrain
            .as_ref()
            .map(|t| t as &dyn ModTerrain)
            .ok_or(CtxError::Unavailable("terrain"))
    }

    fn cutout(&mut self) -> Result<&mut dyn CutoutMask, CtxError> {
        Ok(&mut self.st.cutout)
    }

    fn kv(&mut self) -> Result<&mut dyn ModKv, CtxError> {
        self.st
            .kv
            .as_mut()
            .map(|k| k as &mut dyn ModKv)
            .ok_or(CtxError::Unavailable("kv"))
    }

    fn hotkeys(&mut self) -> Result<&mut dyn Hotkeys, CtxError> {
        Ok(&mut self.st.hotkeys)
    }

    fn audio(&mut self) -> Result<&mut dyn ModAudio, CtxError> {
        Ok(&mut self.st.audio)
    }

    fn assets(&self) -> Result<&dyn ModAssets, CtxError> {
        self.st
            .packs
            .as_ref()
            .map(|p| p as &dyn ModAssets)
            .ok_or(CtxError::Unavailable("assets"))
    }

    fn hud_textures(&mut self) -> Result<&mut dyn HudTextures, CtxError> {
        Ok(&mut self.st.hud)
    }

    fn screen(&self) -> ScreenInfo {
        self.env.screen.unwrap_or_else(|| self.st.net.screen())
    }

    fn gpu(&self) -> Result<&ModGpu, CtxError> {
        self.env.gpu.ok_or(CtxError::Unavailable("gpu"))
    }

    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        self.st.net.request_pointer_lock(lock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_world::world;

    #[test]
    fn the_replica_world_hides_what_the_player_has_not_seen() {
        let v = world();
        let w = ReplicaWorld::new(&v, 7);
        assert_eq!(w.me(), Some(v.human));
        assert_eq!(w.revision(), 7);
        assert_eq!(w.turn(), v.turn);
        let shape = w.map();
        assert_eq!((shape.width, shape.height), (v.map.width, v.map.height));
        assert!(!w.players().is_empty());
        let mut seen = 0;
        let mut unseen = 0;
        for p in 0..shape.plot_count() {
            let pv = w.plot(p);
            if v.map.plots[p as usize].flags.contains(PlotFlags::REVEALED) {
                seen += 1;
                assert!(pv.revealed && pv.class.is_some());
            } else {
                unseen += 1;
                assert_eq!(pv, PlotView::UNKNOWN, "never class or owner of fog");
            }
        }
        assert!(
            seen > 0 && unseen > 0,
            "the fixture has both ({seen}, {unseen})"
        );
        assert_eq!(w.plot(shape.plot_count() + 5), PlotView::UNKNOWN);
        // The map wraps along X in the wrapped fixture and plot_at agrees with the shape.
        assert_eq!(w.plot_at(0, 0), shape.index(0, 0));
        assert_eq!(w.plot_at(-1, -1), shape.index(-1, -1));
    }

    fn cut(w: u32, h: u32, x0: i32, z0: i32) -> CutoutRegion {
        CutoutRegion {
            x0,
            z0,
            width: w,
            height: h,
        }
    }

    #[test]
    fn cutout_writes_are_checked_against_the_grid_and_each_mod_takes_back_its_own() {
        let mut q = CutoutQueue::default();
        // No map yet: refused with a reason.
        assert!(matches!(
            q.set_cutout_mask(cut(2, 2, 0, -10), &[1; 4]),
            Err(CtxError::Unavailable(_))
        ));
        q.grid = Some(CutoutGrid::for_map(16, 10, true));
        let g = q.grid.unwrap();
        q.select(1);
        q.set_cutout_mask(cut(2, 2, 5, g.bz0), &[1; 4]).unwrap();
        // Wrong length, too wide, rows outside the map.
        assert!(q.set_cutout_mask(cut(2, 2, 0, g.bz0), &[1; 3]).is_err());
        assert!(
            q.set_cutout_mask(cut(g.cols + 1, 1, 0, g.bz0), &vec![1; g.cols as usize + 1])
                .is_err()
        );
        assert!(q.set_cutout_mask(cut(1, 1, 0, g.bz0 - 1), &[1]).is_err());
        assert!(
            q.set_cutout_mask(cut(1, 1, 0, g.bz0 + g.rows as i32), &[1])
                .is_err()
        );
        // A second mod writes elsewhere (a region past the seam is fine: x0 wraps in the renderer).
        q.select(2);
        q.set_cutout_mask(cut(2, 1, -3, g.bz0 + 1), &[1, 1])
            .unwrap();
        let cmds = q.take();
        assert_eq!(cmds.len(), 2);
        // Mod 1 clears: only its region is zeroed, the texture stays for mod 2.
        q.select(1);
        q.clear_cutout();
        assert_eq!(q.take(), [CutoutCmd::Set(cut(2, 2, 5, g.bz0), vec![0; 4])]);
        // The last one out frees the texture.
        q.select(2);
        q.clear_cutout();
        assert_eq!(q.take(), [CutoutCmd::Clear]);
        // Clearing with nothing written is nothing.
        q.clear_cutout();
        assert!(q.take().is_empty());
    }

    #[test]
    fn a_game_that_ends_takes_the_cutouts_back() {
        let mut st = HostState::new();
        st.cutout.grid = Some(CutoutGrid::for_map(16, 10, true));
        st.cutout.select(3);
        let bz0 = st.cutout.grid.unwrap().bz0;
        st.cutout.set_cutout_mask(cut(1, 1, 0, bz0), &[1]).unwrap();
        st.cutout.take();
        st.end_game();
        assert_eq!(st.cutout.take(), [CutoutCmd::Clear]);
    }

    #[test]
    fn hud_textures_are_checked_and_get_distinct_ids() {
        let mut h = HudQueue::default();
        assert!(h.upload_rgba8(2, 2, &[0; 15]).is_err());
        assert!(h.upload_rgba8(0, 2, &[]).is_err());
        let a = h.upload_rgba8(2, 2, &[255; 16]).unwrap();
        let b = h.upload_rgba8(1, 1, &[255; 4]).unwrap();
        assert_ne!(a, b);
        assert!(!a.is_external() && a != TextureId::WHITE);
        let pending = h.take();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].1.mips[0].len(), 16);
        // A cap per mod and game.
        for _ in 0..HUD_TEXTURES_MAX - 2 {
            h.upload_rgba8(1, 1, &[0; 4]).unwrap();
        }
        assert!(h.upload_rgba8(1, 1, &[0; 4]).is_err());
        h.select(9);
        h.upload_rgba8(1, 1, &[0; 4]).unwrap();
    }

    #[test]
    fn sounds_come_from_the_mods_pack_only() {
        use mod_assets::{AssetKind, NewFile};
        let pack = Pack::build(
            ModId::new("hello").unwrap(),
            Vec::new(),
            vec![NewFile {
                id: "sounds/ping".into(),
                kind: AssetKind::Sound,
                bytes: vec![1, 2, 3],
            }],
        )
        .unwrap();
        let mut a = AudioQueue::default();
        assert!(
            a.play("sounds/ping", PlayOpts::default()).is_err(),
            "no pack yet"
        );
        a.cur = 4;
        a.pack = Some(Rc::new(pack));
        assert!(a.play("sounds/nope", PlayOpts::default()).is_err());
        a.play(
            "sounds/ping",
            PlayOpts {
                volume: 9.0,
                pitch: 0.0,
                ..PlayOpts::default()
            },
        )
        .unwrap();
        a.mute_strategic(true);
        let (sounds, mute) = a.take();
        assert_eq!(mute, Some(true));
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].key, "4:sounds/ping");
        assert_eq!((sounds[0].volume, sounds[0].pitch), (4.0, 0.25), "clamped");
        for _ in 0..SOUNDS_MAX {
            let _ = a.play("sounds/ping", PlayOpts::default());
        }
        assert!(
            a.play("sounds/ping", PlayOpts::default()).is_err(),
            "bounded queue"
        );
    }
}
