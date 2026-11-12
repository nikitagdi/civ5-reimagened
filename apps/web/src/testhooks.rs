//! Test hooks (playtest §3.1), only with `?test=1`: `window.__civ5test`, the playtest's view of the
//! screens and its hands on them. Thin by design: the client's test port
//! ([`client_app::test_port`]) hands over each frame's screens and turns the actions into the
//! same screen actions and map clicks a player makes; nothing here talks to the game worker.
//!
//! - `state()`: the newest frame's screens as JSON (screen, turn, status, blocker, end-turn text,
//!   unit and city panels, the human's units and cities, the journal tail, the fatal text), plus
//!   `frames`, `asked` and `applied` (actions queued and handed to the client so far).
//! - `act({kind, ...})`: queues one action, returns its number (`applied >= n` once the client
//!   took it). Kinds: `startGame {civ?, handicap?, ai?, seed?}`, `continueGame`, `endTurn {force?}`,
//!   `selectUnit {id}`, `openCity {id}`, `click {x, y, secondary?}`, `unitAction {action}`,
//!   `chooseBuild {index}`, `buyBuild {index, faith?}`, `closeCity`, `confirm {yes}`, `reload`
//!   (the error screen's Reload), `fault {fault: "badMessage" | "killHost"}` (breaks the link to
//!   the game worker on purpose: the playtest's error screen check). On a server session the faults are
//!   `dropSocket` (the client comes back by itself), `corruptView` (the next diff makes it ask for a
//!   keyframe), `badMessage` (a frame that is no message: the server drops the connection) and
//!   `silence {ms}` (the socket is not read: a probe of the liveness checks).
//!   In the game of a server session `state().net` also has `phase`, `rttMs`, `viewSeq`, `viewHash`,
//!   `basisHash`, `revision`, `inflight`, `overlay` (records of the optimistic layer), `ctx` (the
//!   selected unit's context: `fresh`, `stale`, `missing`), `turnState`, `seats`, `reconnect`.
//! - The server session (lobby-ui 4.11; `state().net` has the screens' data, `screen` is `login`,
//!   `lobby`, `room`, `starting`, `loading`, `play`, `notice` or `fatal`): `login {nick, password?}`,
//!   `retry`, `signOut`, `dismissNotice`, `useThisTab`, `createRoom`, `joinRoom {room}`, `leaveRoom`, `giveUpSeat`, `confirmGiveUp {yes}`,
//!   `roomSetting {field, value}` (see [`parse_room_edit`]), `slot {index, occupant: "open" | "ai" | "closed", civ?}`, `pickCiv {civ?}`,
//!   `takeSlot {index}`, `ready {value?}`, `start`, `loadSave {save}`, `takeSeat {game, player}`,
//!   `quickStart {civ?, ai?, size?, seed?, mode?}`. `startGame` is a quick start there.
//! - `metrics()`: `window.__civ5stats` (fps, draws, timings, `timeToMapMs`), when the game was
//!   started (`startGameAtMs`), and per finished turn the time from the last end-turn request to
//!   the next human turn (`wallMs`) and the time the rivals were seen moving (`aiMs`); on a server
//!   session `client`: the counters of the client (`ClientStats`: diffs, `needKeyframe`, orders by
//!   outcome, queries by kind, layer, ...) and the rings `rttMs` and `orderMs`.
//! - `digest()`: FNV-1a 64 (hex) of the visible state (turn, the human's units and cities); `digest('replica')`: a
//!   promise of the replica's own digest (`Replica::digest`: UI v2 actions must leave it as it is).
//! - The game UIs: `act({kind: 'setUi', value: 'classic' | 'v2' | 'v2:<skin>', save?, cold?})` switches,
//!   `state().ui` (see [`ui_json`]) tells which one is on the screen, `ui2()` (see [`ui2_json`]) is UI v2 as of its
//!   last frame, `ui2Rects(parts)` a promise of the rectangles of `data-part` elements.
//! - `waitTurn(n, timeoutMs?)`: a promise of the state once turn `n` is the human's.
//! - `trap(kind)`: kills the module on purpose (`page`, `workerPanic`, `workerTrap`), for the crash report.
//! - `golden` (page with `&golden=<url>&capture=<name>`): once the frame is final,
//!   `{ status: 'done', capture, width, height, rgba, mask }` (typed arrays: the world without the UI,
//!   `Rgba8Unorm`, and the class mask) or `{ status: 'failed', error }`; absent until then.

use std::cell::RefCell;
use std::rc::Rc;

use client_app::golden::GoldenOutput;
use client_app::test_port::{
    ClientStats, Fault, QualityPreset, RoomEdit, RoomParams, TestAction, TestNet, TestPort,
    TestSnapshot, TimerChoice, TurnMode, TurnModeChoice, UiAction, parse_slot_kind,
};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

const GLOBAL: &str = "__civ5test";

/// A finished turn's times (ms).
#[derive(Debug, Clone, Copy)]
struct TurnTime {
    turn: i32,
    wall_ms: Option<f64>,
    ai_ms: Option<f64>,
}

