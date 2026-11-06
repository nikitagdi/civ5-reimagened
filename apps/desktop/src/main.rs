//! Native client entry (`app-desktop`); the whole client is `app_desktop::main_with` in `lib.rs`.

fn main() -> anyhow::Result<()> {
    app_desktop::main_with(client_app::mod_hooks::ModHooks::new())
}
