//! The source never writes into the replica or the session. The builders only ever get `&mut dyn GameRead` (no
//! setters exist on it) and `&dyn SessionRead` (getters on `&self` only); this test also scans the crate's sources
//! for the writing entry points of the replica and of the client core, so that a later edit cannot reach them
//! through `ReplicaView::of`'s `&mut Replica` or the `ClientCore` the session trait is implemented for.

const SOURCES: &[(&str, &str)] = &[
    ("lib.rs", include_str!("../src/lib.rs")),
    ("trigger.rs", include_str!("../src/trigger.rs")),
    ("catalog.rs", include_str!("../src/catalog.rs")),
    ("cost.rs", include_str!("../src/cost.rs")),
    ("session.rs", include_str!("../src/session.rs")),
    ("static_source.rs", include_str!("../src/static_source.rs")),
    ("pending.rs", include_str!("../src/pending.rs")),
    ("build/mod.rs", include_str!("../src/build/mod.rs")),
    ("build/game.rs", include_str!("../src/build/game.rs")),
    ("build/top.rs", include_str!("../src/build/top.rs")),
    ("build/players.rs", include_str!("../src/build/players.rs")),
    ("build/turn.rs", include_str!("../src/build/turn.rs")),
    ("build/map.rs", include_str!("../src/build/map.rs")),
    ("build/notes.rs", include_str!("../src/build/notes.rs")),
    ("build/chat.rs", include_str!("../src/build/chat.rs")),
    ("build/unit.rs", include_str!("../src/build/unit.rs")),
    (
        "build/production.rs",
        include_str!("../src/build/production.rs"),
    ),
];

/// Calls that move the game or the replica machine, or steal a queue another consumer owns.
const FORBIDDEN_REPLICA: &[&str] = &[
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
];

/// Calls of the client core that send, change its state or take a queue (the renderer owns `take_change`).
const FORBIDDEN_SESSION: &[&str] = &[
    "take_change",
    ".order(",
    ".order_at(",
    ".chat(",
    ".game_ctl(",
    ".send_gc_net",
    ".send_gc_resync",
    ".send_mod(",
    ".set_focus(",
    ".query(",
    ".lobby(",
    ".login(",
    ".sign_out(",
    ".set_token(",
    ".save_account(",
    ".set_catalog(",
    ".connect(",
    ".on_connected(",
    ".on_bytes(",
    ".on_disconnected(",
    ".host_event(",
    ".tick(",
    ".poll_outgoing(",
    ".poll_socket_cmd(",
    ".poll_event(",
    ".retry(",
    ".use_this_tab(",
    ".abandon_start(",
    ".dismiss_notice(",
    "debug_corrupt_view",
    "debug_bad_message",
    "&mut ClientCore",
    "&mut self) ->",
];

#[test]
fn the_source_calls_no_writer_of_the_replica_or_the_session() {
    for (file, text) in SOURCES {
        for word in FORBIDDEN_REPLICA.iter().chain(FORBIDDEN_SESSION) {
            // `&mut self) ->` is only a writer inside the session trait.
            if *word == "&mut self) ->" && *file != "session.rs" {
                continue;
            }
            assert!(
                !text.contains(word),
                "{file} mentions `{word}`: the UI source must only read the replica and the session"
            );
        }
    }
}

#[test]
fn every_source_file_is_scanned() {
    let listed: Vec<&str> = SOURCES.iter().map(|(f, _)| *f).collect();
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
    let mut found = Vec::new();
    collect(std::path::Path::new(dir), "", &mut found);
    for f in &found {
        assert!(listed.contains(&f.as_str()), "{f} is not in the scan list");
    }
}

fn collect(dir: &std::path::Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if path.is_dir() {
            collect(&path, &format!("{prefix}{name}/"), out);
        } else if name.ends_with(".rs") {
            out.push(format!("{prefix}{name}"));
        }
    }
}
