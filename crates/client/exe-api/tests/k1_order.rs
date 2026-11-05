//! The order of `Events.*` for the K1 scenario (a settler walks, a warrior is hurt, a city is founded, the turn
//! starts): `tests/fixtures/k1_event_order.txt`.
//!
//! The expected file is written from the order the DLL calls the engine in (`CvPlayer::found` creates the city
//! and then kills the settler; the dirty bits are raised once per frame after the callbacks) and from the
//! signatures of the EUI listeners.
//!
//! - `k1_order_scripted` feeds that callback order, in the layouts of `gamecore_abi.h`, through a fake game side
//!   and compares. Runs always.
//! - `k1_order_real_core` runs the same recorder over the real core: one instance plays a Tiny game as the server
//!   (two test-bot humans, two AI) and keeps the journal, then becomes the replica of seat 0 behind `GameSide`,
//!   and every batch is applied and dispatched as one frame. The rules the fixture's order stands for are checked
//!   on what the core produced (see [`check_real_order`]). Gated by `CIV5_REAL_CORE_K1=1`; returns early on the stub
//!   core. Run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set.
mod fake;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use exe_api::{ExeApi, ExeConfig, GameSide, NetMessage, UiEvent, UiEventKind as K, UiOp};
use fake::{FakeGame, ev, rig_with_game};

const EXPECTED: &str = include_str!("fixtures/k1_event_order.txt");

/// Records every event of the list as `Name(args)`.
const RECORDER: &str = r#"
rec = {}
local function fmt(v)
	local t = type(v)
	if t == 'table' then
		return '{' .. tostring(v.x) .. ',' .. tostring(v.y) .. '}'
	elseif t == 'string' then
		return '"' .. v .. '"'
	end
	return tostring(v)
