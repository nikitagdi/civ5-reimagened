//! `Particle` subsystem (world-objects WOB8b): the player of the
//! Fork particle effects (`fx:` blobs of `assets-b2` 4.12) and the bindings of the map.
//!
//! * [`sim`]: the simulation of the SDK (birth volumes, colour and size keys, forces, gravity,
//!   bursts, tracks, bezier paths, particles that follow their emitter);
//! * [`draw`]: the quads, batches and the eight alpha states;
//! * [`bindings`]: which visual effect plays where (the "look here" beam of `GameplayFX`, the
//!   fire over the buildings of a damaged city).
//!
//! An effect is asked for by the name of its visual effect (`ART_DEF_VEFFECT_*`, `fx:index`). The
//! client loads the blobs [`Particle::wanted`] names and hands them in; a request whose blobs are
//! not in yet waits for them (a one-shot for [`PENDING_SECONDS`]).
//!
//! SIMPLIFIED(SIMP-048): stand-ins where the reading of the SDK is open (answers-b update 10):
//! the bodies of the packages' pixel stages (`fork.wgsl`), the facing of the quads
//! without the camera flags (`draw::FACING_*`), the direction of the gravity, the sign of the
//! drag, linear tracks (no Hermite tangents), the bezier offset not turned by the path, the
//! sub-emitters (ref assets: one emitter of 1162) not spawned, string and triangle quads drawn
//! as rectangles, and the FOW mask tested when an effect starts and each frame from the cell
//! state the renderer passes.

pub mod bindings;
#[cfg(test)]
mod budget_tests;
pub mod draw;
#[cfg(test)]
mod gpu_tests;
pub mod sim;

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use asset_format::{FxEffect, FxIndex, LogicalId, TextureBlob};
use glam::{Mat4, Vec3};

use self::draw::{Batch, CameraAxes, ParticleDraw, Vertex};
use self::sim::{Sim, effect_matrix};
use crate::frame::{
    DrawContext, InitContext, MainSlot, PassSlot, PrepareContext, RenderSystem, SystemId,
    basis_rotation,
};
use crate::world::objects::input::ObjectCounters;

/// Length of the "look here" beam (`FX_Look_At_Me.psb`).
// re: `ART_DEF_VEFFECT_LOOK_AT` (inventory 1.3): 1.1 s
pub const LOOK_AT_SECONDS: f32 = 1.1;
/// The visual effect of `GameplayFX` (`Civ5ArtDefines_EventEffects.xml`).
pub const LOOK_AT_EFFECT: &str = "ART_DEF_VEFFECT_LOOK_AT";
/// How long a one-shot request waits for its blobs.
pub const PENDING_SECONDS: f32 = 5.0;
/// Longest step of the simulation (a frame after a stall does not throw particles away).
pub const MAX_STEP: f32 = 0.1;

/// How long an effect lives.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Lifetime {
    /// `Trigger` / `OneShot`: the visual effect's duration.
    OneShot,
    /// `Spawn`: until [`Particle::stop`].
    Held,
}

/// Where an effect goes: a world point (our basis) or a cell the renderer turns into one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    World(Vec3),
    Cell(i32, i32),
}

#[derive(Debug, Clone)]
struct Request {
    name: String,
    key: u64,
    place: Place,
    scale: f32,
    lifetime: Lifetime,
    age: f32,
}

/// What the renderer gives each frame.
pub struct ParticleFrame<'a> {
    /// The main camera's view matrix (our basis).
    pub view: Mat4,
    /// Animation time, seconds; the step is the difference with the last frame.
    pub time: f32,
    /// World point (our basis) of the ground at the middle of a cell, `None` off the map.
    pub cell_world: &'a dyn Fn(i32, i32) -> Option<Vec3>,
    /// Fog state of a cell: 0 unseen, 1 fogged, 2 visible.
    pub cell_fog: &'a dyn Fn(i32, i32) -> u8,
}

/// Particles of the map.
#[derive(Default)]
pub struct Particle {
    sim: Sim,
    index: Option<Rc<FxIndex>>,
    effects: HashMap<LogicalId, Rc<FxEffect>>,
    pending: Vec<Request>,
    /// Live keys: the cell and the FOW mask of each, for the fog test of each frame.
    live: HashMap<u64, (Option<(i32, i32)>, u8)>,
    textures_in: BTreeSet<LogicalId>,
    uploads: Vec<(LogicalId, TextureBlob)>,
    gpu: Option<ParticleDraw>,
    frame: Option<(Vec<Vertex>, Vec<Batch>)>,
    last_time: Option<f32>,
    /// Cells of the beams still lit and the seconds left.
    look_at: Vec<((i32, i32), f32)>,
    counters: ObjectCounters,
    asked_index: bool,
    city_fires: bindings::CityFires,
}

