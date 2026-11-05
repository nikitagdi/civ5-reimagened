//! Shared C/C++ build flags for `*-sys` build scripts (MSVC, clang, wasi-sdk) and the wasm link list.

use std::env;
use std::path::{Path, PathBuf};

/// Re-exported so `*-sys` build scripts share one `cc`.
pub use cc;

/// Same flags as the Lekmod clang port.
pub const PORT_FLAGS: &[&str] = &[
    "-ffp-contract=off",
    "-fno-fast-math",
    "-fwrapv",
    "-fno-strict-aliasing",
];

/// C/C++ compiler family for the current target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toolchain {
    Msvc,
    Clang,
    /// wasi-sdk, targeting `wasm32-wasip1`.
    WasiSdk {
        root: PathBuf,
    },
}

/// Language of a translation unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    C,
    Cxx17,
    /// gamecore dialect (VS2008). MSVC has no C++03 mode, so it uses `/std:c++14`.
    Cxx03,
}

/// Toolchain from `CARGO_CFG_TARGET_*`.
pub fn toolchain() -> Toolchain {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if arch == "wasm32" {
        Toolchain::WasiSdk { root: wasi_sdk() }
    } else if target_env == "msvc" {
        Toolchain::Msvc
    } else {
        Toolchain::Clang
    }
}

/// True when the Rust target is built with wasm atomics (the threaded web build, `-C target-feature=+atomics`): its
/// link has a shared memory, which refuses every object not compiled with atomics and bulk memory (also those of the
/// C/C++ libraries), so the C/C++ side is compiled for `wasm32-wasip1-threads` then.
pub fn wasm_threads() -> bool {
    env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("wasm32")
        && env::var("CARGO_CFG_TARGET_FEATURE").is_ok_and(|f| f.split(',').any(|x| x == "atomics"))
}

/// Triple of the wasi-sdk sysroot libraries and compiler target: the threads one has libc/libc++ built with atomics.
fn wasi_triple() -> &'static str {
    if wasm_threads() {
        "wasm32-wasip1-threads"
    } else {
        "wasm32-wasip1"
    }
}

/// Nearest ancestor whose `Cargo.toml` has a `[workspace]` section.
pub fn workspace_root() -> PathBuf {
    let manifest = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR is set by Cargo when running build.rs"),
    );
    manifest
        .ancestors()
        .find(|dir| {
            std::fs::read_to_string(dir.join("Cargo.toml"))
                .map(|s| s.lines().any(|l| l.trim() == "[workspace]"))
                .unwrap_or(false)
        })
        .unwrap_or_else(|| panic!("workspace root not found above {}", manifest.display()))
        .to_path_buf()
}

/// `var` if set, else the first existing `<workspace>/../<default_rel>` (the first one is the documented default).
fn dir_from_env(var: &str, default_rels: &[&str], what: &str) -> PathBuf {
    println!("cargo:rerun-if-env-changed={var}");
    let default_rel = default_rels[0];
    let (path, source) = match env::var_os(var) {
        Some(p) => (PathBuf::from(p), format!("from {var}")),
        None => {
            let parent = workspace_root().join("..");
            let path = default_rels
                .iter()
                .map(|r| parent.join(r))
                .find(|p| p.is_dir())
                .unwrap_or_else(|| parent.join(default_rel));
            (path, format!("default, {var} not set"))
        }
    };
    if !path.is_dir() {
        panic!(
            "{what} not found: {} ({source}). Set {var} or place it at <workspace>/../{default_rel}",
            path.display()
        );
    }
    normalize(&path)
}

