//! HUD panels through the real server core; stub builds explicitly skip these proofs.
use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{ModelStore, Sample, Schema, generated::Hud};

#[path = "common/real.rs"]
mod common;

fn start() -> Option<common::H> {
    let mut h = start_unfounded()?;
    h.found_city(0);
    Some(h)
}

fn start_unfounded() -> Option<common::H> {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return None;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    Some(common::H::with(worker, outs))
}

fn text(t: &ui2_model::Text) -> String {
    match t {
        ui2_model::Text::Raw(s) => s.to_string(),
        ui2_model::Text::Key { key, .. } => key.to_string(),
    }
}

fn hud(h: &mut common::H, id: u64, args: Vec<(String, i64)>) -> Hud {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: id,
        q: Query::Screen {
            screen: "hud".into(),
            args,
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid, answer } if *qid == id => Some(answer),
            _ => None,
        })
        .expect("HUD answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("HUD query did not return screen data")
    };
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let sample = Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"hud":value}}).to_string(),
    )
    .unwrap();
    let mut store = ModelStore::new(Schema::v1());
    sample.apply(&mut store).unwrap();
    store.get::<Option<Hud>>().unwrap().clone().unwrap()
}

#[test]
fn top_panel_sources_are_well_formed_after_founding() {
    let Some(mut h) = start() else { return };
    let top = hud(&mut h, 1, vec![]).top;
    assert_eq!(top.tips.len(), 7);
    assert!(
        top.tips
            .iter()
            .all(|t| !t.lines.is_empty() && t.total.is_finite())
    );
    assert!(top.resources.iter().all(|r| !r.name.is_empty()));
    assert!(top.trade_available >= top.trade_used);
    // Top-panel actions open local windows and emit no game mutation.
}

