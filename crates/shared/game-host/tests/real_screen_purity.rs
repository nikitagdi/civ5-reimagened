//! No screen query or refused screen action changes the simulation: every registered screen's data,
//! for both seats and with typical arguments, at several points of a turn, leaves the simulation
//! digest as it was. Screens only read, so a lasting effect would be a state change at query time;
//! an equal digest right after the queries means the game goes on exactly as without them. With an enemy next to
//! units that can explore, the whole HUD catalog of every unit and city and every targeting mode leave it too.
use game_host::Worker;
use protocol::{Command, Query, Start, ToWorker};
#[path = "common/real.rs"]
mod common;

fn digest(h: &mut common::H) -> u64 {
    h.w.core_mut().gamecore_mut().state_digest(-2).unwrap()
}

/// Every screen, both seats, a few argument sets (failures are fine: they still run the script).
fn query_everything(h: &mut common::H, qid: &mut u64, extra: &[(String, i64)]) {
    for (name, _) in game_host::screens::REGISTRY {
        for seat in [0i8, 1] {
            let mut sets: Vec<Vec<(String, i64)>> = vec![
                vec![],
                vec![("a".into(), 0)],
                vec![("a".into(), 1), ("b".into(), 0)],
            ];
            sets.push(extra.to_vec());
            for args in sets {
                *qid += 1;
                h.send(ToWorker::Query {
                    player: seat,
                    qid: *qid,
                    q: Query::Screen {
                        screen: (*name).into(),
                        args,
                    },
                });
                h.run();
            }
        }
    }
}