#[derive(Debug, Default)]
struct Shared {
    snapshot: Option<TestSnapshot>,
    queue: Vec<TestAction>,
    asked: u64,
    applied: u64,
    frames: u64,
    start_game_at: Option<f64>,
    /// The last end-turn request: its turn and time.
    end_asked: Option<(i32, f64)>,
    /// The rivals are moving since then (turn, ms).
    ai_since: Option<(i32, f64)>,
    ai_ms: Option<(i32, f64)>,
    /// The newest human turn seen.
    human_turn: Option<i32>,
    turns: Vec<TurnTime>,
}

fn now_ms() -> f64 {
    platform_web::WebClock::new().now_ms_f64()
}

struct Port(Rc<RefCell<Shared>>);

impl TestPort for Port {
    fn publish(&mut self, snapshot: TestSnapshot) {
        let now = now_ms();
        let mut s = self.0.borrow_mut();
        s.frames += 1;
        match (snapshot.status, snapshot.turn) {
            (Some("ai"), Some(turn)) if s.ai_since.is_none() => s.ai_since = Some((turn, now)),
            (Some("human"), Some(turn)) => {
                if let Some((t, at)) = s.ai_since.take() {
                    s.ai_ms = Some((t, now - at));
                }
                if s.human_turn.is_some_and(|h| turn > h) {
                    let ended = turn - 1;
                    let wall_ms = s
                        .end_asked
                        .filter(|(t, _)| *t == ended)
                        .map(|(_, at)| now - at);
                    let ai_ms = s.ai_ms.filter(|(t, _)| *t == ended).map(|(_, ms)| ms);
                    s.turns.push(TurnTime {
                        turn: ended,
                        wall_ms,
                        ai_ms,
                    });
                }
                s.human_turn = Some(turn);
            }
            _ => {}
        }
        s.snapshot = Some(snapshot);
    }

    fn take_actions(&mut self) -> Vec<TestAction> {
        let mut s = self.0.borrow_mut();
        let actions = std::mem::take(&mut s.queue);
        s.applied += actions.len() as u64;
        let now = now_ms();
        for a in &actions {
            match a {
                TestAction::StartGame { .. }
                | TestAction::ContinueGame
                | TestAction::QuickStart(_)
                | TestAction::Ui(UiAction::Start | UiAction::QuickStart) => {
                    s.start_game_at.get_or_insert(now);
                }
                TestAction::EndTurn { .. } => {
                    if let Some(turn) = s.snapshot.as_ref().and_then(|x| x.turn) {
                        s.end_asked = Some((turn, now));
                    }
                }
                _ => {}
            }
        }
        actions
    }

    /// `window.__civ5test.golden`: `{ status: 'done', capture, width, height, rgba, mask }` (typed arrays) or
    /// `{ status: 'failed', error }`; `golden.mjs` polls for it.
    fn golden(&mut self, result: Result<&GoldenOutput, &str>) {
        let Some(window) = web_sys::window() else {
            return;
        };
        let Ok(api) = js_sys::Reflect::get(&window, &GLOBAL.into()) else {
            return;
        };
        let out = js_sys::Object::new();
        let set = |name: &str, v: JsValue| {
            let _ = js_sys::Reflect::set(&out, &name.into(), &v);
        };
        match result {
            Ok(frame) => {
                set("status", "done".into());
                set("capture", frame.capture.as_str().into());
                set("width", frame.size.0.into());
                set("height", frame.size.1.into());
                set("rgba", js_sys::Uint8Array::from(&frame.rgba[..]).into());
                set("mask", js_sys::Uint8Array::from(&frame.mask[..]).into());
            }
            Err(e) => {
                set("status", "failed".into());
                set("error", e.into());
            }
        }
        let _ = js_sys::Reflect::set(&api, &"golden".into(), &out);
    }
}

