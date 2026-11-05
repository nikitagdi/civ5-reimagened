//! `Shadows` subsystem. Owned by render-core; it has no `[DisableSystems]`
//! name (`ShadowLevel` 0 turns it off).

use crate::frame::{
    DrawContext, InitContext, PassId, PassSlot, PrepareContext, RenderSystem, SystemId,
};

/// The slot of the shadow passes in the plan. The passes themselves (`ShadowMap`, `ShadowBlur`)
/// are run by the renderer through [`crate::shadow::ShadowMap`]: what casts a shadow is in the
/// static models it owns. Draws nothing in `record`.
#[derive(Default)]
pub struct Shadows;

impl RenderSystem for Shadows {
    fn id(&self) -> SystemId {
        SystemId::Shadows
    }

    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Pass(PassId::ShadowBlur)]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}
}