/// A stable key of a binding (`(source, trigger)` of the original, folded; FNV-1a).
pub fn key_of(kind: &str, a: i64, b: i64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in kind.bytes().chain(a.to_le_bytes()).chain(b.to_le_bytes()) {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The key of the `part`-th `.psb` of a request.
fn part_key(key: u64, part: usize) -> u64 {
    key ^ (part as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

impl Particle {
    /// Makes the GPU side (once, when the device exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) {
        self.gpu = Some(ParticleDraw::new(device, queue, samples));
    }

    pub fn set_index(&mut self, index: FxIndex) {
        self.index = Some(Rc::new(index));
    }

    pub fn has_index(&self) -> bool {
        self.index.is_some()
    }

    pub fn add_effect(&mut self, id: LogicalId, fx: FxEffect) {
        self.effects.insert(id, Rc::new(fx));
    }

    /// A texture an effect names (uploaded in `prepare`).
    pub fn add_texture(&mut self, id: LogicalId, blob: TextureBlob) {
        self.textures_in.insert(id.clone());
        self.uploads.push((id, blob));
    }

    /// The blobs to load: the index first (once a request asks for it), then the effects of the
    /// waiting requests, then the textures of the effects that are in.
    pub fn wanted(&mut self) -> Vec<LogicalId> {
        let Some(index) = &self.index else {
            if self.asked_index || self.pending.is_empty() {
                return Vec::new();
            }
            self.asked_index = true;
            return vec![LogicalId::new(asset_format::FX_INDEX_ID)];
        };
        let mut out = BTreeSet::new();
        for r in &self.pending {
            if let Some(v) = index.get(&r.name) {
                out.extend(
                    v.particles
                        .iter()
                        .filter(|p| !self.effects.contains_key(*p))
                        .cloned(),
                );
            }
        }
        for fx in self.effects.values() {
            for e in &fx.emitters {
                for f in &e.frames {
                    out.extend(
                        f.texture
                            .iter()
                            .filter(|t| !self.textures_in.contains(*t))
                            .cloned(),
                    );
                }
                if let asset_format::FxShader::FakeVolume { gradient: Some(g) } = &e.shader
                    && !self.textures_in.contains(g)
                {
                    out.insert(g.clone());
                }
            }
        }
        out.into_iter().collect()
    }

    /// Marks textures as asked for, so `wanted` does not name them again (a failed load stays
    /// out; the particles that need it are not drawn).
    pub fn mark_requested(&mut self, ids: &[LogicalId]) {
        for id in ids {
            if id.as_str().starts_with("tx:") {
                self.textures_in.insert(id.clone());
            }
        }
    }

    /// Plays the visual effect `name` (`ART_DEF_VEFFECT_*`) under `key` at `place`.
    pub fn play(&mut self, name: &str, key: u64, place: Place, scale: f32, lifetime: Lifetime) {
        self.pending.retain(|r| r.key != key);
        self.pending.push(Request {
            name: name.to_owned(),
            key,
            place,
            scale,
            lifetime,
            age: 0.0,
        });
        self.counters.placed_total += 1;
    }

    /// Whether `key` plays or waits for its blobs.
    pub fn is_playing(&self, key: u64) -> bool {
        self.pending.iter().any(|r| r.key == key) || self.sim.has(part_key(key, 0))
    }

    /// Stops the effect of `key` (`DeSpawn`); its particles live out.
    pub fn stop(&mut self, key: u64) {
        self.pending.retain(|r| r.key != key);
        let parts: Vec<u64> = self
            .live
            .keys()
            .copied()
            .filter(|k| (0..8).any(|i| part_key(key, i) == *k))
            .collect();
        for k in parts {
            self.sim.release(k);
            self.live.remove(&k);
        }
    }

    /// The beam "look here" on cell `(x, y)` (called where the original calls `GameplayFX`).
    pub fn look_at(&mut self, x: i32, y: i32) {
        self.look_at.retain(|(cell, _)| *cell != (x, y));
        self.look_at.push(((x, y), LOOK_AT_SECONDS));
        self.play(
            LOOK_AT_EFFECT,
            key_of("look_at", x.into(), y.into()),
            Place::Cell(x, y),
            1.0,
            Lifetime::OneShot,
        );
    }

    /// The fires over the buildings of a city (empty: none), as the city system places them
    /// ([`bindings::CityFires`]).
    pub fn city_fires(&mut self, key: crate::entities::CityKey, fires: &[bindings::CityFire]) {
        let mut d = std::mem::take(&mut self.city_fires);
        d.set(self, key, fires);
        self.city_fires = d;
    }

    /// A city left the feed.
    pub fn city_removed(&mut self, key: crate::entities::CityKey) {
        let mut d = std::mem::take(&mut self.city_fires);
        d.remove(self, key);
        self.city_fires = d;
    }

    /// Every city left the feed.
    pub fn cities_cleared(&mut self) {
        let mut d = std::mem::take(&mut self.city_fires);
        d.clear(self);
        self.city_fires = d;
    }

    /// The game was left: every effect of the map stops (city fires, the "look here" beams, anything still playing or
    /// waiting for its blobs). The loaded effects and textures stay.
    pub fn clear_effects(&mut self) {
        self.cities_cleared();
        self.pending.clear();
        let keys: Vec<u64> = self.live.keys().copied().collect();
        for k in keys {
            self.sim.release(k);
            self.live.remove(&k);
        }
        self.look_at.clear();
    }

    /// Ages the beams by `dt` seconds (the bookkeeping of [`Particle::beams`]).
    pub fn advance(&mut self, dt: f32) {
        for (_, left) in &mut self.look_at {
            *left -= dt;
        }
        self.look_at.retain(|(_, left)| *left > 0.0);
    }

    /// Cells of the beams that are lit.
    pub fn beams(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.look_at.iter().map(|(cell, _)| *cell)
    }

    /// Particles alive now.
    pub fn particles(&self) -> usize {
        self.sim.total()
    }

    /// Effects playing now.
    pub fn effects_live(&self) -> usize {
        self.sim.instances.len()
    }

    pub fn sim(&self) -> &Sim {
        &self.sim
    }

    /// One frame: starts the requests whose blobs are in, steps the simulation, builds the quads.
    pub fn frame(&mut self, f: &ParticleFrame<'_>) {
        let dt = match self.last_time {
            Some(t) => (f.time - t).clamp(0.0, MAX_STEP),
            None => 0.0,
        };
        self.last_time = Some(f.time);
        self.advance(dt);
        self.start_requests(dt, f);
        // The fog: an effect shows only where the cell state meets its mask.
        for (key, (cell, mask)) in &self.live {
            let shown = cell.is_none_or(|(x, y)| (f.cell_fog)(x, y) & mask != 0);
            self.sim.set_visible(*key, shown);
        }
        self.sim.step(dt);
        let alive: BTreeSet<u64> = self.sim.instances.iter().map(|i| i.key).collect();
        self.live.retain(|k, _| alive.contains(k));
        let cam = CameraAxes::from_view(f.view);
        let frame = match &self.gpu {
            Some(gpu) => draw::build(&self.sim, cam, &|t| gpu.has_texture(t)),
            None => (Vec::new(), Vec::new()),
        };
        self.frame = Some(frame);
    }

    fn start_requests(&mut self, dt: f32, f: &ParticleFrame<'_>) {
        let Some(index) = self.index.clone() else {
            for r in &mut self.pending {
                r.age += dt;
            }
            self.pending
                .retain(|r| r.lifetime == Lifetime::Held || r.age < PENDING_SECONDS);
            return;
        };
        let b = basis_rotation();
        let mut keep = Vec::new();
        for mut r in std::mem::take(&mut self.pending) {
            let Some(v) = index.get(&r.name) else {
                continue; // no such effect with particles: nothing to play
            };
            let at = match r.place {
                Place::World(p) => Some((p, None)),
                Place::Cell(x, y) => (f.cell_world)(x, y).map(|p| (p, Some((x, y)))),
            };
            let Some((at, cell)) = at else {
                continue;
            };
            if !v.particles.iter().all(|p| self.effects.contains_key(p)) {
                r.age += dt;
                if r.lifetime == Lifetime::Held || r.age < PENDING_SECONDS {
                    keep.push(r);
                }
                continue;
            }
            // re: InitHandle 0x088ca544 drops the effect if (hexState & mask) == 0
            if let Some((x, y)) = cell
                && (f.cell_fog)(x, y) & v.fow_mask == 0
            {
                continue;
            }
            let scale = v.scale * r.scale;
            let m = effect_matrix(b.transform_point3(at), v.translation, v.rotation, scale);
            let duration = match r.lifetime {
                Lifetime::OneShot => Some(v.duration.max(0.0)),
                Lifetime::Held => None,
            };
            for (i, id) in v.particles.iter().enumerate() {
                let key = part_key(r.key, i);
                self.sim
                    .spawn(key, self.effects[id].clone(), m, scale, duration);
                self.live.insert(key, (cell, v.fow_mask));
            }
        }
        self.pending = keep;
    }

    pub fn counters(&self) -> ObjectCounters {
        ObjectCounters {
            records: self.sim.instances.len() as u32,
            ..self.counters
        }
    }
}

impl RenderSystem for Particle {
    fn id(&self) -> SystemId {
        SystemId::Particle
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Main(MainSlot::Particle)]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        let Some(gpu) = &mut self.gpu else {
            return;
        };
        for (id, blob) in std::mem::take(&mut self.uploads) {
            match crate::texture::upload_blob(
                cx.device,
                cx.queue,
                &blob,
                id.as_str(),
                crate::texture::UploadOptions::WORLD,
            ) {
                Ok(up) if up.view_dimension == wgpu::TextureViewDimension::D2 => {
                    gpu.add_texture(id, up.texture.create_view(&Default::default()));
                }
                Ok(_) => tracing::warn!("particle texture {}: not 2D", id.as_str()),
                Err(e) => tracing::warn!("particle texture {}: {e}", id.as_str()),
            }
        }
        if let Some((verts, batches)) = self.frame.take() {
            gpu.prepare(cx.device, cx.queue, &verts, batches);
        }
    }

    fn record(&self, _slot: PassSlot, cx: &mut DrawContext<'_, '_>) {
        if let Some(gpu) = &self.gpu {
            gpu.record(cx.pass, cx.view_group, cx.stats);
        }
    }

    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("particles", self.sim.total() as u64),
            ("effects", self.sim.instances.len() as u64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::FxVisEffect;

    #[test]
    fn a_beam_burns_for_its_time_and_a_second_call_restarts_it() {
        let mut p = Particle::default();
        p.look_at(2, 3);
        p.advance(1.0);
        assert_eq!(p.beams().collect::<Vec<_>>(), [(2, 3)]);
        p.look_at(2, 3);
        p.advance(1.0);
        assert_eq!(p.beams().count(), 1, "restarted at 1.1 s");
        p.advance(0.2);
        assert_eq!(p.beams().count(), 0);
    }

    fn index() -> FxIndex {
        FxIndex {
            effects: vec![FxVisEffect {
                name: LOOK_AT_EFFECT.into(),
                translation: [0.0; 3],
                rotation: [90.0, 0.0, 0.0],
                scale: 1.0,
                duration: 1.1,
                fow_mask: 3,
                particles: vec![LogicalId::new("fx:t")],
            }],
        }
    }

    fn frame(p: &mut Particle, time: f32, fog: u8) {
        let cell = |x: i32, y: i32| Some(Vec3::new(x as f32 * 64.0, 0.0, -(y as f32) * 55.0));
        let fogf = move |_: i32, _: i32| fog;
        p.frame(&ParticleFrame {
            view: Mat4::IDENTITY,
            time,
            cell_world: &cell,
            cell_fog: &fogf,
        });
    }

    fn burst_effect(n: i32) -> FxEffect {
        let mut pr = sim::tests::props();
        pr.burst = n;
        pr.rate = 0.0;
        (*sim::tests::effect(pr)).clone()
    }

    #[test]
    fn a_request_waits_for_its_blobs_then_plays() {
        let mut p = Particle::default();
        p.look_at(1, 1);
        assert_eq!(p.wanted(), [LogicalId::new("fx:index")]);
        assert!(p.wanted().is_empty(), "asked once");
        p.set_index(index());
        assert_eq!(p.wanted(), [LogicalId::new("fx:t")]);
        frame(&mut p, 0.0, 2);
        assert_eq!(p.effects_live(), 0, "the effect is not in yet");
        p.add_effect(LogicalId::new("fx:t"), burst_effect(2));
        assert_eq!(p.wanted(), [LogicalId::new("tx:a.dds")]);
        frame(&mut p, 0.05, 2);
        assert_eq!(p.effects_live(), 1);
        assert_eq!(p.particles(), 2);
        // The beam stops at 1.1 s and its particles (life 1 s) die after.
        for i in 2..60 {
            frame(&mut p, 0.05 * i as f32, 2);
        }
        assert_eq!(p.effects_live(), 0);
    }

    #[test]
    fn the_fog_mask_keeps_an_effect_off_hidden_cells() {
        let mut p = Particle::default();
        let mut idx = index();
        idx.effects[0].fow_mask = 2;
        p.set_index(idx);
        p.add_effect(LogicalId::new("fx:t"), burst_effect(1));
        p.play(LOOK_AT_EFFECT, 7, Place::Cell(0, 0), 1.0, Lifetime::Held);
        frame(&mut p, 0.0, 1);
        assert_eq!(p.effects_live(), 0, "fogged: dropped at the start");
        p.play(LOOK_AT_EFFECT, 7, Place::Cell(0, 0), 1.0, Lifetime::Held);
        frame(&mut p, 0.1, 2);
        assert_eq!(p.effects_live(), 1);
        assert!(p.is_playing(7));
        frame(&mut p, 0.15, 1);
        assert!(!p.sim().instances[0].visible, "hidden while fogged");
        p.stop(7);
        assert!(!p.is_playing(7));
        for i in 4..40 {
            frame(&mut p, 0.05 * i as f32, 2);
        }
        assert_eq!(p.effects_live(), 0);
    }
}