/// With `?test=1`: installs `window.__civ5test` and returns the port for `Services::test_port`.
pub fn install() -> Option<Box<dyn TestPort>> {
    if platform_web::page::query_param("test").as_deref() != Some("1") {
        return None;
    }
    let window = web_sys::window()?;
    let shared = Rc::new(RefCell::new(Shared::default()));
    let api = js_sys::Object::new();

    let set = |name: &str, f: &JsValue| {
        let _ = js_sys::Reflect::set(&api, &name.into(), f);
    };
    let s = Rc::clone(&shared);
    let state = Closure::<dyn Fn() -> JsValue>::new(move || to_js(&state_json(&s.borrow())));
    set("state", state.as_ref());
    state.forget();

    let s = Rc::clone(&shared);
    let act = Closure::<dyn Fn(JsValue) -> Result<f64, JsValue>>::new(move |v: JsValue| {
        let text: String = js_sys::JSON::stringify(&v)?.into();
        let value: Value =
            serde_json::from_str(&text).map_err(|e| JsValue::from_str(&e.to_string()))?;
        let action = parse_action(&value).map_err(|e| JsValue::from_str(&e))?;
        // A page that switches to UI v2 gets its probe (`ui2()`) from then on.
        if let TestAction::SetUi { value, .. } = &action
            && client_app::parse_ui2_param(Some(value)).is_some()
        {
            client_app::enable_ui2_probe();
        }
        let mut s = s.borrow_mut();
        s.queue.push(action);
        s.asked += 1;
        Ok(s.asked as f64)
    });
    set("act", act.as_ref());
    act.forget();

    // `window.__civ5test.lua(code)`: Lua for the original UI's layer (M2/M3 playtests; `print` goes to the log).
    let lua = Closure::<dyn Fn(String)>::new(client_app::push_m2_lua);
    set("lua", lua.as_ref());
    lua.forget();

    let s = Rc::clone(&shared);
    let metrics = Closure::<dyn Fn() -> JsValue>::new(move || {
        let s = s.borrow();
        let turns: Vec<Value> = s
            .turns
            .iter()
            .map(|t| json!({ "turn": t.turn, "wallMs": t.wall_ms, "aiMs": t.ai_ms }))
            .collect();
        let out = to_js(&json!({
            "startGameAtMs": s.start_game_at,
            "frames": s.frames,
            "turns": turns,
            // The counters and rings of the server session's client (4.12); null in a local game.
            "client": s.snapshot.as_ref().and_then(|x| x.net.as_ref()).map(|n| stats_json(&n.stats)),
        }));
        // The frame stats and start-up timings of `platform_web::stats`, as they are.
        if let Some(w) = web_sys::window()
            && let Ok(stats) = js_sys::Reflect::get(&w, &"__civ5stats".into())
        {
            let _ = js_sys::Reflect::set(&out, &"stats".into(), &stats);
        }
        out
    });
    set("metrics", metrics.as_ref());
    metrics.forget();

    let s = Rc::clone(&shared);
    let digest = Closure::<dyn Fn() -> String>::new(move || {
        s.borrow().snapshot.as_ref().map(digest).unwrap_or_default()
    });
    set("_digestVisible", digest.as_ref());
    digest.forget();
    // `digest()`: the visible state's hash at once; `digest('replica')`: a promise of the replica's own digest
    // (`Replica::digest`, slow on a big game), computed in the next frame.
    let digest = js_sys::Function::new_with_args(
        "kind, timeoutMs",
        "const api = this;\n\
         if (kind !== 'replica') return api._digestVisible();\n\
         const c0 = api.state().ui?.replicaDigest?.count ?? 0, t0 = performance.now();\n\
         api.act({ kind: 'replicaDigest' });\n\
         return new Promise((ok, fail) => {\n\
           const tick = () => {\n\
             const d = api.state().ui?.replicaDigest;\n\
             if (d && d.count > c0) return ok(d.value);\n\
             if (performance.now() - t0 > (timeoutMs ?? 30000)) return fail(new Error('digest(replica) timed out'));\n\
             setTimeout(tick, 50);\n\
           };\n\
           tick();\n\
         });",
    );
    set("digest", &digest);

    let wait_turn = js_sys::Function::new_with_args(
        "n, timeoutMs",
        "const api = this, t0 = performance.now();\n\
         return new Promise((ok, fail) => {\n\
           const tick = () => {\n\
             const s = api.state();\n\
             if (s.fatal) return fail(new Error('fatal: ' + s.fatal));\n\
             if (s.turn !== null && s.turn >= n && s.status === 'human') return ok(s);\n\
             if (timeoutMs && performance.now() - t0 > timeoutMs) return fail(new Error('waitTurn(' + n + ') timed out at turn ' + s.turn));\n\
             setTimeout(tick, 50);\n\
           };\n\
           tick();\n\
         });",
    );
    set("waitTurn", &wait_turn);

    // `ui2()`: UI v2 as of its last frame (null without `&ui=v2`).
    if client_app::parse_ui2_param(platform_web::page::query_param("ui").as_deref()).is_some() {
        client_app::enable_ui2_probe();
    }
    let ui2 = Closure::<dyn Fn() -> JsValue>::new(|| {
        to_js(&client_app::ui2_probe().map_or(Value::Null, |p| ui2_json(&p)))
    });
    set("ui2", ui2.as_ref());
    ui2.forget();
    // `ui2Rects(parts, timeoutMs?)`: a promise of the rectangles of the elements of the `data-part` names `parts`
    // (`Runtime::dump_rects`, CSS px), from the next UI v2 frame.
    let ask = Closure::<dyn Fn(JsValue) -> Result<f64, JsValue>>::new(|v: JsValue| {
        let text: String = js_sys::JSON::stringify(&v)?.into();
        let parts: Vec<String> = match serde_json::from_str::<Value>(&text) {
            Ok(Value::String(s)) => vec![s],
            Ok(Value::Array(a)) => a
                .into_iter()
                .filter_map(|x| x.as_str().map(str::to_owned))
                .collect(),
            _ => {
                return Err(JsValue::from_str(
                    "ui2Rects: parts are a name or a list of names",
                ));
            }
        };
        Ok(client_app::ask_ui2_rects(parts) as f64)
    });
    set("_ui2RectsAsk", ask.as_ref());
    ask.forget();
    let get = Closure::<dyn Fn(f64) -> JsValue>::new(|seq: f64| to_js(&ui2_rects_json(seq as u64)));
    set("_ui2RectsGet", get.as_ref());
    get.forget();
    let rects = js_sys::Function::new_with_args(
        "parts, timeoutMs",
        "const api = this, seq = api._ui2RectsAsk(parts), t0 = performance.now();\n\
         return new Promise((ok, fail) => {\n\
           const tick = () => {\n\
             const r = api._ui2RectsGet(seq);\n\
             if (r !== null) return ok(r);\n\
             if (performance.now() - t0 > (timeoutMs ?? 10000)) return fail(new Error('ui2Rects timed out (is UI v2 on the screen?)'));\n\
             setTimeout(tick, 50);\n\
           };\n\
           tick();\n\
         });",
    );
    set("ui2Rects", &rects);

    let trap = Closure::<dyn Fn(String)>::new(test_trap);
    set("trap", trap.as_ref());
    trap.forget();

    js_sys::Reflect::set(&window, &GLOBAL.into(), &api).ok()?;
    tracing::info!("test hooks on: window.{GLOBAL}");
    Some(Box::new(Port(shared)))
}

