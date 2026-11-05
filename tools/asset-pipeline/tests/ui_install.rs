//! The UI file system against a real install. Runs only when `CIV5_ROOT` is set.

use asset_pipeline::ui::{UiOptions, open_vfs};

#[test]
fn layers_follow_civ5pkg_priority() {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        eprintln!("CIV5_ROOT not set: skipped");
        return;
    };
    let vfs = open_vfs(std::path::Path::new(&root), &UiOptions::default()).unwrap();
    let prio = |n: &str| vfs.layers.iter().find(|l| l.name == n).map(|l| l.priority);
    assert_eq!(prio("Expansion2"), Some(200));
    assert_eq!(prio("UI_bc1"), Some(250));
    assert_eq!(prio("LEKMOD_v34.15"), Some(300));
    // Increasing along the vector: the override order.
    assert!(
        vfs.layers
            .windows(2)
            .all(|w| w[0].priority <= w[1].priority)
    );
    // The visible TopPanel comes from the highest layer that has it (Lekmod), EUI and BNW below.
    let names: Vec<_> = vfs
        .copies("toppanel.lua")
        .iter()
        .map(|e| vfs.layer(e).name.clone())
        .collect();
    assert_eq!(names.last().map(String::as_str), Some("LEKMOD_v34.15"));
    assert!(names.iter().any(|n| n == "UI_bc1") && names.iter().any(|n| n == "Expansion2"));
    // Scenario copies of UI files do not take part.
    assert!(vfs.skipped_scenario_files > 0);
    // The Override folder is left out by default.
    assert!(!vfs.override_files.is_empty());
    assert!(
        vfs.copies("civ5units_expansion.xml")
            .iter()
            .all(|e| vfs.layer(e).name != "LEKMOD_v34.15")
    );
}
