//! The first slice of the v1 schema: roots `meta`, `game`, `top`, `research`.

use ui2_model::{RootKind, Scalar, Schema, Type};

/// Every path that the schema spec lists for these roots, with the type it must have.
const PATHS: &[(&str, &str)] = &[
    ("meta.schema", "str"),
    ("meta.source", "Source"),
    ("meta.live", "bool"),
    ("meta.catching_up", "bool"),
    ("meta.connected", "bool"),
    ("game.turn", "i32"),
    ("game.year", "i32"),
    ("game.year_text", "text"),
    ("game.era", "EraRef"),
    ("game.era.id", "i16"),
    ("game.era.name", "text"),
    ("game.me", "key<player>"),
    ("game.phase", "Phase"),
    ("game.mode", "TurnMode"),
    ("game.winner", "key<player>?"),
    ("top.science_rate", "i32"),
    ("top.gold", "i32"),
    ("top.gold_rate", "i32"),
    ("top.happiness", "i32"),
    ("top.mood", "Mood"),
    ("top.golden_age", "GoldenAge"),
    ("top.golden_age.active", "bool"),
    ("top.golden_age.turns", "i16"),
    ("top.golden_age.progress", "i32"),
    ("top.golden_age.threshold", "i32"),
    ("top.culture", "i32"),
    ("top.culture_rate", "i32"),
    ("top.policy_cost", "i32"),
    ("top.policy_turns", "i16?"),
    ("top.faith", "i32"),
    ("top.faith_rate", "i32"),
    ("top.tourism", "i32?"),
    ("top.resources", "list<Resource>"),
    ("top.resources[].id", "key<item>"),
    ("top.resources[].name", "text"),
    ("top.resources[].icon", "asset"),
    ("top.resources[].available", "i32"),
    ("top.resources[].used", "i32"),
    ("top.resources[].total", "i32"),
    ("top.policies", "list<PolicyBranch>"),
    ("top.policies[].id", "i16"),
    ("top.policies[].name", "text"),
    ("top.policies[].unlocked", "bool"),
    ("top.policies[].owned", "i8"),
    ("top.policies[].total", "i8"),
    ("research.current", "Tech?"),
    ("research.current.id", "key<tech>"),
    ("research.current.name", "text"),
    ("research.current.icon", "asset"),
    ("research.current.turns", "i16?"),
    ("research.current.progress", "i32"),
    ("research.current.cost", "i32"),
    ("research.current.fraction", "f32"),
    ("research.next", "Tech?"),
    ("research.next.fraction", "f32"),
    ("research.queue", "list<Tech>"),
    ("research.queue[].id", "key<tech>"),
    ("research.queue[].turns", "i16?"),
    ("research.options", "list<Tech>"),
    ("research.options[].cost", "i32"),
    ("research.known", "bool"),
    ("research.blocked", "bool"),
];

#[test]
fn all_listed_paths_resolve_with_their_types() {
    let s = Schema::v1();
    for (path, ty) in PATHS {
        let r = s
            .resolve(path)
            .unwrap_or_else(|e| panic!("{path} does not resolve: {e}"));
        assert_eq!(s.type_name(&r.ty), *ty, "{path}");
    }
}

#[test]
fn roots_owners_and_enums() {
    let s = Schema::v1();
    let names: Vec<&str> = s.roots().iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["meta", "game", "top", "research"]);
    assert!(s.roots().iter().all(|r| r.kind == RootKind::Data));
    let e = |n: &str| s.enums().iter().find(|e| e.name == n).unwrap();
    assert_eq!(
        e("Phase").variants,
        ["starting", "ai", "humans", "game_over"]
    );
    assert_eq!(e("TurnMode").variants, ["simultaneous", "hybrid"]);
    assert_eq!(e("Mood").variants, ["content", "unhappy", "very_unhappy"]);
    assert_eq!(e("Source").variants, ["replica", "server"]);
    // The list keys of the schema.
    let key = |p: &str| s.path(s.resolve(p).unwrap().id).list_key.clone();
    assert_eq!(key("top.resources").as_deref(), Some("id"));
    assert_eq!(key("research.queue").as_deref(), Some("id"));
    // No field is writable from JS outside `ui`.
    assert!(s.paths().iter().all(|p| !p.js));
    assert!(matches!(
        s.resolve("top.gold").unwrap().ty,
        Type::Scalar(Scalar::I32)
    ));
}
