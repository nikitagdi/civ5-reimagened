//! The replica is never changed by the UI v2 source or the actions module: its digest, its batch counters and its
//! last applied batch are the same after the model was rebuilt from scratch many times and every intent of the
//! catalogue went through the actions module with a recording sink (the steps are in `r17/scenario.rs`, shared with
//! the browser run `cargo xtask web-selftest --scenario ui2-r17`). A control batch of the fixture, applied through
//! the replica's own entry points, must be seen by the same comparison.
//!
//! It runs on the real core (Linux/WSL, `GAMECORE_LIB_DIR_x86_64`) with the fixture of the replica test, written by
//! `REPLICA_FIXTURE_OUT=<dir> cargo test -p replica --test real_replica -- --ignored small`.
//! `UI2_R17_FIXTURE=<dir>/small` names the scenario directory (any scenario with a journal batch works);
//! `UI2_R17_SEAT=<n>` joins as another seat. Without `UI2_R17_FIXTURE` the test prints `skipped` and passes.
//!
//! ```text
//! UI2_R17_FIXTURE=<dir>/small cargo test -p ui2-source --test r17_replica_untouched -- --nocapture
//! ```

use std::path::Path;

#[path = "r17/scenario.rs"]
mod scenario;

/// The fixture's files by the names its `list.json` gives.
fn load(dir: &Path) -> scenario::Files {
    let list = std::fs::read(dir.join("list.json"))
        .unwrap_or_else(|e| panic!("{}: no list.json: {e}", dir.display()));
    let names: Vec<String> = serde_json::from_slice(&list).expect("list.json");
    names
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(dir.join(&n)).unwrap_or_else(|e| panic!("fixture: {n}: {e}"));
            (n, bytes)
        })
        .collect()
}

#[test]
fn the_ui_never_changes_the_replica() {
    let Some(dir) = std::env::var_os("UI2_R17_FIXTURE") else {
        eprintln!(
            "skipped: UI2_R17_FIXTURE=<fixture dir>/<scenario> is not set (it needs the real core linked)"
        );
        return;
    };
    let seat = std::env::var("UI2_R17_SEAT")
        .ok()
        .and_then(|s| s.parse().ok());
    let files = load(Path::new(&dir));
    let report = scenario::run(&files, seat).unwrap_or_else(|e| panic!("R17: {e}"));
    println!("UI2R17 {}", report.to_json());
    assert!(report.reads.getter_calls > 0);
    assert!(!report.control.is_empty());
}

/// The comparison itself, without a core: a changed digest, batch count or last batch is reported, an equal mark
/// is not.
#[test]
fn the_comparison_sees_each_watched_value() {
    let base = scenario::Mark {
        digest: 0x1234,
        applied_jseq: 10,
        generation: 1,
        state: replica::State::Live,
        stats: replica::ReplicaStats {
            applied: 10,
            ..Default::default()
        },
    };
    assert!(scenario::untouched(&base, &base.clone()).is_ok());
    let mut d = base.clone();
    d.digest = 0x1235;
    assert!(
        scenario::untouched(&base, &d)
            .unwrap_err()
            .contains("digest")
    );
    let mut a = base.clone();
    a.stats.applied = 11;
    assert!(
        scenario::untouched(&base, &a)
            .unwrap_err()
            .contains("applied")
    );
    let mut j = base.clone();
    j.applied_jseq = 11;
    assert!(
        scenario::untouched(&base, &j)
            .unwrap_err()
            .contains("applied_jseq")
    );
    let mut g = base.clone();
    g.generation = 2;
    assert!(scenario::untouched(&base, &g).is_err());
    let mut n = base.clone();
    n.stats.net_sent = 1;
    assert!(
        scenario::untouched(&base, &n)
            .unwrap_err()
            .contains("counters")
    );
}
