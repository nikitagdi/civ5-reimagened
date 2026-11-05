//! Builds the vendored SQLite amalgamation (C) for native and wasm (wasi-sdk).

use build_support::{Lang, Toolchain};

const SQLITE_DIR: &str = "third_party/sqlite-3.53.2";

fn main() {
    let dir = build_support::workspace_root().join(SQLITE_DIR);
    build_support::rerun_if_dir_changed(&dir);

    let mut b = build_support::cc_build(Lang::C);
    b.warnings(false)
        .include(&dir)
        .file(dir.join("sqlite3.c"))
        .define("SQLITE_CORE", None)
        .define("SQLITE_OMIT_LOAD_EXTENSION", None)
        .define("SQLITE_OMIT_DEPRECATED", None)
        .define("SQLITE_DQS", "0");
    match build_support::toolchain() {
        Toolchain::WasiSdk { .. } => {
            // No threads, no files: temp tables and journals stay in memory.
            b.define("SQLITE_THREADSAFE", "0")
                .define("SQLITE_TEMP_STORE", "3")
                .define("SQLITE_OMIT_WAL", None)
                .define("LONGDOUBLE_TYPE", "double")
                .define("_WASI_EMULATED_PROCESS_CLOCKS", None)
                .define("_WASI_EMULATED_GETPID", None)
                .define("_WASI_EMULATED_MMAN", None)
                .define("_WASI_EMULATED_SIGNAL", None);
        }
        Toolchain::Msvc => {
            b.define("SQLITE_THREADSAFE", "1");
        }
        Toolchain::Clang => {
            b.define("SQLITE_THREADSAFE", "1")
                .define("HAVE_USLEEP", "1")
                .define("HAVE_LOCALTIME_R", "1");
        }
    }
    b.compile("sqlite3");
    build_support::link_wasm_runtime();

    println!("cargo:include={}", dir.display());
}
