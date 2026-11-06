//! `civ5-server [--config server.toml] [flags]`: the gateway (the body is in `server::entry`).

use std::process::ExitCode;

fn main() -> ExitCode {
    server::entry::main_with(mod_host_server::HostRegistry::new())
}