/// Canonical path. Strips the Windows `\\?\` prefix, which MSVC rejects in `-I`.
fn normalize(path: &Path) -> PathBuf {
    let Ok(p) = path.canonicalize() else {
        return path.to_path_buf();
    };
    match p.to_str().and_then(|s| s.strip_prefix(r"\\?\")) {
        Some(stripped) if !stripped.starts_with("UNC\\") => PathBuf::from(stripped),
        _ => p,
    }
}

/// wasi-sdk 33 root: `WASI_SDK`, else `<workspace>/../toolchains/wasi-sdk`.
pub fn wasi_sdk() -> PathBuf {
    let root = dir_from_env("WASI_SDK", &["toolchains/wasi-sdk"], "wasi-sdk 33");
    let clang = root.join("bin").join(exe("clang++"));
    if !clang.is_file() {
        panic!(
            "wasi-sdk has no {}: directory {} does not look like wasi-sdk",
            clang.display(),
            root.display()
        );
    }
    root
}

/// Newest LLVM (22, else 18) with libc++ headers. Plain `clang++` may be a Windows binary under WSL.
fn llvm_with_libcxx() -> Option<u32> {
    [22, 18].into_iter().find(|v| {
        let root = PathBuf::from(format!("/usr/lib/llvm-{v}"));
        root.join("bin/clang++").is_file() && root.join("include/c++/v1").is_dir()
    })
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// `cc::Build` for the target and language. MSVC uses `/EHs` because Lua's `extern "C"` functions throw.
pub fn cc_build(lang: Lang) -> cc::Build {
    let mut b = cc::Build::new();
    b.cpp(lang != Lang::C);
    // The C++ stdlib is linked manually (see link_wasm_runtime).
    b.cpp_link_stdlib(None);
    match toolchain() {
        Toolchain::Msvc => {
            b.flag("/EHs");
            match lang {
                Lang::C => {}
                Lang::Cxx17 => {
                    b.flag("/std:c++17");
                }
                Lang::Cxx03 => {
                    b.flag("/std:c++14");
                }
            }
            b.flag("/utf-8");
        }
        Toolchain::Clang => {
            let use_env = |var: &str| env::var_os(var).is_some();
            let llvm = llvm_with_libcxx();
            let versioned = |name: &str| match llvm {
                Some(v) => format!("{name}-{v}"),
                None => name.to_string(),
            };
            if lang == Lang::C {
                if !use_env("CC") {
                    b.compiler(versioned("clang"));
                }
            } else {
                if !use_env("CXX") {
                    b.compiler(versioned("clang++"));
                    if let Some(v) = llvm {
                        // libc++.so of this version lives in the LLVM dir.
                        println!("cargo:rustc-link-search=native=/usr/lib/llvm-{v}/lib");
                    }
                }
                if llvm.is_some() {
                    b.cpp_set_stdlib(Some("c++"));
                } else {
                    // No libc++ headers on this machine: libstdc++ (ABI-compatible for gamecore's C API).
                    b.cpp_link_stdlib(Some("stdc++"));
                }
            }
            b.flag(std_flag(lang));
            for f in PORT_FLAGS {
                b.flag(f);
            }
        }
        Toolchain::WasiSdk { root } => {
            let bin = root.join("bin");
            let sysroot = root.join("share").join("wasi-sysroot");
            b.no_default_flags(true)
                .target(wasi_triple())
                .compiler(bin.join(exe(if lang == Lang::C { "clang" } else { "clang++" })))
                .archiver(bin.join(exe("llvm-ar")))
                .ranlib(bin.join(exe("llvm-ranlib")))
                .flag(format!("--target={}", wasi_triple()))
                .flag(format!("--sysroot={}", sysroot.display()))
                .flag("-fwasm-exceptions")
                .flag("-mllvm")
                .flag("-wasm-use-legacy-eh=false")
                .flag("-O2")
                .flag("-ffunction-sections")
                .flag("-fdata-sections")
                .flag(std_flag(lang));
            if lang != Lang::C {
                b.flag("-stdlib=libc++");
            }
            if wasm_threads() {
                // The code stays single-threaded; the features make the objects linkable into a shared memory.
                b.flag("-matomics")
                    .flag("-mbulk-memory")
                    .flag("-mmutable-globals");
            }
            for f in PORT_FLAGS {
                b.flag(f);
            }
        }
    }
    b
}

fn std_flag(lang: Lang) -> &'static str {
    match lang {
        Lang::C => "-std=c11",
        Lang::Cxx17 => "-std=c++17",
        Lang::Cxx03 => "-std=c++03",
    }
}

/// wasi-sdk runtime libraries in clang driver order. `clock()` (Lua's `os` lib) comes from
/// `wasi-emulated-process-clocks`.
pub const WASM_RUNTIME_LIBS: &[&str] = &[
    "c++",
    "c++abi",
    "unwind",
    "wasi-emulated-process-clocks",
    "c",
    "clang_rt.builtins",
];

/// Emits link directives for the wasi-sdk C/C++ runtime. No-op on other targets.
pub fn link_wasm_runtime() {
    let Toolchain::WasiSdk { root } = toolchain() else {
        return;
    };
    let lib = root
        .join("share")
        .join("wasi-sysroot")
        .join("lib")
        .join(wasi_triple());
    let builtins = find_builtins(&root);
    for dir in [lib.join("eh"), lib, builtins] {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    for l in WASM_RUNTIME_LIBS {
        println!("cargo:rustc-link-lib=static:-bundle={l}");
    }
}

fn builtins_dir() -> &'static str {
    if wasm_threads() {
        "wasm32-unknown-wasip1-threads"
    } else {
        "wasm32-unknown-wasip1"
    }
}

/// `libclang_rt.builtins.a` under `lib/clang/<version>/lib/wasm32-unknown-wasip1`.
fn find_builtins(root: &Path) -> PathBuf {
    let clang_dir = root.join("lib").join("clang");
    let found = std::fs::read_dir(&clang_dir).ok().and_then(|it| {
        it.filter_map(Result::ok)
            .map(|e| e.path().join("lib").join(builtins_dir()))
            .find(|p| p.join("libclang_rt.builtins.a").is_file())
    });
    found.unwrap_or_else(|| {
        panic!(
            "libclang_rt.builtins.a not found in wasi-sdk (expected {}/<version>/lib/{})",
            clang_dir.display(),
            builtins_dir()
        )
    })
}

/// `rerun-if-changed` for every file in the directory, recursively.
pub fn rerun_if_dir_changed(dir: &Path) {
    println!("cargo:rerun-if-changed={}", dir.display());
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(Result::ok) {
            let p = e.path();
            if p.is_dir() {
                rerun_if_dir_changed(&p);
            } else {
                println!("cargo:rerun-if-changed={}", p.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_flags_match_lekmod_port() {
        assert_eq!(
            PORT_FLAGS,
            [
                "-ffp-contract=off",
                "-fno-fast-math",
                "-fwrapv",
                "-fno-strict-aliasing"
            ]
        );
    }

    #[test]
    fn eh_runtime_is_linked() {
        for l in ["c++", "c++abi", "unwind", "c", "clang_rt.builtins"] {
            assert!(WASM_RUNTIME_LIBS.contains(&l), "{l}");
        }
    }
}