/// The top strip's tooltips are the original top panel's lines: every text key resolved, no markup left, the
/// colours as tones, and the strategic resource tooltip with what uses the resource.
#[test]
fn top_panel_tooltips_are_the_original_lines() {
    let Some(mut h) = start() else { return };
    // Test-only bridge: the seat knows iron and may trade it, so its tooltip has the local and the usage parts.
    let lua = lua_sys::Lua::new().unwrap();
    // The screens' own attach: the core's Lua and the server localization.
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    lua.exec(
        r#"
        local r = GameInfo.Resources.RESOURCE_IRON
        local team = Teams[Players[0]:GetTeam()]
        for _, tech in ipairs({r.TechReveal, r.TechCityTrade}) do
            if tech then team:SetHasTech(GameInfoTypes[tech], true) end
        end
        iron_id = r.ID
        used_label = Locale.ConvertTextKey("TXT_KEY_PEDIA_REQ_RESRC_LABEL")
        expenses = Locale.ConvertTextKey("TXT_KEY_TP_TOTAL_EXPENSES", Players[0]:CalculateUnitCost() + Players[0]:CalculateUnitSupply()
            + Players[0]:GetBuildingGoldMaintenance() + Players[0]:GetImprovementGoldMaintenance())
        -- The seat's own swordsman (a civilization may replace it with its unique unit).
        local civ, unit = GameInfo.Civilizations[Players[0]:GetCivilizationType()].Type, GameInfo.Units.UNIT_SWORDSMAN
        for o in GameInfo.Civilization_UnitClassOverrides{CivilizationType = civ, UnitClassType = "UNITCLASS_SWORDSMAN"} do
            if o.UnitType then unit = GameInfo.Units[o.UnitType] end
        end
        swords = Locale.ConvertTextKey(unit.Description)
    "#,
        "=iron-known",
    )
    .unwrap();
    let get = |name: &str| -> String { lua.globals().get(name).unwrap() };
    let (used_label, expenses, swords) = (get("used_label"), get("expenses"), get("swords"));
    let iron: i32 = lua.globals().get("iron_id").unwrap();
    let top = hud(&mut h, 1, vec![]).top;
    let clean =
        |text: &str| !text.contains("TXT_KEY_") && !text.contains('[') && !text.contains(']');
    for tip in top.tips.iter() {
        assert!(!tip.text.is_empty(), "{:?} has lines", tip.kind);
        for row in tip.text.iter() {
            assert!(clean(&text(&row.text)), "{:?}: {:?}", tip.kind, row.text);
        }
    }
    let gold = top
        .tips
        .iter()
        .find(|t| t.kind == ui2_model::generated::HudTopTipKind::Gold)
        .unwrap();
    let plain = |s: &str| {
        let mut out = String::new();
        let mut inside = false;
        for c in s.chars() {
            match c {
                '[' => inside = true,
                ']' => inside = false,
                c if !inside => out.push(c),
                _ => {}
            }
        }
        out.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let row = gold
        .text
        .iter()
        .find(|r| text(&r.text) == plain(&expenses))
        .unwrap_or_else(|| panic!("expenses line {expenses:?} in {:?}", gold.text));
    assert_eq!(row.tone, ui2_model::generated::HudTopTone::Negative);
    assert!(
        gold.text.iter().any(|r| r.bullet),
        "income and expenses are bullets"
    );
    let resource = top
        .resources
        .iter()
        .find(|r| i32::from(r.id) == iron)
        .expect("iron in the top strip");
    let lines: Vec<String> = resource.tip.iter().map(|r| text(&r.text)).collect();
    assert!(lines.iter().all(|l| clean(l)), "{lines:?}");
    assert!(
        lines.iter().any(|l| l.ends_with(&plain(&used_label))),
        "the usage part: {lines:?}"
    );
    assert!(
        resource
            .tip
            .iter()
            .any(|r| r.bullet && text(&r.text).contains(&swords)),
        "a unit that uses iron: {lines:?}"
    );
    // Luxuries and bonus resources have no tooltip of their own.
    assert!(
        top.resources
            .iter()
            .all(|r| r.usage == 1 || r.tip.is_empty())
    );
}

#[test]
fn end_turn_reports_queue_and_uses_the_session_handshake() {
    let Some(mut h) = start() else { return };
    let turn = hud(&mut h, 1, vec![]).turn;
    assert!(!turn.queue.is_empty());
    assert!(turn.queue.iter().all(|p| p.known || p.team == -1));
    let outcome = h.cmd(0, protocol::Command::EndTurn { force: false });
    match outcome {
        protocol::CommandOutcome::Applied => {}
        protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
        _ => panic!("End-turn handshake did not return a final outcome"),
    }
}

#[test]
fn unit_panel_catalog_and_text_rename_change_the_owned_unit() {
    let Some(mut h) = start() else { return };
    let world = h.w.fresh_view(0).unwrap();
    let id = world
        .units
        .iter()
        .find(|u| u.owner == 0)
        .expect("owned unit")
        .id;
    let root = hud(&mut h, 1, vec![("unit".into(), i64::from(id))]);
    assert!(!root.unit.unwrap().catalog.is_empty());
    assert_eq!(
        h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "hud".into(),
                action: "rename".into(),
                args: vec![("a".into(), i64::from(id))],
                text: Some("Test Guard".into())
            }
        ),
        protocol::CommandOutcome::Applied
    );
    assert_eq!(
        hud(&mut h, 2, vec![("unit".into(), i64::from(id))])
            .unit
            .unwrap()
            .name,
        ui2_model::Text::raw("Test Guard (Warrior)")
    );
    let root = hud(&mut h, 3, vec![("unit".into(), i64::from(id))]);
    for (index, a) in root.unit.unwrap().catalog.iter().enumerate() {
        if !a.client.is_empty() {
            continue;
        }
        let before = h.w.fresh_view(0).unwrap();
        let qid = 10 + index as u64 * 2;
        let before_hud = hud(&mut h, qid, vec![]);
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "hud".into(),
                action: "do_action".into(),
                args: vec![("a".into(), i64::from(id)), ("b".into(), i64::from(a.id))],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Applied => {
                let after = h.w.fresh_view(0).unwrap();
                let after_hud = hud(&mut h, qid + 1, vec![]);
                assert!(
                    before != after || before_hud != after_hud,
                    "Accepted unit action must change game or HUD facts"
                );
            }
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            _ => panic!("Unit action did not return a final outcome"),
        }
    }
}

#[test]
fn unit_lists_agree_with_the_owned_world_after_founding() {
    let Some(mut h) = start() else { return };
    let lists = hud(&mut h, 1, vec![]).lists;
    let world = h.w.fresh_view(0).unwrap();
    let owned = world.units.iter().filter(|u| u.owner == 0).count();
    assert_eq!(lists.units.len(), owned);
    assert!(lists.units.iter().all(|u| u.hp >= 0 && u.moves >= 0.0));
    for g in lists.great_people.iter() {
        assert!(
            g.rows
                .iter()
                .all(|r| r.per_turn > 0.0 && r.threshold > 0 && r.turns.is_some())
        );
    }
    // Lists use client selection/camera/window intents; they do not mutate the core.
}

