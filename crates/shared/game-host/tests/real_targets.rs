//! The map targeting query (`targets` screen) and its order through the real worker: an archer of seat 0 and a city
//! with a barbarian in range. The query lists the plots the core accepts, only for the seat that owns the shooter,
//! and leaves the simulation digest as it was; the order on a listed plot is the core's ranged attack.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn digest(h: &mut common::H) -> u64 {
    h.w.core_mut().gamecore_mut().state_digest(-2).unwrap()
}

fn screen(
    h: &mut common::H,
    qid: u64,
    player: i8,
    name: &str,
    args: &[(&str, i64)],
) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player,
        qid,
        q: Query::Screen {
            screen: name.into(),
            args: args.iter().map(|(k, v)| ((*k).into(), *v)).collect(),
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid: id, answer } if *id == qid => Some(answer),
            _ => None,
        })
        .expect("screen answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("{name} query failed: {answer:?}")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model": {name: root}}).to_string(),
    )
    .expect("schema");
    root
}

fn cells(v: &serde_json::Value) -> Vec<(i64, i64)> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|p| (p["x"].as_i64().unwrap(), p["y"].as_i64().unwrap()))
        .collect()
}

#[test]
fn targets_list_what_the_core_accepts_for_the_seat_and_change_nothing() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    h.found_city(0);
    // An archer in the capital and a barbarian warrior two plots away on land the city sees.
    let lua = lua_sys::Lua::new().unwrap();
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    lua.exec(
        r#"
        local p = Players[0]
        local city = p:GetCapitalCity()
        local x, y = city:GetX(), city:GetY()
        archer = p:InitUnit(GameInfoTypes.UNIT_ARCHER, x, y):GetID()
        city_id = city:GetID()
        for dx = -2, 2 do for dy = -2, 2 do
            local plot = Map.PlotXYWithRangeCheck(x, y, dx, dy, 2)
            if not ex and plot and Map.PlotDistance(x, y, plot:GetX(), plot:GetY()) == 2 and not plot:IsWater()
                and not plot:IsMountain() and not plot:IsImpassable() and plot:GetNumUnits() == 0
                and plot:IsVisible(p:GetTeam(), false) and city:Plot():CanSeePlot(plot, p:GetTeam(), 1, -1) then
                ex, ey = plot:GetX(), plot:GetY()
            end
        end end
        enemy = Players[GameDefines.BARBARIAN_PLAYER or 63]:InitUnit(GameInfoTypes.UNIT_WARRIOR, ex, ey):GetID()
        -- A warrior of the seat next to the barbarian (the move-attack's test wants an adjacent enemy).
        for dx = -1, 1 do for dy = -1, 1 do
            local plot = Map.PlotXYWithRangeCheck(ex, ey, dx, dy, 1)
            if not warrior and plot and not plot:IsWater() and not plot:IsMountain() and not plot:IsImpassable()
                and plot:GetNumUnits() == 0 and not plot:IsCity() and Map.PlotDistance(x, y, plot:GetX(), plot:GetY()) <= 2 then
                warrior = p:InitUnit(GameInfoTypes.UNIT_WARRIOR, plot:GetX(), plot:GetY()):GetID()
            end
        end end
        team0 = p:GetTeam()
        "#,
        "=targets-fixture",
    )
    .unwrap();
    let archer: i64 = lua.globals().get("archer").unwrap();
    let warrior: i64 = lua.globals().get("warrior").unwrap();
    let city: i64 = lua.globals().get("city_id").unwrap();
    let enemy_at: (i64, i64) = (
        lua.globals().get("ex").unwrap(),
        lua.globals().get("ey").unwrap(),
    );

    lua.exec(
        r#"attack_row = -1 local id = 0
        while GameInfoActions[id] do
            if GameInfoActions[id].Type == "INTERFACEMODE_ATTACK" then attack_row = id end
            id = id + 1
        end"#,
        "=attack-row",
    )
    .unwrap();
    let attack_row: i64 = lua.globals().get("attack_row").unwrap();

    // Every targeting query, for every kind of shooter and for a seat that does not own it, leaves the digest.
    let before = digest(&mut h);
    let ranged = screen(
        &mut h,
        1,
        0,
        "targets",
        &[("kind", 0), ("id", archer), ("action", -1)],
    );
    let again = screen(
        &mut h,
        2,
        0,
        "targets",
        &[("kind", 0), ("id", archer), ("action", -1)],
    );
    let strike = screen(&mut h, 3, 0, "targets", &[("kind", 1), ("id", city)]);
    // Seat 1 asking for seat 0's archer learns nothing of it.
    let foreign = screen(
        &mut h,
        5,
        1,
        "targets",
        &[("kind", 0), ("id", archer), ("action", -1)],
    );
    let melee = screen(
        &mut h,
        8,
        0,
        "targets",
        &[("kind", 0), ("id", warrior), ("action", attack_row)],
    );
    assert_eq!(
        digest(&mut h),
        before,
        "the targeting queries changed the simulation"
    );
    // The HUD's new availability tests (CvUnit::CanDoInterfaceMode's getters, the move-attack's path search among
    // them) in the read scope the screens run in.
    lua.exec(
        r#"GcUiReadScope(function()
            for _, id in ipairs({archer, warrior}) do
                local u = Players[0]:GetUnitByID(id)
                u:IsEnemyInMovementRange(false, u:IsCityAttackOnly()) u:CanNuke() u:CanParadrop(u:GetPlot(), true)
                u:CanAirlift(u:GetPlot()) u:CanEmbark(u:GetPlot()) u:CanDisembark(u:GetPlot()) u:CanBuildRoute()
                u:CanRangeStrike() u:IsCanAttackWithMove() u:IsOutOfAttacks()
            end
        end)"#,
        "=hud-getters",
    )
    .unwrap();
    assert_eq!(
        digest(&mut h),
        before,
        "the HUD's targeting availability changed the simulation"
    );
    // The HUD's catalogs (after the digest checks: its automate-explore test is not pure with an enemy near, which
    // predates this mode). The archer's ranged attack and the warrior's attack are targeting orders.
    let hud = screen(&mut h, 4, 0, "hud", &[("unit", archer)]);
    let melee_hud = screen(&mut h, 7, 0, "hud", &[("unit", warrior)]);
    let attack = melee_hud["unit"]["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["client"] == "target" && a["id"] == attack_row)
        .unwrap_or_else(|| panic!("the attack order in {}", melee_hud["unit"]["catalog"]))
        .clone();
    assert_eq!(attack["enabled"], true, "an enemy is in reach: {attack}");
    assert_eq!(melee["look"], "attack");

    eprintln!("ranged: {ranged}\nstrike: {strike}\nenemy at {enemy_at:?}");
    assert_eq!(ranged, again, "the same answer twice");
    assert_eq!(ranged["id"], archer);
    assert_eq!(ranged["look"], "fire");
    assert_eq!(ranged["name"], "Ranged Attack");
    let plots = cells(&ranged["plots"]);
    assert_eq!(plots, vec![enemy_at], "the barbarian is the one target");
    let area = cells(&ranged["area"]);
    assert!(
        area.contains(&enemy_at) && area.len() > 3,
        "the radius around the archer: {area:?}"
    );
    // Every plot of the radius is seen by the seat and within the archer's range.
    let list: Vec<String> = area.iter().map(|(x, y)| format!("{{{x}, {y}}}")).collect();
    lua.exec(&format!("area = {{{}}}", list.join(", ")), "=area")
        .unwrap();
    lua.exec(
        r#"
        local u = Players[0]:GetUnitByID(archer)
        for _, p in ipairs(area) do
            local plot = Map.GetPlot(p[1], p[2])
            assert(plot:IsVisible(team0, false), "a radius plot the seat does not see")
            assert(Map.PlotDistance(u:GetX(), u:GetY(), p[1], p[2]) <= u:Range(), "a radius plot out of range")
        end
        "#,
        "=area-check",
    )
    .unwrap();
    assert_eq!(
        cells(&strike["plots"]),
        vec![enemy_at],
        "the city strikes the same warrior"
    );
    assert_eq!(
        cells(&melee["plots"]),
        vec![enemy_at],
        "the warrior can attack it by moving"
    );
    let row = hud["unit"]["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["client"] == "target")
        .unwrap_or_else(|| panic!("a targeting order in {}", hud["unit"]["catalog"]));
    assert_eq!(row["enabled"], true, "{row}");
    assert!(
        cells(&foreign["plots"]).is_empty() && foreign["reason"].is_string(),
        "{foreign}"
    );

    // The order: a plot that is not listed is refused, the listed one is the core's ranged attack.
    let go = |d: i64| Command::ScreenAction {
        screen: "targets".into(),
        action: "go".into(),
        args: vec![
            ("a".into(), 0),
            ("b".into(), archer),
            ("c".into(), -1),
            ("d".into(), d),
        ],
        text: None,
    };
    let cell = |(x, y): (i64, i64)| x * 65_536 + y;
    let wrong = area.iter().copied().find(|p| *p != enemy_at).unwrap();
    assert!(matches!(
        h.cmd(0, go(cell(wrong))),
        CommandOutcome::Rejected { .. }
    ));
    assert!(
        matches!(
            h.cmd(1, go(cell(enemy_at))),
            CommandOutcome::Rejected { .. }
        ),
        "not seat 1's archer"
    );
    assert_eq!(h.cmd(0, go(cell(enemy_at))), CommandOutcome::Applied);
    lua.exec(
        r#"
        local e = Players[GameDefines.BARBARIAN_PLAYER or 63]:GetUnitByID(enemy)
        hit = (e == nil) or e:GetDamage() > 0
        shot = Players[0]:GetUnitByID(archer):IsOutOfAttacks()
        "#,
        "=after-shot",
    )
    .unwrap();
    let (hit, shot): (bool, bool) = (
        lua.globals().get("hit").unwrap(),
        lua.globals().get("shot").unwrap(),
    );
    assert!(hit, "the warrior took damage");
    assert!(shot, "the archer used its attack");
    // Now the archer has nothing to shoot at: the mode lists nothing and says why.
    let spent = screen(
        &mut h,
        6,
        0,
        "targets",
        &[("kind", 0), ("id", archer), ("action", -1)],
    );
    assert!(
        cells(&spent["plots"]).is_empty() && spent["reason"].is_string(),
        "{spent}"
    );
}
