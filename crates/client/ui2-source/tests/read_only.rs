//! The source never writes into the replica. The builders only ever get `&mut dyn GameRead` (no setters exist on
//! it); this test also scans the crate's sources for the replica's writing entry points, so that a later edit cannot
//! reach them through `ReplicaView::of`'s `&mut Replica`.

const SOURCES: &[(&str, &str)] = &[
    ("lib.rs", include_str!("../src/lib.rs")),
    ("trigger.rs", include_str!("../src/trigger.rs")),
    ("catalog.rs", include_str!("../src/catalog.rs")),
    ("build/mod.rs", include_str!("../src/build/mod.rs")),
    ("build/game.rs", include_str!("../src/build/game.rs")),
    ("build/top.rs", include_str!("../src/build/top.rs")),
];

/// Calls that move the game or the replica machine, or steal a queue another consumer owns.
const FORBIDDEN: &[&str] = &[
    "engine_mut",
    "post_net",
    "on_msg",
    ".pump(",
    ".blob(",
    "blob_failed",
    "ui_op",
    "setup_graphical",
    "attach_lua",
    "invalidate_lua",
    "subscribe_game_event",
    "take_events",
    "take_outgoing",
    "unsend",
    "set_turn_sent",
    "set_turn_locked",
    "take_turn_reopened",
    "note_input",
    "config_mut",
    "set_clock",
    "set_seat",
    "plots_dirty",
    "take_change",
];

#[test]
fn the_source_calls_no_writer_of_the_replica() {
    for (file, text) in SOURCES {
        for word in FORBIDDEN {
            assert!(
                !text.contains(word),
                "{file} mentions `{word}`: the UI source must only read the replica"
            );
        }
    }
}
