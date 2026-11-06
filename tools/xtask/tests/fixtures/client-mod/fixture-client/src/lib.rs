//! Test fixture of `cargo xtask mods build --target client`: the client half of a mod that does nothing.

use mod_client::{ClientMod, ClientRegistry};

struct Nothing;

impl ClientMod for Nothing {}

/// The entry named in `mod.toml`.
pub fn register(r: &mut ClientRegistry) {
    r.add("fixture-client", || Box::new(Nothing));
}