/// `trap(kind)`: kills the module on purpose, for the check of the crash report (`bootstrap.js`). `page`: a
/// wasm trap on the page's thread (no panic); `workerPanic` / `workerTrap`: a panic / a trap on a worker of
/// the jobs pool while the page waits on the job (threaded build; without workers nothing happens).
fn test_trap(kind: String) {
    match kind.as_str() {
        "page" => core::arch::wasm32::unreachable(),
        "workerPanic" | "workerTrap" => {
            let panic = kind == "workerPanic";
            // The page's indices are quick, a worker's are slow and end in the failure: the page waits on a dead
            // worker's index without calling into JS (the freeze of the live report 2026-10-31, which
            // `jobs::check_alive` turns into a panic on the page).
            jobs::for_each(64, &|_| {
                if web_sys::window().is_none() {
                    let t0 = js_sys::Date::now();
                    while js_sys::Date::now() - t0 < 20.0 {}
                    if panic {
                        panic!("test: a jobs worker panics");
                    }
                    core::arch::wasm32::unreachable();
                }
            });
            tracing::warn!("trap({kind}): no worker took an index");
        }
        _ => tracing::warn!("trap({kind}): unknown kind (page, workerPanic, workerTrap)"),
    }
}

fn to_js(v: &Value) -> JsValue {
    js_sys::JSON::parse(&v.to_string()).unwrap_or(JsValue::NULL)
}