end
local names = {
	'SerialEventUnitCreated', 'SerialEventUnitTeleportedToHex', 'SerialEventUnitSetDamage', 'SerialEventCityCreated',
	'SerialEventUnitDestroyed', 'SerialEventGameDataDirty', 'SerialEventCityInfoDirty', 'SerialEventUnitInfoDirty',
	'EndTurnBlockingChanged', 'SerialEventEndTurnDirty', 'ActivePlayerTurnStart', 'UnitSelectionChanged',
	'UnitMoveQueueChanged', 'HexFOWStateChanged', 'NotificationAdded',
}
for _, name in ipairs(names) do
	Events[name].Add(function(...)
		local args = {}
		for i = 1, select('#', ...) do args[#args + 1] = fmt((select(i, ...))) end
		rec[#rec + 1] = name .. '(' .. table.concat(args, ',') .. ')'
	end)
end
"#;

/// The unit block of `gamecore_abi.h`: owner, id, x, y, type, culture 1, civ 2, colours 3 4, flag index, fog 0
/// (`FOGOFWARMODE_OFF`: seen; Lua gets 2), not selected, military, visible.
fn unit(id: i32, x: i32, y: i32, ty: i32, flag: i32, military: i32) -> Vec<i32> {
    vec![0, id, x, y, ty, 1, 2, 3, 4, flag, 0, 0, military, 1]
}

/// The callbacks of the DLL for the scenario, in the order it makes them (the layouts of `gamecore_abi.h`).
fn dll_callbacks() -> Vec<UiEvent> {
    let settler = |x, y| unit(10, x, y, 7, 5, 0);
    let warrior = unit(11, 5, 2, 8, 6, 1);
    let with = |mut block: Vec<i32>, rest: &[i32]| {
        block.extend_from_slice(rest);
        block
    };
    // A Tiny map is 40 plots wide (plot index = y * width + x).
    let mut moved = UiEvent::new(K::UnitMoved, settler(5, 2));
    moved.path = vec![2 * 40 + 4, 2 * 40 + 5];
    vec![
        // The settler (player 0, unit 10) and a warrior (unit 11) exist.
        UiEvent::new(K::UnitCreated, settler(4, 2)),
        UiEvent::new(K::UnitCreated, warrior.clone()),
        // The settler walks one plot: the block holds the plot it ends on.
        moved,
        UiEvent::new(K::UnitMissionEnd, settler(5, 2)),
        // The warrior is hurt.
        UiEvent::new(K::UnitSetDamage, with(warrior, &[20, 0])),
        // The settler is selected (count, head) and founds the city: the city appears (CITY block, era), the
        // settler dies, the screen is dirty.
        ev(K::UnitSelectionChanged, &[1, 0, 10]),
        UiEvent::new(K::CityCreated, vec![0, 5, 5, 2, 1, 0, 200, 1, 0, 0, 1, 0]),
        UiEvent::new(K::UnitDestroyed, settler(5, 2)),
        ev(K::SetDirty, &[19, 1]),
        ev(K::SetDirty, &[18, 1]),
        ev(K::SetDirty, &[11, 1]),
        // The end of turn changes, and the next turn starts.
        ev(K::EndTurnBlockingChanged, &[-1, 3]),
        ev(K::ActivePlayerTurnStart, &[]),
    ]
}

/// The DLL's `Map.GetPlot(x, y):GetContinentArtType()` for the city of the scenario.
const MAP_STAND_IN: &str = "Map = { GetPlot = function(x, y) return { GetContinentArtType = function() return 1 end } end }";

fn run_scripted() -> Vec<String> {
    let game = FakeGame {
        attach_script: MAP_STAND_IN.to_owned(),
        ..FakeGame::default()
    };
    let mut r = rig_with_game(ExeConfig::default(), game);
    // The seat sees the plot of the city (the core's fog sync of an earlier batch).
    r.game
        .borrow_mut()
        .events
        .push(ev(K::FowChanged, &[5, 2, 0, 0]));
    r.exe.dispatch(&r.rt);
    r.exec(RECORDER);
    r.game.borrow_mut().events = dll_callbacks();
    r.exe.dispatch(&r.rt);
    assert_eq!(r.errors(), 0);
    let n: usize = r.eval_str("return tostring(#rec)").parse().unwrap();
    (1..=n)
        .map(|i| r.eval_str(&format!("return rec[{i}]")))
        .collect()
}

fn expected() -> Vec<String> {
    EXPECTED
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn k1_order_scripted() {
    let got = run_scripted();
    assert_eq!(got, expected(), "got:\n{}", got.join("\n"));
}

// ---------------------------------------------------------------- the real core

/// One recorded event: its name and its arguments as printed (a hex vector is one argument, `{x,y}`).
#[derive(Debug, Clone, PartialEq)]
struct Rec {
    name: String,
    args: Vec<String>,
}

fn parse(line: &str) -> Rec {
    let (name, rest) = line.split_once('(').expect("Name(args)");
    let body = rest.strip_suffix(')').expect("Name(args)");
    let mut args = Vec::new();
    let (mut depth, mut cur) = (0, String::new());
    for ch in body.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                args.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(ch);
    }
    if !cur.is_empty() || !args.is_empty() {
        args.push(cur);
    }
    Rec {
        name: name.to_owned(),
        args,
    }
}

const DIRTY: [&str; 3] = [
    "SerialEventGameDataDirty",
    "SerialEventCityInfoDirty",
    "SerialEventUnitInfoDirty",
];

/// What the fixture's order stands for, on the frames the real core produced (frame 0 is the first announcement):
/// 1. a unit is told of before anything else about it (created -> walks -> mission end / hurt -> destroyed);
/// 2. `CvPlayer::found`: in one frame a city appears and then the unit that stood on its plot (the settler) dies;
/// 3. the dirty events come after every other event of their frame, once each (frames after the first);
/// 4. `EndTurnBlockingChanged` is followed at once by `SerialEventEndTurnDirty`;
/// 5. the turn start (`ActivePlayerTurnStart`) is the core's and reaches the UI;
/// 6. the selection made by the UI comes back as `UnitSelectionChanged(owner, id, hex of the unit, true)`;
/// 7. a city of the seat (player 0) appears on a plot it sees: `fowState` 2 (the core's fog is translated).
///
/// Returns a short description of the founding frame (the events of the fixture's kinds, in order).
fn check_real_order(frames: &[Vec<Rec>], selected: Option<(String, String)>) -> String {
    let mut known: HashMap<(String, String), String> = HashMap::new();
    let mut founding: Option<String> = None;
    let mut turn_starts = 0;
    let mut selection_seen = false;
    for (fi, frame) in frames.iter().enumerate() {
        // 3.
        if fi > 0
            && let Some(first) = frame.iter().position(|r| DIRTY.contains(&r.name.as_str()))
        {
            let tail = &frame[first..];
            assert!(
                tail.iter().all(|r| DIRTY.contains(&r.name.as_str())),
                "frame {fi}: an event after the dirty events: {tail:?}"
            );
            for d in DIRTY {
                assert!(
                    tail.iter().filter(|r| r.name == d).count() <= 1,
                    "frame {fi}: {d} twice"
                );
            }
        }
        let mut cities_here: Vec<(String, String)> = Vec::new();
        for (i, r) in frame.iter().enumerate() {
            let a = |k: usize| r.args.get(k).cloned().unwrap_or_default();
            match r.name.as_str() {
                "SerialEventUnitCreated" => {
                    known.insert((a(0), a(1)), a(2));
                }
                "SerialEventUnitTeleportedToHex" => {
                    let key = (a(2), a(3));
                    assert!(
                        known.contains_key(&key),
                        "frame {fi}: {r:?} before the unit"
                    );
                    known.insert(key, format!("{{{},{}}}", a(0), a(1)));
                }
                "UnitMoveQueueChanged" | "SerialEventUnitSetDamage" => {
                    assert!(
                        known.contains_key(&(a(0), a(1))),
                        "frame {fi}: {r:?} before the unit"
                    );
                }
                "SerialEventCityCreated" => {
                    // 7. a city the seat founds stands on a plot it sees (`WhiteFog`, 2).
                    if fi > 0 && a(1) == "0" {
                        assert_eq!(a(8), "2", "frame {fi}: {r:?}");
                    }
                    cities_here.push((a(1), a(0)));
                }
                "SerialEventUnitDestroyed" => {
                    let key = (a(0), a(1));
                    let at = known.remove(&key);
                    assert!(at.is_some(), "frame {fi}: {r:?} of a unit never told of");
                    // 2. the settler stood on the plot of a city that appeared before, in this frame.
                    if founding.is_none()
                        && let Some(at) = at
                        && cities_here.iter().any(|(o, hex)| *o == key.0 && *hex == at)
                    {
                        let names: Vec<String> = frame[..=i]
                            .iter()
                            .filter(|r| !r.name.starts_with("HexFOW"))
                            .map(|r| format!("{}({})", r.name, r.args.join(",")))
                            .collect();
                        founding = Some(format!(
                            "frame {fi}: {}",
                            names[names.len().saturating_sub(4)..].join(" ")
                        ));
                    }
                }
                "EndTurnBlockingChanged" => {
                    // 4.
                    assert_eq!(
                        frame.get(i + 1).map(|r| r.name.as_str()),
                        Some("SerialEventEndTurnDirty"),
                        "frame {fi}: {r:?}"
                    );
                }
                "ActivePlayerTurnStart" => turn_starts += 1,
                "UnitSelectionChanged" => {
                    // 6.
                    if let Some((o, id)) = &selected
                        && a(0) == *o
                        && a(1) == *id
                        && a(5) == "true"
                    {
                        let hex = format!("{{{},{}}}", a(2), a(3));
                        assert_eq!(
                            known.get(&(o.clone(), id.clone())),
                            Some(&hex),
                            "frame {fi}: {r:?} is not at the unit's plot"
                        );
                        assert_eq!(
                            a(4).parse::<i32>().unwrap(),
                            -a(2).parse::<i32>().unwrap() - a(3).parse::<i32>().unwrap()
                        );
                        selection_seen = true;
                    }
                }
                _ => {}
            }
        }
    }
    // 5.
    assert!(turn_starts > 0, "no ActivePlayerTurnStart from the core");
    if selected.is_some() {
        assert!(selection_seen, "the selection did not come back");
    }
    founding.expect("no frame where a city appears and the settler on its plot dies")
}

struct DirHost(PathBuf);

impl gamecore::HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: gamecore::GameEvent) {}
}

