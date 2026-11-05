//! The fixed list of frame passes and main-pass slots (table 4.1). Not a
//! dependency-resolving graph: the order is the original's, [`plan`] only picks which enabled
//! subsystem takes part in which step.

use super::system::{PassSlot, RenderSystem, SystemId};

/// One row of table 4.1, in frame order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PassId {
    Prepare,
    Minimap,
    TerrainBake,
    WaterPrep,
    ShadowMap,
    ShadowBlur,
    Reflection,
    ScreenOverlay,
    FowMask,
    Main,
    Resolve,
    Present,
    /// The original UI's draw list (`ui-render`), over the finished frame and under egui.
    UiGame,
    Ui,
    /// Only for `golden`: the same calls into an `R8Uint` class target. Not in [`PassId::ORDER`].
    ClassMask,
}

/// Who opens the pass and how subsystems take part in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassMode {
    /// CPU only.
    Cpu,
    /// The subsystem encodes the whole pass itself (`record_offscreen`).
    Encoder,
    /// `render-core` begins the pass; subsystems `record` into it, once per visible map copy.
    CorePass,
    /// `render-core` (or the UI layer) does it; subsystems are not called.
    Core,
}

impl PassId {
    /// Table 4.1, rows 0 to 13 (`RenderGame` order).
    pub const ORDER: [PassId; 14] = [
        PassId::Prepare,
        PassId::Minimap,
        PassId::TerrainBake,
        PassId::WaterPrep,
        PassId::ShadowMap,
        PassId::ShadowBlur,
        PassId::Reflection,
        PassId::ScreenOverlay,
        PassId::FowMask,
        PassId::Main,
        PassId::Resolve,
        PassId::Present,
        PassId::UiGame,
        PassId::Ui,
    ];

    pub fn mode(self) -> PassMode {
        match self {
            PassId::Prepare => PassMode::Cpu,
            PassId::Minimap
            | PassId::TerrainBake
            | PassId::WaterPrep
            | PassId::ShadowBlur
            | PassId::ScreenOverlay
            | PassId::FowMask => PassMode::Encoder,
            PassId::ShadowMap | PassId::Reflection | PassId::Main | PassId::ClassMask => {
                PassMode::CorePass
            }
            PassId::Resolve | PassId::Present | PassId::UiGame | PassId::Ui => PassMode::Core,
        }
    }
}

/// Slots of the main pass, in the original's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MainSlot {
    /// Ground, then water, rivers, ice.
    Terrain,
    Waves,
    City,
    Arrows,
    Forest,
    /// With the dynamic geometry.
    Landmark,
    UnitZFail,
    Unit,
    Particle,
    FowClassic,
    FluidFow,
    MovementPath,
}

impl MainSlot {
    pub const ORDER: [MainSlot; 12] = [
        MainSlot::Terrain,
        MainSlot::Waves,
        MainSlot::City,
        MainSlot::Arrows,
        MainSlot::Forest,
        MainSlot::Landmark,
        MainSlot::UnitZFail,
        MainSlot::Unit,
        MainSlot::Particle,
        MainSlot::FowClassic,
        MainSlot::FluidFow,
        MainSlot::MovementPath,
    ];
}

/// One step of a frame. The executor walks [`FramePlan::steps`] and does the GPU work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// `WorldServices::prepare`, always first.
    PrepareServices,
    /// `RenderSystem::prepare`.
    Prepare(SystemId),
    /// `RenderSystem::record_offscreen` for an [`PassMode::Encoder`] pass.
    Offscreen {
        pass: PassId,
        system: SystemId,
    },
    /// `render-core` begins a [`PassMode::CorePass`] pass.
    BeginPass(PassId),
    /// `RenderSystem::record` into the open pass, once per visible map copy.
    Record {
        pass: PassId,
        slot: PassSlot,
        system: SystemId,
    },
    EndPass(PassId),
    /// A pass `render-core` does itself (resolve, present, UI).
    Core(PassId),
}