#[test]
fn screen_queries_leave_the_simulation_digest_unchanged() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut setup = common::setup();
    setup.settings.minor_civs = 2;
    let (worker, outs) = Worker::new(common::real_core(), common::init(Start::New(setup), ""), 0);
    let mut h = common::H::with(worker, outs);
    let mut qid = 1000;
    let before = digest(&mut h);
    query_everything(&mut h, &mut qid, &[]);
    assert_eq!(digest(&mut h), before, "new game, before any city");
    for seat in [0, 1] {
        h.found_city(seat);
    }
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;
    let extra = vec![("a".into(), i64::from(city)), ("d".into(), i64::from(city))];
    let before = digest(&mut h);
    query_everything(&mut h, &mut qid, &extra);
    assert_eq!(digest(&mut h), before, "cities founded, mid turn");
    // A refused action runs the script too.
    let _ = h.cmd(
        0,
        Command::ScreenAction {
            screen: "city_state".into(),
            action: "bully_gold".into(),
            args: vec![("a".into(), 99)],
            text: None,
        },
    );
    for seat in [0, 1] {
        h.rest_idle_units(seat);
    }
    h.next_turn(&[0, 1]);
    // Turn 2's human phase: the AI has moved, caches were refreshed by the simulation or are stale.
    let before = digest(&mut h);
    query_everything(&mut h, &mut qid, &extra);
    assert_eq!(digest(&mut h), before, "next turn, human phase");
    query_everything(&mut h, &mut qid, &extra);
    assert_eq!(digest(&mut h), before, "queried twice");

    // An enemy next to units that can explore: the HUD's automate-explore test (CvUnit::CanAutomate ->
    // CvHomelandAI::IsAnyValidExploreMoves) rebuilds the economic AI's exploration plots while they are dirty
    // (CvEconomicAI::UpdatePlots). The whole HUD catalog of every unit and city and every targeting mode run under it.
    let lua = lua_sys::Lua::new().unwrap();
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    lua.exec(
        r#"
        local barbarian = Players[GameDefines.BARBARIAN_PLAYER or 63]
        explorers = {}
        for seat = 0, 1 do
            local p = Players[seat]
            local city = p:GetCapitalCity()
            local x, y = city:GetX(), city:GetY()
            p:InitUnit(GameInfoTypes.UNIT_ARCHER, x, y)
            local placed
            for dx = -2, 2 do for dy = -2, 2 do
                local a = Map.PlotXYWithRangeCheck(x, y, dx, dy, 2)
                if not placed and a and not a:IsWater() and not a:IsMountain() and not a:IsImpassable()
                    and a:GetNumUnits() == 0 and not a:IsCity() then
                    for ex = -1, 1 do for ey = -1, 1 do
                        local b = Map.PlotXYWithRangeCheck(a:GetX(), a:GetY(), ex, ey, 1)
                        if not placed and b and b ~= a and not b:IsWater() and not b:IsMountain() and not b:IsImpassable()
                            and b:GetNumUnits() == 0 and not b:IsCity() and b:GetOwner() ~= seat then
                            explorers[#explorers + 1] = p:InitUnit(GameInfoTypes.UNIT_WARRIOR, a:GetX(), a:GetY()):GetID()
                            barbarian:InitUnit(GameInfoTypes.UNIT_WARRIOR, b:GetX(), b:GetY())
                            placed = true
                        end
                    end end
                end
            end end
            assert(placed, "no plot for seat " .. seat .. "'s warrior and its barbarian")
        end
        -- The action rows a targeting mode is asked with (and those it refuses), and the city-states for the gifts.
        local rows, id = {}, 0
        while GameInfoActions[id] do
            local t = GameInfoActions[id].Type
            if type(t) == "string" and t:find("^INTERFACEMODE_") then rows[#rows + 1] = id end
            id = id + 1
        end
        target_rows = table.concat(rows, ",")
        local minors = {}
        for i = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
            if Players[i]:IsMinorCiv() and Players[i]:IsAlive() then minors[#minors + 1] = i end
        end
        minor_ids = table.concat(minors, ",")
        "#,
        "=purity-enemies",
    )
    .unwrap();
    let list = |name: &str| -> Vec<i64> {
        let s: String = lua.globals().get(name).unwrap();
        s.split(',')
            .filter(|v| !v.is_empty())
            .map(|v| v.parse().unwrap())
            .collect()
    };
    let (rows, minors) = (list("target_rows"), list("minor_ids"));
    assert!(
        !rows.is_empty() && !minors.is_empty(),
        "rows {rows:?}, city-states {minors:?}"
    );
    let before = digest(&mut h);
    let mut screens = 0;
    for seat in [0i8, 1] {
        let view = h.w.fresh_view(seat).unwrap();
        let units: Vec<i64> = view
            .units
            .iter()
            .filter(|u| u.owner == seat)
            .map(|u| i64::from(u.id))
            .collect();
        let cities: Vec<i64> = view
            .cities
            .iter()
            .filter(|c| c.owner == seat)
            .map(|c| i64::from(c.id))
            .collect();
        let mut asks: Vec<(&str, Vec<(String, i64)>)> = Vec::new();
        for &u in &units {
            asks.push(("hud", vec![("unit".into(), u)]));
            for action in std::iter::once(-1).chain(rows.iter().copied()) {
                asks.push((
                    "targets",
                    vec![
                        ("kind".into(), 0),
                        ("id".into(), u),
                        ("action".into(), action),
                    ],
                ));
            }
        }
        for &c in &cities {
            asks.push(("hud", vec![("city".into(), c)]));
            asks.push(("targets", vec![("kind".into(), 1), ("id".into(), c)]));
        }
        for &m in &minors {
            for kind in [2, 3] {
                asks.push(("targets", vec![("kind".into(), kind), ("id".into(), m)]));
            }
        }
        for (screen, args) in asks {
            qid += 1;
            screens += 1;
            h.send(ToWorker::Query {
                player: seat,
                qid,
                q: Query::Screen {
                    screen: screen.into(),
                    args,
                },
            });
            h.run();
        }
        // Refused orders run the scripts' checks too: a plot that is not on the map, an unknown action row.
        for &u in &units {
            let _ = h.cmd(
                seat,
                Command::ScreenAction {
                    screen: "targets".into(),
                    action: "go".into(),
                    args: vec![
                        ("a".into(), 0),
                        ("b".into(), u),
                        ("c".into(), -1),
                        ("d".into(), -1),
                    ],
                    text: None,
                },
            );
            let _ = h.cmd(
                seat,
                Command::ScreenAction {
                    screen: "hud".into(),
                    action: "do_action".into(),
                    args: vec![("a".into(), u), ("b".into(), -1)],
                    text: None,
                },
            );
        }
    }
    assert!(screens > 40, "{screens} screen queries");
    assert_eq!(
        digest(&mut h),
        before,
        "the HUD catalog and the targeting modes with an enemy next to an explorer"
    );
    // The case is the one that changed the simulation: outside the read scope the same test does.
    lua.exec(
        r#"
        for _, id in ipairs(explorers) do
            for seat = 0, 1 do
                local u = Players[seat]:GetUnitByID(id)
                if u then u:CanDoCommand(CommandTypes.COMMAND_AUTOMATE, GameInfoTypes.AUTOMATE_EXPLORE or 1, -1, 0, 1) end
            end
        end
        "#,
        "=unscoped-automate",
    )
    .unwrap();
    assert_ne!(
        digest(&mut h),
        before,
        "the automate-explore test outside the scope rebuilds the exploration plots"
    );
    let before = digest(&mut h);

    // The digest does see what an unscoped getter does: the same call outside the read scope draws from the
    // synced game RNG (DS-0038) and changes it.
    lua.exec(
        "for c in Players[0]:Cities() do c:GetNextBuyablePlot() end",
        "=unscoped-probe",
    )
    .unwrap();
    assert_ne!(
        digest(&mut h),
        before,
        "the probe changes the digest outside the scope"
    );
}