/// The unit list shows the original status text (a sleeping unit reads as the game's "sleep" mission) and puts
/// combat units and civilians in their own stacks.
#[test]
fn unit_list_status_is_the_original_text() {
    let Some(mut h) = start_unfounded() else {
        return;
    };
    let lua = lua_sys::Lua::new().unwrap();
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    lua.exec(
        r#"
        sleep_text = Locale.ConvertTextKey("TXT_KEY_MISSION_SLEEP")
        for u in Players[0]:Units() do
            if u:IsCombatUnit() then warrior = u:GetID() else civilian = u:GetID() end
        end
    "#,
        "=list-status",
    )
    .unwrap();
    let sleep: String = lua.globals().get("sleep_text").unwrap();
    let warrior: i32 = lua.globals().get("warrior").unwrap();
    let civilian: i32 = lua.globals().get("civilian").unwrap();
    let before = hud(&mut h, 1, vec![]).lists;
    let row = |lists: &ui2_model::generated::HudLists, id: i32| {
        lists.units.iter().find(|u| u.id == id).cloned().unwrap()
    };
    assert_eq!(
        &*row(&before, warrior).status_text,
        "",
        "an idle unit shows no status"
    );
    assert!(row(&before, warrior).military && !row(&before, civilian).military);
    assert_eq!(
        h.cmd(0, protocol::Command::UnitSleep { unit: civilian }),
        protocol::CommandOutcome::Applied
    );
    let after = hud(&mut h, 2, vec![]).lists;
    // A civilian sleeps (combat units fortify instead).
    assert_eq!(&*row(&after, civilian).status_text, sleep.as_str());
}

#[test]
fn minimap_summary_is_well_formed_for_a_new_seat() {
    let Some(mut h) = start() else { return };
    let root = hud(&mut h, 1, vec![]).minimap;
    assert!(!root.observer);
    assert!(root.religion.is_none() && root.beliefs.is_empty());
    // Minimap clicks move the client camera; renderer and session options never mutate the game core.
}

#[test]
fn on_map_tooltips_include_owned_entities_after_founding() {
    let Some(mut h) = start() else { return };
    let map = hud(&mut h, 1, vec![]).map;
    assert!(!map.cities.is_empty() && !map.units.is_empty());
    assert!(map.units.iter().all(|u| u.hp <= u.hp_max && u.moves >= 0.0));
    assert!(
        map.cities
            .iter()
            .all(|c| c.hp <= c.hp_max && c.strength >= 0)
    );
    // Hover data is read-only. City opening and unit selection are client intents.
}

#[test]
fn unit_catalog_builds_for_every_starting_unit_before_a_city_exists() {
    let Some(mut h) = start_unfounded() else {
        return;
    };
    let world = h.w.fresh_view(0).unwrap();
    let ids: Vec<i64> = world
        .units
        .iter()
        .filter(|u| u.owner == 0)
        .map(|u| i64::from(u.id))
        .collect();
    assert!(ids.len() >= 2, "a settler and a warrior at least");
    for (n, id) in ids.into_iter().enumerate() {
        let root = hud(&mut h, 10 + n as u64, vec![("unit".into(), id)]);
        let unit = root.unit.expect("the focused unit");
        assert!(!unit.catalog.is_empty(), "unit {id} has an action catalog");
        for a in unit.catalog.iter() {
            eprintln!(
                "unit {id}: {} client={} enabled={} reason={:?}",
                a.id, &*a.client, a.enabled, a.reason
            );
        }
        // The orders a new game starts with can be given: the dial's first unit gets its order here.
        assert!(
            unit.catalog
                .iter()
                .any(|a| (&*a.client == "found_city" || &*a.client == "fortify") && a.enabled),
            "unit {id} can found a city or fortify"
        );
    }
}

#[test]
fn catalog_found_city_order_founds_a_city() {
    let Some(mut h) = start_unfounded() else {
        return;
    };
    let world = h.w.fresh_view(0).unwrap();
    let id = world
        .units
        .iter()
        .filter(|u| u.owner == 0)
        .map(|u| i64::from(u.id))
        .find(|id| {
            hud(&mut h, 1, vec![("unit".into(), *id)])
                .unit
                .is_some_and(|u| u.catalog.iter().any(|a| &*a.client == "found_city"))
        })
        .expect("a unit that can found a city");
    let action = hud(&mut h, 2, vec![("unit".into(), id)])
        .unit
        .unwrap()
        .catalog
        .iter()
        .find(|a| &*a.client == "found_city")
        .map(|a| i64::from(a.id))
        .unwrap();
    let outcome = h.cmd(
        0,
        protocol::Command::ScreenAction {
            screen: "hud".into(),
            action: "do_action".into(),
            args: vec![
                ("a".into(), id),
                ("b".into(), action),
                ("c".into(), 0),
                ("d".into(), 0),
            ],
            text: None,
        },
    );
    assert_eq!(outcome, protocol::CommandOutcome::Applied);
    let view = h.w.fresh_view(0).unwrap();
    assert!(view.cities.iter().any(|c| c.owner == 0), "the city exists");
}
