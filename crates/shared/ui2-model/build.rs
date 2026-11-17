#[path = "schema/assemble.rs"]
mod assemble;

fn main() {
    use std::{env, fs, path::PathBuf};
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest directory"));
    let dir = root.join("schema/screens");
    println!("cargo:rerun-if-changed=schema");
    let mut paths = fs::read_dir(dir)
        .expect("screen schemas")
        .map(|e| e.expect("schema entry").path())
        .filter(|p| p.extension().is_some_and(|s| s == "toml"))
        .collect::<Vec<_>>();
    paths.sort();
    let screens = paths
        .into_iter()
        .map(|p| {
            let name = p
                .file_stem()
                .expect("screen filename")
                .to_string_lossy()
                .into_owned();
            let script = root
                .join("../game-host/screens")
                .join(format!("{name}.lua"));
            assert!(script.is_file(), "screen {name} has no server script");
            println!("cargo:rerun-if-changed={}", script.display());
            (name, fs::read_to_string(p).expect("screen schema"))
        })
        .collect::<Vec<_>>();
    let source = assemble::assemble(
        &fs::read_to_string(root.join("schema/ui-model.toml")).expect("model schema"),
        &screens,
    )
    .expect("assemble screen schemas");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("output directory")).join("ui-model.toml"),
        source,
    )
    .expect("embedded schema");
}