impl Step {
    /// The row of table 4.1 whose time and draws the step counts toward.
    pub fn pass(&self) -> PassId {
        match *self {
            Step::PrepareServices | Step::Prepare(_) => PassId::Prepare,
            Step::Offscreen { pass, .. }
            | Step::BeginPass(pass)
            | Step::Record { pass, .. }
            | Step::EndPass(pass)
            | Step::Core(pass) => pass,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FramePlan {
    pub steps: Vec<Step>,
}

/// Order of one frame for the enabled subsystems. `systems` is in `WorldSystems` field order,
/// which is also the order inside a slot. A subsystem for which `enabled` is false appears in no
/// step, so none of its methods is called. A [`PassMode::CorePass`] pass with no recorder is
/// skipped, except `Main`, which always clears.
pub fn plan(systems: &[&dyn RenderSystem], enabled: &dyn Fn(SystemId) -> bool) -> FramePlan {
    let live: Vec<&&dyn RenderSystem> = systems.iter().filter(|s| enabled(s.id())).collect();
    let mut steps = vec![Step::PrepareServices];
    steps.extend(live.iter().map(|s| Step::Prepare(s.id())));

    for pass in PassId::ORDER {
        match pass.mode() {
            PassMode::Cpu => {}
            PassMode::Encoder => {
                for s in &live {
                    if s.passes().contains(&PassSlot::Pass(pass)) {
                        steps.push(Step::Offscreen {
                            pass,
                            system: s.id(),
                        });
                    }
                }
            }
            PassMode::CorePass => {
                let mut records = Vec::new();
                if pass == PassId::Main {
                    for slot in MainSlot::ORDER {
                        let slot = PassSlot::Main(slot);
                        for s in &live {
                            if s.passes().contains(&slot) {
                                records.push((slot, s.id()));
                            }
                        }
                    }
                } else {
                    let slot = PassSlot::Pass(pass);
                    for s in &live {
                        if s.passes().contains(&slot) {
                            records.push((slot, s.id()));
                        }
                    }
                }
                if records.is_empty() && pass != PassId::Main {
                    continue;
                }
                steps.push(Step::BeginPass(pass));
                for (slot, system) in records {
                    steps.push(Step::Record { pass, slot, system });
                }
                steps.push(Step::EndPass(pass));
            }
            PassMode::Core => steps.push(Step::Core(pass)),
        }
    }
    FramePlan { steps }
}

/// `Renderer::settled()` for the subsystems: nothing left to bake or load. Disabled subsystems do
/// not count, they never run.
pub fn settled(systems: &[&dyn RenderSystem], enabled: &dyn Fn(SystemId) -> bool) -> bool {
    systems
        .iter()
        .filter(|s| enabled(s.id()))
        .all(|s| s.pending_work() == 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::system::{DrawContext, InitContext, PrepareContext};
    use std::cell::Cell;

    /// Counts calls through `Cell`s; the GPU contexts cannot be built in a test, so only `plan`
    /// and `settled` are exercised.
    struct Mock {
        id: SystemId,
        passes: &'static [PassSlot],
        pending: u32,
        calls: Cell<u32>,
    }

    impl RenderSystem for Mock {
        fn id(&self) -> SystemId {
            self.id
        }
        fn passes(&self) -> &'static [PassSlot] {
            self.calls.set(self.calls.get() + 1);
            self.passes
        }
        fn init(&mut self, _: &mut InitContext<'_>) {}
        fn prepare(&mut self, _: &mut PrepareContext<'_>) {}
        fn record(&self, _: PassSlot, _: &mut DrawContext<'_, '_>) {}
        fn pending_work(&self) -> u32 {
            self.pending
        }
    }

    fn mock(id: SystemId, passes: &'static [PassSlot], pending: u32) -> Mock {
        Mock {
            id,
            passes,
            pending,
            calls: Cell::new(0),
        }
    }

    fn all(_: SystemId) -> bool {
        true
    }

    #[test]
    fn pass_and_slot_order_is_table_4_1() {
        use PassId::*;
        assert_eq!(
            PassId::ORDER,
            [
                Prepare,
                Minimap,
                TerrainBake,
                WaterPrep,
                ShadowMap,
                ShadowBlur,
                Reflection,
                ScreenOverlay,
                FowMask,
                Main,
                Resolve,
                Present,
                UiGame,
                Ui
            ]
        );
        use MainSlot::*;
        assert_eq!(
            MainSlot::ORDER,
            [
                Terrain,
                Waves,
                City,
                Arrows,
                Forest,
                Landmark,
                UnitZFail,
                Unit,
                Particle,
                FowClassic,
                FluidFow,
                MovementPath
            ]
        );
    }

    #[test]
    fn plan_follows_pass_then_slot_order_not_system_order() {
        // Declared in reverse of the frame order on purpose.
        let unit = mock(
            SystemId::Unit,
            &[
                PassSlot::Pass(PassId::ShadowMap),
                PassSlot::Main(MainSlot::Unit),
                PassSlot::Main(MainSlot::UnitZFail),
            ],
            0,
        );
        let terrain = mock(
            SystemId::Terrain,
            &[
                PassSlot::Main(MainSlot::Terrain),
                PassSlot::Pass(PassId::TerrainBake),
            ],
            0,
        );
        let fow = mock(SystemId::FluidFow, &[PassSlot::Pass(PassId::FowMask)], 0);
        let systems: [&dyn RenderSystem; 3] = [&unit, &terrain, &fow];
        let p = plan(&systems, &all);
        use Step::*;
        let main = |slot, system| Record {
            pass: PassId::Main,
            slot: PassSlot::Main(slot),
            system,
        };
        assert_eq!(
            p.steps,
            vec![
                PrepareServices,
                Prepare(SystemId::Unit),
                Prepare(SystemId::Terrain),
                Prepare(SystemId::FluidFow),
                Offscreen {
                    pass: PassId::TerrainBake,
                    system: SystemId::Terrain
                },
                BeginPass(PassId::ShadowMap),
                Record {
                    pass: PassId::ShadowMap,
                    slot: PassSlot::Pass(PassId::ShadowMap),
                    system: SystemId::Unit
                },
                EndPass(PassId::ShadowMap),
                Offscreen {
                    pass: PassId::FowMask,
                    system: SystemId::FluidFow
                },
                BeginPass(PassId::Main),
                main(MainSlot::Terrain, SystemId::Terrain),
                main(MainSlot::UnitZFail, SystemId::Unit),
                main(MainSlot::Unit, SystemId::Unit),
                EndPass(PassId::Main),
                Core(PassId::Resolve),
                Core(PassId::Present),
                Core(PassId::UiGame),
                Core(PassId::Ui),
            ]
        );
    }

    #[test]
    fn the_original_ui_comes_after_the_minimap_and_before_egui() {
        let at = |p: PassId| PassId::ORDER.iter().position(|&q| q == p).unwrap();
        assert!(at(PassId::Minimap) < at(PassId::UiGame));
        assert!(at(PassId::Present) < at(PassId::UiGame));
        assert!(at(PassId::UiGame) < at(PassId::Ui));
        // The plan always carries the step; the renderer skips it for an empty list.
        let steps = plan(&[], &all).steps;
        let step_at = |p: PassId| steps.iter().position(|s| *s == Step::Core(p)).unwrap();
        assert!(step_at(PassId::UiGame) < step_at(PassId::Ui));
        assert_eq!(PassId::UiGame.mode(), PassMode::Core);
    }

    #[test]
    fn disabled_system_is_not_called() {
        let terrain = mock(SystemId::Terrain, &[PassSlot::Main(MainSlot::Terrain)], 0);
        let unit = mock(
            SystemId::Unit,
            &[
                PassSlot::Pass(PassId::ShadowMap),
                PassSlot::Main(MainSlot::Unit),
            ],
            5,
        );
        let systems: [&dyn RenderSystem; 2] = [&terrain, &unit];
        let enabled = |id: SystemId| id != SystemId::Unit;
        let p = plan(&systems, &enabled);
        assert!(p.steps.iter().all(|s| match s {
            Step::Prepare(id) => *id != SystemId::Unit,
            Step::Offscreen { system, .. } | Step::Record { system, .. } => {
                *system != SystemId::Unit
            }
            _ => true,
        }));
        // The shadow pass has no recorder left, so it is not even opened.
        assert!(!p.steps.contains(&Step::BeginPass(PassId::ShadowMap)));
        assert!(p.steps.contains(&Step::BeginPass(PassId::Main)));
        // The disabled system was not asked for its slots, and its backlog does not count.
        assert_eq!(unit.calls.get(), 0);
        assert!(settled(&systems, &enabled));
        assert!(!settled(&systems, &all));
    }

    #[test]
    fn every_step_counts_toward_a_row_of_the_table() {
        assert_eq!(Step::PrepareServices.pass(), PassId::Prepare);
        assert_eq!(Step::Prepare(SystemId::Unit).pass(), PassId::Prepare);
        let record = Step::Record {
            pass: PassId::Main,
            slot: PassSlot::Main(MainSlot::Unit),
            system: SystemId::Unit,
        };
        assert_eq!(record.pass(), PassId::Main);
        assert_eq!(Step::Core(PassId::Present).pass(), PassId::Present);
        let p = plan(&[], &all);
        for step in &p.steps {
            assert!(PassId::ORDER.contains(&step.pass()));
        }
    }

    #[test]
    fn services_prepare_first() {
        let p = plan(&[], &all);
        assert_eq!(p.steps[0], Step::PrepareServices);
        assert!(p.steps.contains(&Step::BeginPass(PassId::Main)));
    }
}