fn parse_action(v: &Value) -> Result<TestAction, String> {
    let kind = v["kind"].as_str().ok_or("act: no `kind`")?;
    let int = |k: &str| {
        v[k].as_i64()
            .ok_or_else(|| format!("act {kind}: `{k}` must be an integer"))
    };
    let flag = |k: &str| v[k].as_bool().unwrap_or(false);
    let text = |k: &str| v[k].as_str().map(str::to_owned);
    Ok(match kind {
        "startGame" => TestAction::StartGame {
            civ: text("civ"),
            handicap: text("handicap"),
            ai_count: v["ai"].as_u64().unwrap_or(1).clamp(1, 2) as u8,
            seed: v["seed"].as_u64().map(|s| s as u32),
        },
        "continueGame" => TestAction::ContinueGame,
        "endTurn" => TestAction::EndTurn {
            force: flag("force"),
        },
        "selectUnit" => TestAction::SelectUnit(int("id")? as i32),
        "openCity" => TestAction::OpenCity(int("id")? as i32),
        "click" => TestAction::MapClick {
            x: int("x")? as i32,
            y: int("y")? as i32,
            secondary: flag("secondary"),
        },
        "unitAction" => TestAction::UnitAction(int("action")? as i16),
        "chooseBuild" => TestAction::ChooseBuild(int("index")? as usize),
        "buyBuild" => TestAction::BuyBuild {
            index: int("index")? as usize,
            faith: flag("faith"),
        },
        "closeCity" => TestAction::CloseCity,
        "confirm" => TestAction::Confirm(flag("yes")),
        "reload" => TestAction::Reload,
        // The crash overlay of a panic (`platform_web::show_crash`): the playtest's `fatal` checks it.
        "panic" => panic!("test hook: a panic on request"),
        "fault" => TestAction::Fault(match v["fault"].as_str() {
            Some("badMessage") => Fault::BadMessage,
            Some("killHost") => Fault::KillHost,
            Some("dropSocket") => Fault::DropSocket,
            Some("corruptView") => Fault::CorruptView,
            Some("silence") => Fault::Silence {
                ms: v["ms"]
                    .as_u64()
                    .ok_or("act fault silence: `ms` must be an integer")?
                    .min(u64::from(u32::MAX)) as u32,
            },
            other => return Err(format!("act fault: unknown fault {other:?}")),
        }),
        // The server session's screens (lobby-ui 4.11): each is the click a player makes.
        "login" => TestAction::Ui(UiAction::Login {
            nick: text("nick").ok_or("act login: `nick` is required")?,
            password: text("password").unwrap_or_default(),
        }),
        "retry" => TestAction::Ui(UiAction::Retry),
        "signOut" => TestAction::Ui(UiAction::SignOut),
        "dismissNotice" => TestAction::Ui(UiAction::DismissNotice),
        "useThisTab" => TestAction::Ui(UiAction::UseThisTab),
        "createRoom" => TestAction::Ui(UiAction::CreateRoom),
        "joinRoom" => TestAction::Ui(UiAction::JoinRoom(int("room")? as u32)),
        "leaveRoom" => TestAction::Ui(UiAction::LeaveRoom),
        "giveUpSeat" => TestAction::Ui(UiAction::GiveUpSeat),
        "confirmGiveUp" => TestAction::Ui(UiAction::ConfirmGiveUp(flag("yes"))),
        "loadSave" => TestAction::Ui(UiAction::LoadSave(
            text("save").ok_or("act loadSave: `save` is required")?,
        )),
        "takeSeat" => TestAction::Ui(UiAction::TakeSeat {
            game: int("game")? as u32,
            player: int("player")? as i8,
        }),
        "roomSetting" => TestAction::Ui(UiAction::RoomEdit(parse_room_edit(v)?)),
        "slot" => TestAction::Ui(UiAction::SlotSet {
            index: int("index")? as u8,
            kind: parse_slot_kind(v["occupant"].as_str().unwrap_or(""))
                .ok_or("act slot: `occupant` is open, ai or closed")?,
            civ: text("civ"),
        }),
        "pickCiv" => TestAction::Ui(UiAction::PickCiv(text("civ"))),
        "takeSlot" => TestAction::Ui(UiAction::TakeSlot(int("index")? as u8)),
        "ready" => TestAction::Ui(UiAction::Ready(v["value"].as_bool().unwrap_or(true))),
        "start" => TestAction::Ui(UiAction::Start),
        "cancelEndTurn" => TestAction::Ui(UiAction::CancelEndTurn),
        "chat" => TestAction::Ui(UiAction::Chat {
            to: v["to"].as_i64().map(|p| p as i8),
            text: text("text").ok_or("act chat: `text` is required")?,
        }),
        "declareWar" => TestAction::Ui(UiAction::DeclareWar {
            team: int("team")? as i8,
        }),
        "proposePeace" => TestAction::Ui(UiAction::ProposePeace {
            to: int("to")? as i8,
        }),
        "answerPeace" => TestAction::Ui(UiAction::AnswerPeace {
            from: int("from")? as i8,
            accept: flag("accept"),
        }),
        "kick" => TestAction::Ui(UiAction::Kick {
            player: int("player")? as i8,
        }),
        "pause" => TestAction::Ui(UiAction::Pause(v["value"].as_bool().unwrap_or(true))),
        "minimapClick" => TestAction::Ui(UiAction::MinimapPointer {
            u: int("u")?.clamp(0, 65535) as u16,
            v: int("v")?.clamp(0, 65535) as u16,
            down: flag("down"),
        }),
        "setQuality" => TestAction::Ui(UiAction::SetQuality(match v["preset"].as_str() {
            Some("low") => QualityPreset::Low,
            Some("medium") => QualityPreset::Medium,
            Some("high") => QualityPreset::High,
            _ => return Err("act setQuality: `preset` is low, medium or high".to_owned()),
        })),
        // The game UI (`classic`, `v2`, `v2:<skin>`); `save`: as the settings window keeps it; `cold`: the classic UI
        // wakes up with a new layer.
        "setUi" => TestAction::SetUi {
            value: text("value").ok_or("act setUi: `value` is required")?,
            save: flag("save"),
            cold: v["cold"].as_bool(),
        },
        "replicaDigest" => TestAction::ReplicaDigest,
        "leaveGame" => TestAction::Ui(UiAction::LeaveGame),
        "toggleSettings" => TestAction::Ui(UiAction::ToggleSettings),
        "toggleStats" => {
            TestAction::Ui(UiAction::ToggleStats(v["value"].as_bool().unwrap_or(true)))
        }
        "quickStart" => TestAction::QuickStart(RoomParams {
            size: text("size"),
            ai: v["ai"].as_u64().unwrap_or(1).clamp(1, 20) as u8,
            civ: text("civ"),
            seed: v["seed"].as_u64().map(|s| s as u32),
            mode: match v["mode"].as_str() {
                Some("simultaneous") => TurnMode::Simultaneous,
                _ => TurnMode::Hybrid,
            },
            manual_choices: v["manual"].as_bool().unwrap_or_else(|| {
                crate::manual_choices(
                    platform_web::page::query_param("ui").as_deref() != Some("interim"),
                )
            }),
        }),
        other => return Err(format!("act: unknown kind {other:?}")),
    })
}

