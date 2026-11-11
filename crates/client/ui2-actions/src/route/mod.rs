//! Routes: what each intent does. A route changes the `ui` state, asks the sink for something, or refuses with a
//! toast; it never reads the replica, only the model the player sees.

pub mod window;

use ui2_model::ModelStore;

use crate::sink::OrderSink;
use crate::ui::UiState;

/// What a route works with.
pub(crate) struct Ctx<'a> {
    pub ui: &'a mut UiState,
    /// The model as the player sees it (the roots the source wrote).
    pub model: &'a ModelStore,
    #[allow(dead_code)] // routes that send orders take it
    pub sink: &'a mut dyn OrderSink,
    pub now_ms: u64,
}