fn gamedata_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CIV5_GAMEDATA") {
        return PathBuf::from(d);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .flat_map(|a| {
            ["gamedata-b2", "gamedata-b1"]
                .into_iter()
                .map(move |n| a.join("assets-out").join(n))
        })
        .find(|d| d.join("vfs-index.json").is_file())
        .expect("game data not found: set CIV5_GAMEDATA=<export-gamedata dir>")
}

/// The real core behind `GameSide`: the replica's `CoreEngine`; the UI's messages are kept (no server here).
struct RealGame {
    core: replica::CoreEngine,
    posted: Vec<NetMessage>,
}

impl GameSide for RealGame {
    fn attach_lua(&mut self, lua: &lua_sys::Lua) -> Result<(), String> {
        replica::Engine::attach_lua(&mut self.core, lua)
    }
    fn invalidate_lua(&mut self, lua: &lua_sys::Lua) {
        replica::Engine::invalidate_lua(&mut self.core, lua);
    }
    fn take_events(&mut self, out: &mut Vec<UiEvent>) {
        replica::Engine::take_events(&mut self.core, out);
    }
    fn ui_op(&mut self, op: UiOp, args: &[i32], out: &mut Vec<i32>) -> Result<(), String> {
        replica::Engine::ui_op(&mut self.core, op, args, out)
    }
    fn subscribe_game_event(&mut self, name: &str) {
        replica::Engine::subscribe_game_event(&mut self.core, name);
    }
    fn post_net(&mut self, msg: NetMessage) {
        self.posted.push(msg);
    }
    fn generation(&self) -> u64 {
        0
    }
    fn setup_graphical(&mut self) -> Result<(), String> {
        replica::Engine::setup_graphical(&mut self.core)
    }
}

