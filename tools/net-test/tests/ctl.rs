//! `net-bot ctl` against a fake gateway: every operation that needs no real game, the answers to bad JSON,
//! and a real WebSocket round trip (login, `Attached`, a keyframe, an order and its answer).

use std::io::Cursor;
use std::net::TcpListener;
use std::thread;

use net_test::bot::ctl::{Ctl, CtlOptions};
use protocol::{
    AttachMode, Attached, CatalogRef, ClientMsg, Codec, CommandOutcome, CommandResult, GameBatch,
    GameMsg, Keyframe, MapBasis, MapView, PlotBasisView, PlotView, SeatTurn, ServerMsg, Session,
    Token, TopBar, TurnMode, TurnPhase, TurnState, TurnStatus, ViewHash, WorldView, basis_hash_of,
};
use serde_json::{Value, json};
use tungstenite::Message;
use wire::{Encoder, Limits};

fn send(ws: &mut tungstenite::WebSocket<std::net::TcpStream>, m: &ServerMsg) {
    let body = protocol::Postcard.encode(m).unwrap();
    ws.send(Message::binary(Encoder::plain(&body))).unwrap();
}

fn world() -> (WorldView, MapBasis) {
    let mut b = MapBasis {
        game: 1,
        basis_hash: 0,
        width: 1,
        height: 1,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 1,
        plots: vec![PlotBasisView::default()],
    };
    b.basis_hash = basis_hash_of(&b);
    let v = WorldView {
        turn: 1,
        year: 0,
        human: 0,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker: None,
        map: MapView {
            width: 1,
            height: 1,
            wrap_x: false,
            wrap_y: false,
            plots: vec![PlotView::default()],
            terrain_seed: 1,
        },
        units: vec![],
        cities: vec![],
        players: vec![],
        top_bar: TopBar::default(),
        notifications: vec![],
        peace_offers: vec![],
        trade_routes: vec![],
    };
    (v, b)
}

/// A gateway that plays one seat: `LoggedIn`, `Attached`, a first batch, then answers every order with
/// `Applied` and every `Ping` with a `Pong`. Returns its URL.
fn fake_gateway() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let read = |ws: &mut tungstenite::WebSocket<std::net::TcpStream>| -> Option<ClientMsg> {
            loop {
                match ws.read() {
                    Ok(Message::Binary(b)) => {
                        return wire::decode::<ClientMsg>(&b, &Limits::GATEWAY).ok();
                    }
                    Ok(Message::Close(_)) | Err(_) => return None,
                    Ok(_) => {}
                }
            }
        };
        assert!(matches!(read(&mut ws), Some(ClientMsg::Hello { .. })));
        assert!(matches!(read(&mut ws), Some(ClientMsg::Login(_))));
        send(
            &mut ws,
            &ServerMsg::LoggedIn(Session {
                token: Some(Token([7; 16])),
                nick: "t".into(),
            }),
        );
        send(
            &mut ws,
            &ServerMsg::Attached(Attached {
                game: 1,
                player: 0,
                stream: 3,
                mode: AttachMode::Fresh,
                catalog: CatalogRef { hash: 0, len: 0 },
                last_cmd_id: 0,
            }),
        );
        let (view, basis) = world();
        let kf = Keyframe {
            hash: ViewHash::of(&view).value(),
            basis_hash: basis.basis_hash,
            view,
        };
        let ts = TurnState {
            turn: 1,
            phase: TurnPhase::Humans,
            mode: TurnMode::Simultaneous,
            seats: vec![SeatTurn {
                player: 0,
                finished: false,
                may_act: true,
                group: 0,
            }],
            timers: vec![],
            paused: false,
        };
        let mut seq = 1;
        send(
            &mut ws,
            &ServerMsg::Game(GameBatch {
                seq,
                msgs: vec![
                    GameMsg::MapBasis(basis),
                    GameMsg::Keyframe(kf),
                    GameMsg::TurnState(ts),
                ],
            }),
        );
        while let Some(m) = read(&mut ws) {
            match m {
                ClientMsg::Cmd(c) => {
                    seq += 1;
                    send(
                        &mut ws,
                        &ServerMsg::Game(GameBatch {
                            seq,
                            msgs: vec![GameMsg::Results(vec![CommandResult {
                                id: c.id,
                                outcome: CommandOutcome::Applied,
                            }])],
                        }),
                    );
                }
                ClientMsg::Ping { seq, client_ms } => send(
                    &mut ws,
                    &ServerMsg::Pong {
                        seq,
                        client_ms,
                        server_ms: 1,
                    },
                ),
                _ => {}
            }
        }
    });
    url
}

