//! `YieldIcons` subsystem. Stub of RC-1: `world-objects` writes the body.
//! The table gives no pass or slot yet (the B1 adapter slot comes with R0.10), so `passes` is empty.

use crate::frame::{DrawContext, InitContext, PassSlot, PrepareContext, RenderSystem, SystemId};

/// Draws nothing yet.
#[derive(Default)]
pub struct YieldIcons;

impl RenderSystem for YieldIcons {
    fn id(&self) -> SystemId {
        SystemId::YieldIcons
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}
}