/// Pumps the server until it can be saved at a turn after `after`; every batch and the digest after it are kept.
fn pump_to_save_point(
    game: &mut gamecore::Game,
    after: i32,
    batches: &mut Vec<(Vec<u8>, u64)>,
) -> i32 {
    use gamecore::replica::REPLICA_DIGEST;
    for _ in 0..20_000 {
        let r = game.pump(64).unwrap();
        let humans = game.game_info().unwrap().phase == gamecore::Phase::Humans;
        let batch = game.journal_take(humans).unwrap();
        let digest = game.state_digest(REPLICA_DIGEST).unwrap();
        batches.push((batch, digest));
        if r.turn > after && game.save().is_ok() {
            return r.turn;
        }
    }
    panic!("no save point after turn {after}");
}

const TURNS: i32 = 30;

#[test]
fn k1_order_real_core() {
    if std::env::var_os("CIV5_REAL_CORE_K1").is_none() {
        eprintln!("skipped: set CIV5_REAL_CORE_K1=1 (and link the real core)");
        return;
    }
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    use gamecore::replica::REPLICA_DIGEST;
    use gamecore::{Role, Setup, SlotSetup};

    // The server: a Tiny game of two test-bot humans and two AI, from its first save point.
    let mut core = replica::CoreEngine::new(DirHost(gamedata_dir()), 0).unwrap();
    let mut setup = Setup::network(
        1,
        vec![
            SlotSetup::human("random", "a"),
            SlotSetup::human("random", "b"),
            SlotSetup::ai("random"),
            SlotSetup::ai("random"),
        ],
    );
    setup.world_size = "WORLDSIZE_TINY".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.autoplay_humans = vec![0, 1];
    let game = core.game().game();
    game.new_game(&setup).unwrap();
    game.set_role(Role::Server).unwrap();
    let mut ignored = Vec::new();
    let start = pump_to_save_point(game, -1, &mut ignored);
    let first = game.save().unwrap();
    let mut batches = Vec::new();
    let mut turn = start;
    while turn < start + TURNS {
        turn = pump_to_save_point(game, turn, &mut batches);
    }

    // The same instance as the replica of seat 0, behind the exe API.
    game.set_role(Role::Replica { seat: Some(0) }).unwrap();
    game.resync(&first).unwrap();
    let real = Rc::new(RefCell::new(RealGame {
        core,
        posted: Vec::new(),
    }));
    let host = Rc::new(fake::TestHost::default());
    let mut exe = ExeApi::new(ExeConfig::default(), host);
    let rt = lua_sys::Runtime::new(
        lua_sys::RuntimeConfig::default(),
        Rc::new(lua_sys::MemorySource::new()),
        exe.base_setup(),
    )
    .unwrap();
    let ctx = rt.create_context("Test", |_, _| Ok(())).unwrap();
    exe.attach_runtime(&rt);
    let dynamic: Rc<RefCell<dyn GameSide>> = real.clone();
    exe.bind_game(dynamic, ctx.lua());
    ctx.exec(RECORDER, "=recorder").unwrap();
    ctx.exec(
        "function SelectFirst()
            for u in Players[0]:Units() do UI.SelectUnit(u) return u:GetOwner() .. ':' .. u:GetID() end
            return ''
         end",
        "=select",
    )
    .unwrap();
    let eval = |code: &str| -> String {
        ctx.lua()
            .load(code)
            .set_name("=eval")
            .set_environment(ctx.env())
            .eval::<String>()
            .unwrap_or_else(|e| panic!("{e}"))
    };

    let mut ends = Vec::new();
    // Frame 0: the first announcement (setup_graphical).
    exe.dispatch(&rt);
    ends.push(eval("return tostring(#rec)").parse::<usize>().unwrap());
    let mut selected = None;
    let total = batches.len();
    for (i, (batch, digest)) in batches.iter().enumerate() {
        {
            let mut g = real.borrow_mut();
            let game = g.core.game().game();
            game.journal_apply(batch)
                .unwrap_or_else(|e| panic!("batch {i} of {total}: {e}"));
            assert_eq!(
                game.state_digest(REPLICA_DIGEST).unwrap(),
                *digest,
                "digest after batch {i}"
            );
        }
        // Halfway, the UI selects a unit of the seat (a frame of its own).
        if i == total / 2 && selected.is_none() {
            let s = eval("return SelectFirst()");
            if let Some((o, id)) = s.split_once(':') {
                selected = Some((o.to_owned(), id.to_owned()));
            }
        }
        exe.dispatch(&rt);
        ends.push(eval("return tostring(#rec)").parse::<usize>().unwrap());
        // The UI does not move the game.
        let mut g = real.borrow_mut();
        assert_eq!(
            g.core.game().game().state_digest(REPLICA_DIGEST).unwrap(),
            *digest,
            "the UI moved the digest after batch {i}"
        );
    }
    assert_eq!(rt.error_count(), 0, "listener errors");
    let n = *ends.last().unwrap();
    let lines: Vec<Rec> = (1..=n)
        .map(|i| parse(&eval(&format!("return rec[{i}]"))))
        .collect();
    let mut frames = Vec::new();
    let mut from = 0;
    for end in ends {
        frames.push(lines[from..end].to_vec());
        from = end;
    }
    let founding = check_real_order(&frames, selected.clone());
    let count = |name: &str| lines.iter().filter(|r| r.name == name).count();
    eprintln!(
        "K1 real core: {total} batches, {n} events ({} created, {} teleported, {} destroyed, {} cities, {} turn starts, {} end-turn blocking), selected {selected:?}",
        count("SerialEventUnitCreated"),
        count("SerialEventUnitTeleportedToHex"),
        count("SerialEventUnitDestroyed"),
        count("SerialEventCityCreated"),
        count("ActivePlayerTurnStart"),
        count("EndTurnBlockingChanged"),
    );
    eprintln!("K1 real core founding: {founding}");
    let stats = exe.stats();
    eprintln!(
        "K1 real core: unmapped {} ({:?})",
        stats.unmapped,
        stats
            .simp_by_name
            .iter()
            .filter(|(k, _)| k.starts_with("engine."))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_recorder_lines_parse() {
    let r = parse("SerialEventCityCreated({4,2},0,5,1,0,1,1,1,2)");
    assert_eq!(r.name, "SerialEventCityCreated");
    assert_eq!(r.args, ["{4,2}", "0", "5", "1", "0", "1", "1", "1", "2"]);
    assert!(parse("SerialEventEndTurnDirty()").args.is_empty());
    // The scripted run passes the rules the real core is held to.
    let frames = vec![
        Vec::new(),
        run_scripted().iter().map(|l| parse(l)).collect(),
    ];
    let founding = check_real_order(&frames, Some(("0".into(), "10".into())));
    assert!(
        founding.contains("SerialEventCityCreated({4,2},0,5"),
        "{founding}"
    );
}