fn run_script(lines: &[Value]) -> Vec<Value> {
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    run_text(&text)
}

fn run_text(text: &str) -> Vec<Value> {
    let mut ctl = Ctl::new(CtlOptions {
        result: None,
        timeline: None,
    });
    let mut out = Vec::new();
    ctl.run(Cursor::new(text.to_owned().into_bytes()), &mut out)
        .unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn answer(out: &[Value], req: u64) -> &Value {
    out.iter()
        .find(|v| v.get("req").and_then(Value::as_u64) == Some(req))
        .unwrap_or_else(|| panic!("no answer to req {req} in {out:?}"))
}

#[test]
fn bad_input_is_answered_not_fatal() {
    let out = run_text(
        "this is not json\n{\"op\":\"bogus\",\"req\":2}\n{\"op\":\"state\",\"req\":3}\n{\"req\":4}\n",
    );
    assert_eq!(out.len(), 4);
    assert_eq!(out[0]["ok"], false);
    assert!(out[0]["error"].as_str().unwrap().starts_with("bad json"));
    assert!(
        answer(&out, 2)["error"]
            .as_str()
            .unwrap()
            .contains("unknown op")
    );
    assert!(
        answer(&out, 3)["error"]
            .as_str()
            .unwrap()
            .contains("not connected")
    );
    assert!(answer(&out, 4)["error"].as_str().unwrap().contains("op"));
}

#[test]
fn connect_play_an_order_and_read_the_result() {
    let url = fake_gateway();
    let out = run_script(&[
        json!({"op": "connect", "url": url, "nick": "t", "register": false, "req": 1}),
        json!({"op": "wait", "until": {"may_act": true}, "timeout_ms": 10000, "req": 2}),
        json!({"op": "state", "req": 3}),
        json!({"op": "cmd", "command": {"UnitSleep": {"unit": 1}}, "req": 4}),
        json!({"op": "wait", "until": {"result_id": 1}, "timeout_ms": 10000, "req": 5}),
        json!({"op": "cmd", "command": {"EndTurn": {"force": false}}, "id": 2, "dup": 1, "req": 6}),
        json!({"op": "wait", "until": {"result_id": 2}, "timeout_ms": 10000, "req": 7}),
        json!({"op": "knob", "name": "claim_basis", "value": "none", "req": 8}),
        json!({"op": "knob", "name": "nonsense", "value": 1, "req": 9}),
        json!({"op": "knob", "name": "reconnect", "value": "probe", "req": 10}),
        json!({"op": "gamectl", "ctl": "Leave", "req": 11}),
        json!({"op": "stats", "req": 12}),
        json!({"op": "quit", "req": 13}),
    ]);
    assert_eq!(answer(&out, 1)["ok"], true);
    assert_eq!(answer(&out, 2)["met"], true);
    let st = &answer(&out, 3)["state"];
    assert_eq!(st["turn"], 1);
    assert_eq!(st["attached"]["player"], 0);
    assert_eq!(answer(&out, 4)["cmd_id"], 1);
    assert_eq!(answer(&out, 5)["met"], true);
    assert_eq!(answer(&out, 6)["cmd_id"], 2);
    assert_eq!(answer(&out, 7)["met"], true);
    assert_eq!(answer(&out, 8)["ok"], true);
    assert_eq!(answer(&out, 9)["ok"], false);
    assert_eq!(
        answer(&out, 10)["ok"],
        false,
        "the probe policy waits for net-client"
    );

    let r = &answer(&out, 12)["result"];
    assert_eq!(r["schema"], 1);
    assert_eq!(r["cmds"]["unique"], 2);
    assert_eq!(r["cmds"]["applied"], 2);
    assert_eq!(r["cmds"]["dup_sent"], 1);
    assert_eq!(
        r["ledger"]["double_outcome"], 0,
        "the duplicate's answer is a repeat"
    );
    assert_eq!(r["hash"]["checked"], 1);
    assert_eq!(r["hash"]["mismatch"], 0);
    assert_eq!(r["bytes"]["basis"]["n"], 1);
    assert_eq!(r["bytes"]["cold_batches"]["n"], 1);
    assert_eq!(r["errors"], json!([]));
    assert_eq!(r["modes"], json!(["Fresh"]));
    // The events went out on lines of their own.
    assert!(out.iter().any(|v| v["ev"] == "attached"));
    assert!(out.iter().any(|v| v["ev"] == "batch"));
    assert!(out.iter().any(|v| v["ev"] == "result"));
    assert!(answer(&out, 13)["result"]["nick"] == "t");
}
