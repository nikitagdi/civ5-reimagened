//! The v1 schema: every root, path, enum and intent of the model.

use ui2_model::{RootKind, RootShape, Scalar, Schema, Type};

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
    // players
    ("players", "list<PlayerRow>"),
    ("players[].id", "key<player>"),
    ("players[].leader", "str"),
    ("players[].civ", "str"),
    ("players[].nick", "str?"),
    ("players[].color", "color"),
    ("players[].color2", "color"),
    ("players[].human", "bool"),
    ("players[].alive", "bool"),
    ("players[].met", "bool"),
    ("players[].at_war", "bool"),
    ("players[].me", "bool"),
    // turn
    ("turn.state", "TurnUi"),
    ("turn.blocker", "Blocker?"),
    ("turn.blocker.kind", "i8"),
    ("turn.blocker.unit", "key<unit>?"),
    ("turn.blocker.city", "key<city>?"),
    ("turn.blocker.plot", "Plot?"),
    ("turn.blocker.plot.x", "i16"),
    ("turn.units_waiting", "i32"),
    ("turn.finished", "bool"),
    ("turn.can_cancel", "bool"),
    ("turn.timer", "Timer?"),
    ("turn.timer.left_s", "i32"),
    ("turn.timer.total_s", "i32"),
    ("turn.timer.stopped", "bool"),
    ("turn.ready", "i32"),
    ("turn.humans", "i32"),
    ("turn.seats", "list<Seat>"),
    ("turn.seats[].player", "key<player>"),
    ("turn.seats[].name", "str"),
    ("turn.seats[].me", "bool"),
    ("turn.seats[].finished", "bool"),
    ("turn.seats[].may_act", "bool"),
    ("turn.seats[].online", "bool"),
    // unit
    ("unit", "Unit?"),
    ("unit.key", "key<unit>"),
    ("unit.name", "str"),
    ("unit.type_name", "text"),
    ("unit.portrait", "asset"),
    ("unit.level", "i16"),
    ("unit.xp", "i16"),
    ("unit.strength", "i16"),
    ("unit.ranged", "i16"),
    ("unit.range", "i16"),
    ("unit.moves", "f32"),
    ("unit.max_moves", "i16"),
    ("unit.hp", "i16"),
    ("unit.max_hp", "i16"),
    ("unit.hp_fraction", "f32"),
    ("unit.domain", "Domain"),
    ("unit.activity", "Activity"),
    ("unit.promotions", "list<Promo>"),
    ("unit.promotions[].id", "i16"),
    ("unit.promotions[].name", "text"),
    ("unit.promotions[].icon", "asset"),
    ("unit.promotion_ready", "bool"),
    ("unit.actions", "list<UnitActionRow>"),
    ("unit.actions[].kind", "UnitActionKind"),
    ("unit.actions[].enabled", "bool"),
    ("unit.actions[].reason", "str?"),
    ("unit.actions[].supported", "bool"),
    ("unit.act", "UnitActs"),
    ("unit.act.fortify", "Act"),
    ("unit.act.fortify.shown", "bool"),
    ("unit.act.fortify.enabled", "bool"),
    ("unit.act.fortify.reason", "str?"),
    ("unit.act.found_city.shown", "bool"),
    ("unit.act.alert.enabled", "bool"),
    ("unit.pending", "bool"),
    // map
    ("map.cities", "list<CityBanner>"),
    ("map.cities[].key", "key<city>"),
    ("map.cities[].owner", "key<player>"),
    ("map.cities[].color", "color"),
    ("map.cities[].color2", "color"),
    ("map.cities[].name", "str"),
    ("map.cities[].plot", "Plot"),
    ("map.cities[].plot.y", "i16"),
    ("map.cities[].size", "i16"),
    ("map.cities[].capital", "bool"),
    ("map.cities[].mine", "bool"),
    ("map.cities[].visible", "bool"),
    ("map.cities[].puppet", "bool"),
    ("map.cities[].razing", "bool"),
    ("map.cities[].occupied", "bool"),
    ("map.cities[].resistance", "bool"),
    ("map.cities[].can_shoot", "bool"),
    ("map.cities[].strength", "i16"),
    ("map.cities[].hp_fraction", "f32"),
    ("map.cities[].growth_turns", "i16?"),
    ("map.cities[].production", "BannerProd?"),
    ("map.cities[].production.item", "key<item>"),
    ("map.cities[].production.name", "text"),
    ("map.cities[].production.icon", "asset"),
    ("map.cities[].production.turns", "i16?"),
    ("map.cities[].production.fraction", "f32"),
    ("map.units", "list<UnitFlag>"),
    ("map.units[].key", "key<unit>"),
    ("map.units[].owner", "key<player>"),
    ("map.units[].color", "color"),
    ("map.units[].color2", "color"),
    ("map.units[].plot", "Plot"),
    ("map.units[].icon", "asset"),
    ("map.units[].hp_fraction", "f32"),
    ("map.units[].domain", "Domain"),
    ("map.units[].mine", "bool"),
    ("map.units[].waiting", "bool"),
    ("map.units[].at_war", "bool"),
    ("map.units[].cargo", "bool"),
    // notes and chat
    ("notes", "list<Note>"),
    ("notes[].id", "key<note>"),
    ("notes[].category", "NoteCat"),
    ("notes[].message", "str"),
    ("notes[].summary", "str"),
    ("notes[].turn", "i16"),
    ("notes[].plot", "Plot?"),
    ("chat", "list<ChatRow>"),
    ("chat[].seq", "key<chat>"),
    ("chat[].from", "str"),
    ("chat[].from_player", "key<player>?"),
    ("chat[].to", "ChatTo"),
    ("chat[].text", "str"),
    ("chat[].turn", "i32"),
    ("chat[].mine", "bool"),
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
    assert_eq!(
        names,
        [
            "meta", "game", "top", "research", "players", "turn", "unit", "map", "notes", "chat"
        ]
    );
    assert!(s.roots().iter().all(|r| r.kind == RootKind::Data));
    let shape = |n: &str| s.root(s.root_by_name(n).unwrap()).shape.clone();
    assert_eq!(shape("top"), RootShape::Struct);
    assert_eq!(shape("unit"), RootShape::Optional);
    for n in ["players", "notes", "chat"] {
        assert!(matches!(shape(n), RootShape::List(_)), "{n}");
    }
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
    assert_eq!(key("players").as_deref(), Some("id"));
    assert_eq!(key("notes").as_deref(), Some("id"));
    assert_eq!(key("chat").as_deref(), Some("seq"));
    assert_eq!(key("map.cities").as_deref(), Some("key"));
    assert_eq!(key("unit.actions").as_deref(), Some("kind"));
    assert!(matches!(
        s.resolve("top.gold").unwrap().ty,
        Type::Scalar(Scalar::I32)
    ));
}

