//! Native spike selftest passes.

#[test]
fn selftest_passes() {
    let report = gamecore::selftest();
    assert!(report.passed(), "{report}");
    for name in [
        "cpp_global_ctor",
        "cpp_throw_catch",
        "cpp_stl",
        "sqlite_memory",
        "rust_cpp_lua_rust_cpp",
        "lua_error_from_cpp",
        "rust_error_to_lua",
    ] {
        assert!(
            report.cases.iter().any(|c| c.name == name),
            "no case named {name}:\n{report}"
        );
    }
    println!("{report}");
    assert!(
        report.cases.iter().any(|c| c.name == "implementation"),
        "{report}"
    );
    assert!(
        report.cases.iter().any(|c| c.name == "game_pump"),
        "{report}"
    );
    let again = gamecore::selftest();
    assert!(again.passed(), "{again}");
}