/// `roomSetting {field, value}`: one changed field of the room's settings (`name`, `worldSize`, `mapScript`,
/// `speed`, `era`, `handicap`, `minorCivs`, `turnMode` (`simultaneous` | `hybrid`), `timer` (`off` |
/// `original:<row>` | `fixed:<seconds>`), `option` (with `key`), `seed`, `mapSeed` (a number or null)).
fn parse_room_edit(v: &Value) -> Result<RoomEdit, String> {
    let field = v["field"].as_str().ok_or("act roomSetting: no `field`")?;
    let value = &v["value"];
    let text = || {
        value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("act roomSetting {field}: `value` must be a string"))
    };
    let seed = || value.as_u64().map(|s| s as u32);
    Ok(match field {
        "name" => RoomEdit::Name(text()?),
        "worldSize" => RoomEdit::WorldSize(text()?),
        "mapScript" => RoomEdit::MapScript(text()?),
        "speed" => RoomEdit::Speed(text()?),
        "era" => RoomEdit::Era(text()?),
        "handicap" => RoomEdit::Handicap(text()?),
        "minorCivs" => RoomEdit::MinorCivs(
            value
                .as_u64()
                .ok_or("act roomSetting minorCivs: `value` must be an integer")? as u8,
        ),
        "turnMode" => RoomEdit::TurnMode(match value.as_str() {
            Some("simultaneous") => TurnModeChoice::Simultaneous,
            Some("hybrid") => TurnModeChoice::Hybrid,
            _ => return Err("act roomSetting turnMode: simultaneous or hybrid".to_owned()),
        }),
        "timer" => RoomEdit::Timer(match text()?.as_str() {
            "off" => TimerChoice::Off,
            t if t.starts_with("original:") => TimerChoice::Original(t[9..].to_owned()),
            t if t.starts_with("fixed:") => TimerChoice::Fixed(
                t[6..]
                    .parse()
                    .map_err(|_| "act roomSetting timer: fixed:<seconds>".to_owned())?,
            ),
            _ => return Err("act roomSetting timer: off, original:<row>, fixed:<n>".to_owned()),
        }),
        "option" => RoomEdit::Option {
            key: v["key"]
                .as_str()
                .ok_or("act roomSetting option: no `key`")?
                .to_owned(),
            value: value
                .as_i64()
                .ok_or("act roomSetting option: `value` must be an integer")?
                as i32,
        },
        "seed" => RoomEdit::Seed(seed()),
        "mapSeed" => RoomEdit::MapSeed(seed()),
        other => return Err(format!("act roomSetting: unknown field {other:?}")),
    })
}

/// `ClientStats` as `metrics().client` (net-client 4.12).
fn stats_json(s: &ClientStats) -> Value {
    json!({
        "bytesIn": s.bytes_in,
        "bytesOut": s.bytes_out,
        "decodeErrors": s.decode_errors,
        "refusedOffline": s.refused_offline,
        "hostChanges": s.host_changes,
        "batches": s.batches,
        "dupBatches": s.dup_batches,
        "keyframes": s.keyframes,
        "diffs": s.diffs,
        "diffsSkipped": s.diffs_skipped,
        "undoRestores": s.undo_restores,
        "needKeyframe": {
            "hashMismatch": s.need_keyframe.hash_mismatch,
            "baseMismatch": s.need_keyframe.base_mismatch,
            "applyError": s.need_keyframe.apply_error,
            "decode": s.need_keyframe.decode,
            "basisMismatch": s.need_keyframe.basis_mismatch,
            "total": s.need_keyframe.total(),
        },
        "basisMsgs": s.basis_msgs,
        "basisBytes": s.basis_bytes,
        "attachedReplay": s.attached_replay,
        "attachedFresh": s.attached_fresh,
        "streamBreaks": s.stream_breaks,
        "orders": {
            "sent": s.orders_sent,
            "resent": s.orders_resent,
            "applied": s.orders_applied,
            "rejected": s.orders_rejected,
            "lost": s.orders_lost,
            "expired": s.orders_expired,
            "queued": s.orders_queued,
            "dropped": s.orders_dropped,
        },
        "dupResults": s.dup_results,
        "unknownResults": s.unknown_results,
        "queries": s.queries,
        "queriesAnswered": s.queries_answered,
        "queriesDropped": s.queries_dropped,
        "ctxFreshAtSelect": s.ctx_fresh_at_select,
        "ctxStaleAtSelect": s.ctx_stale_at_select,
        "ctxStaleAtOrder": s.ctx_stale_at_order,
        "focusSent": s.focus_sent,
        "layerRecords": s.layer_records,
        "layerRollbacks": s.layer_rollbacks,
        "layerStuck": s.layer_stuck,
        "endTurnCancelledByOrder": s.end_turn_cancelled_by_order,
        "rttMs": s.rtt_ring,
        "orderMs": s.order_ms_ring,
    })
}

