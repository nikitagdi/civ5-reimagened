//! M1: runs the EUI contexts of the original (tool tips, `EUI_context`, `TopPanel`,
//! `NotificationPanel`) on the fixture stubs from the game install and prints the result.
//!
//! `cargo run -p ui --example m1 -- [--dump-tree]` (set `CIV5_GAME_DIR` to the game's install folder).

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let dump = std::env::args().any(|a| a == "--dump-tree");
    let Some(dir) = std::env::var_os("CIV5_GAME_DIR").map(std::path::PathBuf::from) else {
        eprintln!("set CIV5_GAME_DIR to the game's install folder (the one with Assets)");
        std::process::exit(2);
    };
    let assets = dir.join("Assets");
    let stubs = include_str!("../tests/fixtures/m1_stubs.lua");
    let r = ui::dev::run_m1(&assets, stubs, dump);
    if dump {
        println!("{}", r.tree);
    }
    for l in &r.log {
        println!("log: {l}");
    }
    println!("controls: {}", r.controls);
    println!("xml warnings: {}", r.xml_warnings);
    println!(
        "draw commands: {} ({} vertices)",
        r.draw_cmds, r.draw_vertices
    );
    println!("zero-sized visible controls: {}", r.zero_sized.len());
    for z in &r.zero_sized {
        println!("zero: {z}");
    }
    for w in &r.warnings {
        println!("warn: {w}");
    }
    println!("lua errors: {}", r.lua_errors);
    for e in &r.errors {
        println!("error: {e}");
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