fn variants(s: &Schema, name: &str) -> Vec<String> {
    s.enums()
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no enum {name}"))
        .variants
        .clone()
}

#[test]
fn enums_of_the_model() {
    let s = Schema::v1();
    let eq = |n: &str, want: &[&str]| assert_eq!(variants(s, n), want, "{n}");
    eq(
        "TurnUi",
        &[
            "next_turn",
            "unit_needs_orders",
            "city_needs_production",
            "research_needed",
            "policy_needed",
            "other_blocker",
            "waiting_others",
            "ai_moving",
            "ended",
            "game_over",
        ],
    );
    eq(
        "UnitActionKind",
        &[
            "move",
            "range_attack",
            "found_city",
            "fortify",
            "sleep",
            "skip",
            "heal",
            "automate_build",
            "automate_explore",
            "cancel",
            "disband",
            "wake",
            "alert",
            "pillage",
            "upgrade",
        ],
    );
    eq(
        "NoteCat",
        &[
            "growth",
            "great_people",
            "diplomacy",
            "military",
            "production",
            "research",
            "policy",
            "religion",
            "other",
        ],
    );
    eq("ChatTo", &["all", "player"]);
}

#[test]
fn unit_acts_has_one_field_per_action() {
    let s = Schema::v1();
    let kinds = variants(s, "UnitActionKind");
    let acts = s.types().iter().find(|t| t.name == "UnitActs").unwrap();
    let names: Vec<&str> = acts.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, kinds);
    for k in &kinds {
        let r = s.resolve(&format!("unit.act.{k}")).unwrap();
        assert_eq!(s.type_name(&r.ty), "Act");
    }
}
