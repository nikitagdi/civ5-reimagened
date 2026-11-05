//! `DistanceFog` subsystem. Owned by render-core: it only sets constants, so
//! it has no slot and no pass.

use crate::frame::{DrawContext, InitContext, PassSlot, PrepareContext, RenderSystem, SystemId};

/// Draws nothing; the fog constants come later with `lighting`.
#[derive(Default)]
pub struct DistanceFog;

impl RenderSystem for DistanceFog {
    fn id(&self) -> SystemId {
        SystemId::DistanceFog
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}
}