fn net_json(n: &TestNet) -> Value {
    json!({
        "phase": n.phase,
        "nick": n.nick,
        "conn": n.conn,
        "connStatus": n.conn_status,
        "ordersEnabled": n.orders_enabled,
        "rttMs": n.rtt_ms,
        "viewSeq": n.view_seq,
        "viewHash": n.view_hash,
        "basisHash": n.basis_hash,
        "revision": n.revision,
        "inflight": n.inflight,
        "overlay": n.overlay,
        "ctx": n.ctx,
        "turnState": n.turn_state.as_ref().map(|t| json!({
            "turn": t.turn,
            "phase": t.phase,
            "mode": t.mode,
            "paused": t.paused,
            "seats": t.seats.iter().map(|(player, finished, may_act)| json!({
                "player": player, "finished": finished, "mayAct": may_act,
            })).collect::<Vec<_>>(),
        })),
        "seats": n.seats.iter().map(|(player, who)| json!({ "player": player, "who": who })).collect::<Vec<_>>(),
        "reconnect": n.reconnect.map(|(attempt, next_in_ms, probing)| json!({
            "attempt": attempt, "nextInMs": next_in_ms, "probing": probing,
        })),
        "loginError": n.login_error,
        "rooms": n.rooms.iter().map(|r| json!({
            "id": r.id, "name": r.name, "host": r.host, "players": r.players, "map": r.map,
            "starting": r.starting,
        })).collect::<Vec<_>>(),
        "games": n.games.iter().map(|g| json!({
            "id": g.id, "name": g.name, "turn": g.turn, "status": g.status,
            "seats": g.seats.iter().map(|(player, civ, who, takeable, yours)| json!({
                "player": player, "civ": civ, "who": who, "takeable": takeable, "yours": yours,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "saves": n.saves.iter().map(|s| json!({
            "id": s.id, "name": s.name, "turn": s.turn, "civs": s.civs,
        })).collect::<Vec<_>>(),
        "lobbyError": n.lobby_error,
        "notice": n.notice,
        "host": n.host,
        "room": n.room.as_ref().map(|r| json!({
            "host": r.host,
            "meHost": r.me_host,
            "starting": r.starting,
            "hasSlot": r.has_slot,
            "name": r.name,
            "worldSize": r.world_size,
            "mapScript": r.map_script,
            "speed": r.speed,
            "era": r.era,
            "handicap": r.handicap,
            "minorCivs": r.minor_civs,
            "turnMode": r.turn_mode,
            "timer": r.timer,
            "seed": r.seed,
            "mapSeed": r.map_seed,
            "options": r.options.iter().map(|(k, v)| json!({ "key": k, "value": v })).collect::<Vec<_>>(),
            "slots": r.slots.iter().map(|s| json!({
                "index": s.index, "who": s.who, "nick": s.nick, "ready": s.ready, "me": s.me,
                "civ": s.civ, "was": s.was, "yours": s.yours,
            })).collect::<Vec<_>>(),
            "canStart": r.can_start,
            "whyNot": r.why_not,
            "fromSave": r.from_save,
            "note": r.note,
            "error": r.error,
        })),
    })
}

/// `ui2()`: `{state, active, skin, pkg, loadMs, model: {rev}, frames, panels: [{id, owner, status, reason}],
/// scripts: [{ctx, status, errors, restarts}], reload: {rev, appliedAt, skin} | null, ui: {windows: [name],
/// state: {path: value}}, texts: [{part, text}], counters: {"ui2.*": n}}`; the counters are the renderer's and the
/// frame's, as `__civ5stats` has them. `reload` is the last skin change or hot reload on the screen (`appliedAt`:
/// `performance.now()`); `ui.state` is the skin's own `state.*`.
fn ui2_json(p: &client_app::V2Probe) -> Value {
    let texts: Vec<Value> = p
        .texts
        .iter()
        .map(|(part, text)| json!({ "part": part, "text": text }))
        .collect();
    let counters: serde_json::Map<String, Value> = client_app::render::perf::extra_counters()
        .into_iter()
        .filter(|(name, _)| name.starts_with("ui2."))
        .map(|(name, v)| (name, json!(v)))
        .collect();
    let panels: Vec<Value> = p
        .panels
        .iter()
        .map(|x| {
            json!({
                "id": x.name,
                "owner": x.owner,
                "status": if x.failed.is_some() { "placeholder" } else { "ok" },
                "reason": x.failed,
            })
        })
        .collect();
    let scripts: Vec<Value> = p
        .scripts
        .iter()
        .map(|s| json!({ "ctx": s.ctx, "status": s.status, "errors": s.errors, "restarts": s.restarts }))
        .collect();
    json!({
        "state": p.state,
        "active": p.active,
        "skin": p.skin,
        "pkg": p.pkg,
        "loadMs": p.load_ms,
        "model": { "rev": p.model_rev },
        "frames": p.frames,
        "panels": panels,
        "scripts": scripts,
        "reload": p.reload.as_ref().map(|r| json!({ "rev": r.rev, "appliedAt": r.applied_at, "skin": r.skin })),
        "ui": {
            "windows": p.windows,
            "state": p.skin_state.iter().cloned().collect::<serde_json::Map<String, Value>>(),
        },
        "texts": texts,
        "counters": counters,
    })
}

/// `state().ui`: `{active, choice, saved, skin, switching, switches, lastSwitchMs, failed: [{ui, why}], notice,
/// frames: {classic, v2}, inputs: {classic, v2}, replicaDigest: {count, value}, screen, screenShown}`.
fn ui_json(u: &client_app::test_port::TestUi) -> Value {
    json!({
        "active": u.active,
        "choice": u.choice,
        "saved": u.saved,
        "skin": u.skin,
        "switching": u.switching,
        "switches": u.switches,
        "lastSwitchMs": u.last_switch_ms,
        "failed": u.failed.iter().map(|(ui, why)| json!({ "ui": ui, "why": why })).collect::<Vec<_>>(),
        "notice": u.notice,
        "frames": { "classic": u.frames[0], "v2": u.frames[1] },
        "inputs": { "classic": u.inputs[0], "v2": u.inputs[1] },
        "replicaDigest": u.replica_digest.as_ref().map(|(count, value)| json!({ "count": count, "value": value })),
        "screen": u.screen,
        "screenShown": u.screen_shown,
    })
}

/// `_ui2Rects(seq)`: the rectangles of the answer `seq` (`[{part, node, keys, x, y, w, h}]`), null until the UI v2
/// frame answered it.
fn ui2_rects_json(seq: u64) -> Value {
    match client_app::ui2_probe().and_then(|p| p.rects) {
        Some((n, rects)) if n >= seq => Value::Array(
            rects
                .iter()
                .map(|r| {
                    json!({
                        "part": r.part, "node": r.node, "keys": r.keys,
                        "x": r.rect[0], "y": r.rect[1], "w": r.rect[2], "h": r.rect[3],
                    })
                })
                .collect(),
        ),
        _ => Value::Null,
    }
}

fn state_json(s: &Shared) -> Value {
    let mut v = s.snapshot.as_ref().map_or_else(
        || json!({ "screen": "none", "turn": null, "status": null, "fatal": null }),
        snapshot_json,
    );
    v["frames"] = json!(s.frames);
    v["asked"] = json!(s.asked);
    v["applied"] = json!(s.applied);
    v
}

fn snapshot_json(s: &TestSnapshot) -> Value {
    json!({
        "screen": s.screen,
        "loading": s.loading,
        "fatal": s.fatal,
        "lobby": {
            "civs": s.lobby_civs,
            "handicaps": s.lobby_handicaps,
            "save": s.lobby_save,
            "error": s.lobby_error,
        },
        "turn": s.turn,
        "year": s.year,
        "top": s.top.map(|(gold, gold_rate, science_rate)| json!({ "gold": gold, "goldRate": gold_rate, "scienceRate": science_rate })),
        "status": s.status,
        "blocker": s.blocker.as_ref().map(|(kind, target)| json!({ "kind": kind, "target": target })),
        "endTurn": s.end_turn,
        "endTurnNote": s.end_turn_note,
        "selectedUnit": s.selected_unit,
        "unitPanel": s.unit_panel.as_ref().map(|p| json!({
            "name": p.name,
            "loading": p.loading,
            "aiming": p.aiming,
            "actions": p.actions.iter().map(|a| json!({
                "action": a.action,
                "label": a.label,
                "enabled": a.enabled,
                "reason": a.reason,
            })).collect::<Vec<_>>(),
        })),
        "cityPanel": s.city_panel.as_ref().map(|c| json!({
            "name": c.name,
            "producing": c.producing,
            "loading": c.loading,
            "refused": c.refused,
            "gold": c.gold,
            "faith": c.faith,
            "builds": c.builds.iter().enumerate().map(|(i, b)| json!({
                "index": i,
                "name": b.name,
                "section": b.section,
                "turns": b.turns,
                "current": b.current,
                "gold": b.gold.map(|o| json!({ "cost": o.cost, "enabled": o.enabled })),
                "faith": b.faith.map(|o| json!({ "cost": o.cost, "enabled": o.enabled })),
            })).collect::<Vec<_>>(),
        })),
        "question": s.question,
        "tooltip": s.tooltip,
        "units": s.units.iter().map(|u| json!({
            "id": u.id,
            "name": u.name,
            "x": u.x,
            "y": u.y,
            "hp": u.hp,
            "movesLeft": u.moves_left,
            "activity": u.activity,
        })).collect::<Vec<_>>(),
        "cities": s.cities.iter().map(|c| json!({
            "id": c.id,
            "name": c.name,
            "x": c.x,
            "y": c.y,
            "population": c.population,
            "productionItem": c.production_item,
            "productionTurns": c.production_turns,
        })).collect::<Vec<_>>(),
        "rivalUnits": s.rival_units,
        "rivalCities": s.rival_cities,
        "players": s.players,
        "journal": s.journal,
        "worlds": s.worlds,
        "net": s.net.as_ref().map(net_json),
        "timer": s.timer.as_ref().map(|(text, stopped)| json!({ "text": text, "stopped": stopped })),
        "chat": s.chat,
        "minimap": s.minimap,
        "settingsOpen": s.settings_open,
        "quality": s.quality,
        "terrainCells": s.terrain_cells.map(|(visible, unpaged)| json!({ "visible": visible, "unpaged": unpaged })),
        // The unit and city models the render bridge gave the renderer: [owner, id, x, y].
        "mapUnits": s.map_units,
        "mapCities": s.map_cities,
        "ui": s.ui.as_ref().map(ui_json),
        "playersPanel": s.players_panel.as_ref().map(|p| json!({
            "open": p.open,
            "note": p.note,
            "ordersEnabled": p.orders_enabled,
            "rows": p.rows.iter().map(|r| json!({
                "player": r.player,
                "name": r.name,
                "me": r.me,
                "host": r.host,
                "hostPassesInS": r.host_passes_in_s,
                "turn": r.turn,
                "line": r.line,
                "diplomacy": r.diplomacy,
                "canKick": r.can_kick,
            })).collect::<Vec<_>>(),
        })),
    })
}

/// FNV-1a 64 over the turn and the human's units and cities, as text.
fn digest(s: &TestSnapshot) -> String {
    let mut text = format!("turn {:?};", s.turn);
    for u in &s.units {
        text.push_str(&format!("u{} {} {},{} hp{};", u.id, u.name, u.x, u.y, u.hp));
    }
    for c in &s.cities {
        text.push_str(&format!(
            "c{} {} {},{} pop{} build{};",
            c.id, c.name, c.x, c.y, c.population, c.production_item
        ));
    }
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}
