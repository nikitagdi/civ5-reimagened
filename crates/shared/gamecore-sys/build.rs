//! Builds the stub and spike, or links a real gamecore from `GAMECORE_LIB_DIR` (architecture §8).
//!
//! Real-link contract (per target, first match wins):
//! `GAMECORE_LIB_DIR_<target triple, '-' -> '_'>`, `GAMECORE_LIB_DIR_<arch>` (`x86_64`, `wasm32`), `GAMECORE_LIB_DIR`;
//! the threaded wasm build (`+atomics`, a shared memory) first tries `GAMECORE_LIB_DIR_wasm32_threads`: an archive built
//! with `build.sh --threads` (atomics and bulk memory; the single-threaded one does not link into a shared memory).
//! The directory holds `build-info.txt` and static archives. `build-info.txt` lines used here:
//! - `lua headers: <path> ... lua_Integer = int` (required, checked against lua-sys);
//! - `archives: a.a b.a ...` (optional; archive file names relative to the directory, space or comma
//!   separated; default: `libgamecore.a` first, then every other `lib*.a`, sorted);
//! - `stdlib: libc++|libstdc++` (optional; the C++ library the archives were built with;
//!   default: whatever build-support picked for the spike; ignored on wasm, where lua-sys links libc++).
//!
//! Lua and SQLite are not in the archives: lua-sys and sqlite-sys provide them.

use std::path::{Path, PathBuf};

use build_support::Lang;

const LEKMOD_LUA_INCLUDE_REL: &str =
    "LEKMOD_DLL/CvGameCoreDLL_Expansion2/ThirdPartyLibs/Lua51/include";

/// Env var names tried for the current target, most specific first.
fn lib_dir_vars() -> Vec<String> {
    let triple = std::env::var("TARGET")
        .unwrap_or_default()
        .replace('-', "_");
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let mut vars = Vec::new();
    if build_support::wasm_threads() {
        vars.push("GAMECORE_LIB_DIR_wasm32_threads".to_string());
    }
    vars.extend([
        format!("GAMECORE_LIB_DIR_{triple}"),
        format!("GAMECORE_LIB_DIR_{arch}"),
        "GAMECORE_LIB_DIR".to_string(),
    ]);
    vars
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("set by Cargo"));
    let cpp = manifest.join("cpp");
    let lua_include = PathBuf::from(
        std::env::var("DEP_LUA51_INCLUDE")
            .expect("lua-sys (links = \"lua51\") must pass DEP_LUA51_INCLUDE"),
    );
    build_support::rerun_if_dir_changed(&cpp);
    println!("cargo::rustc-check-cfg=cfg(gamecore_real)");
    let vars = lib_dir_vars();
    for v in &vars {
        println!("cargo:rerun-if-env-changed={v}");
    }

    let real_dir = vars
        .iter()
        .find_map(|v| std::env::var_os(v).filter(|d| !d.is_empty()));
    if build_support::wasm_threads()
        && std::env::var_os("GAMECORE_LIB_DIR_wasm32_threads").is_none()
    {
        // The single-threaded archive does not link into a shared memory, and a stub core must not stand in silently.
        if let Some(v) = vars
            .iter()
            .find(|v| std::env::var_os(v).is_some_and(|d| !d.is_empty()))
        {
            panic!(
                "threaded wasm build: {v} names a game core built without atomics; set GAMECORE_LIB_DIR_wasm32_threads                  to an archive built with `build.sh --target wasm --threads` (cargo xtask gamecore builds it into wasm-mt/)"
            );
        }
    }
    if let Some(dir) = &real_dir {
        link_real_gamecore(Path::new(dir), &lua_include);
    }

    let mut stub = build_support::cc_build(Lang::Cxx03);
    stub.include(cpp.join("include"))
        .include(cpp.join("stub"))
        .include(&lua_include)
        .define("LEKMOD_LUA_INTEGER", "int")
        .file(cpp.join("stub/spike.cpp"));
    if real_dir.is_none() {
        stub.file(cpp.join("stub/gamecore_stub.cpp"));
    }
    stub.compile("gamecore_stub");
}

/// Checks `build-info.txt` against the lua-sys headers and links the archives.
fn link_real_gamecore(dir: &Path, lua_include: &Path) {
    let info_path = dir.join("build-info.txt");
    println!("cargo:rerun-if-changed={}", info_path.display());
    if !dir.join("libgamecore.a").is_file() || !info_path.is_file() {
        panic!(
            "GAMECORE_LIB_DIR={}: need libgamecore.a and build-info.txt (headless/tools/clang/build.sh --mode lib --out <dir>)",
            dir.display()
        );
    }
    let info = std::fs::read_to_string(&info_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", info_path.display()));
    let field = |key: &str| {
        info.lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::trim)
    };
    let lua_line = field("lua headers:")
        .unwrap_or_else(|| panic!("{} has no 'lua headers:' line", info_path.display()));
    // lua-sys uses the vendored Lua 5.1.5 headers (third_party/lua-5.1.5/src); their luaconf.h carries the same
    // LEKMOD_LUA_INTEGER blocks as the Lekmod headers the archives are built with, so both sides share one Lua ABI.
    let ours = lua_include.join("luaconf.h");
    let ours_text = std::fs::read_to_string(&ours)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", ours.display()));
    if !lua_line.starts_with(LEKMOD_LUA_INCLUDE_REL) || !ours_text.contains("LEKMOD_LUA_INTEGER") {
        panic!(
            "Lua headers differ: libgamecore.a was built with '{lua_line}', lua-sys uses '{}' (expected the Lekmod headers on the gamecore side and a luaconf.h with LEKMOD_LUA_INTEGER here)",
            ours.display()
        );
    }
    if !lua_line.contains("lua_Integer = int") {
        panic!(
            "libgamecore.a was not built with lua_Integer = int ('{lua_line}'), but lua-sys uses int (D12)"
        );
    }

    let archives = match field("archives:") {
        Some(list) => list
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect(),
        None => default_archives(dir),
    };
    println!("cargo:rustc-link-search=native={}", dir.display());
    for a in &archives {
        let path = dir.join(a);
        if !path.is_file() {
            panic!(
                "{}: archive '{a}' from the archive list is missing",
                dir.display()
            );
        }
        println!("cargo:rerun-if-changed={}", path.display());
        let stem = a
            .strip_prefix("lib")
            .and_then(|n| n.strip_suffix(".a"))
            .unwrap_or_else(|| panic!("archive '{a}' must be named lib<name>.a"));
        // lld (the default linker for Rust on Linux, and wasm-ld) resolves archives in any order, so
        // cycles between archives need no link group.
        println!("cargo:rustc-link-lib=static:-bundle={stem}");
    }

    // The spike's own C++ library is linked by build-support; add the archives' one only when it is named.
    let wasm = std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32");
    match field("stdlib:") {
        Some("libc++") if !wasm => println!("cargo:rustc-link-lib=c++"),
        Some("libstdc++") if !wasm => println!("cargo:rustc-link-lib=stdc++"),
        _ => {}
    }
    println!("cargo:rustc-cfg=gamecore_real");
}

/// `libgamecore.a` first, then the other `lib*.a` files in name order.
fn default_archives(dir: &Path) -> Vec<String> {
    let mut rest: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("lib") && n.ends_with(".a") && n != "libgamecore.a")
        .collect();
    rest.sort();
    let mut all = vec!["libgamecore.a".to_string()];
    all.extend(rest);
    all
}
