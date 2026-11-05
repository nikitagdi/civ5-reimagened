//! `net-bot ctl`: the runner drives a bot line by line. One JSON object per line on
//! stdin, `{"op": "...", "req": 7, ...}`; the answer is `{"req": 7, "ok": true, ...}` or `{"req": 7, "ok":
//! false, "error": "..."}` (`req` is the caller's tag; the `id` of an order is a field of `cmd`). The events
//! of the bot (`"ev": ...`) go to stdout on lines of their own.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;

use protocol::{ChatSend, Command, Focus, GameCtl, KeyframeReason, LobbyCmd, Query};
use serde_json::{Value, json};

use super::mission::{Mission, run_lobby};
use super::persona::Persona;
use super::policy::{Pace, Policy};
use super::{Bot, BotConfig, BotError};

/// What `ctl` does when the stream ends.
pub struct CtlOptions {
    /// Where `result.json` goes on `quit` and on the end of input.
    pub result: Option<PathBuf>,
    pub timeline: Option<PathBuf>,
}

type Cond = Box<dyn Fn(&Bot) -> bool>;

/// `{"turn": N}`, `{"phase": "Humans"}`, `{"seq": N}`, `{"view_hash": H}`, `{"conn": "attached"}`,
/// `{"result_id": N}`, `{"may_act": true}`.
fn parse_until(u: &Value) -> Result<Cond, String> {
    let obj = u.as_object().ok_or("until: expected an object")?;
    let (k, v) = obj.iter().next().ok_or("until: empty")?;
    let num = || v.as_u64().ok_or(format!("until.{k}: expected a number"));
    Ok(match k.as_str() {
        "turn" => {
            let n = i32::try_from(num()?).map_err(|e| e.to_string())?;
            Box::new(move |b| b.s.turn_number() >= n)
        }
        "phase" => {
            let p = v
                .as_str()
                .ok_or("until.phase: expected a string")?
                .to_owned();
            Box::new(move |b| {
                b.s.turn
                    .as_ref()
                    .is_some_and(|t| format!("{:?}", t.phase).starts_with(&p))
            })
        }
        "seq" => {
            let n = num()?;
            Box::new(move |b| b.s.last_seq >= n)
        }
        "view_hash" => {
            let n = num()?;
            Box::new(move |b| b.s.view.as_ref().is_some_and(|(_, h)| h.value() == n))
        }
        "result_id" => {
            let n = u32::try_from(num()?).map_err(|e| e.to_string())?;
            Box::new(move |b| b.s.ledger.outcome(n).is_some())
        }
        "autoplay" => Box::new(|b| b.play_done.is_some()),
        "may_act" => {
            let want = v.as_bool().ok_or("until.may_act: expected true or false")?;
            Box::new(move |b| b.s.may_act() == want)
        }
        "conn" => {
            let what = v
                .as_str()
                .ok_or("until.conn: expected a string")?
                .to_owned();
            match what.as_str() {
                "attached" => Box::new(|b| b.is_connected() && b.s.attach.is_some()),
                "lobby" => Box::new(|b| b.is_connected() && b.s.logged_in && b.s.attach.is_none()),
                "down" => Box::new(|b| !b.is_connected()),
                other => return Err(format!("until.conn: unknown state {other}")),
            }
        }
        other => return Err(format!("until: unknown condition {other}")),
    })
}

fn field<T: serde::de::DeserializeOwned>(req: &Value, name: &str) -> Result<T, String> {
    let v = req
        .get(name)
        .ok_or_else(|| format!("missing field {name}"))?;
    serde_json::from_value(v.clone()).map_err(|e| format!("field {name}: {e}"))
}

fn str_field(req: &Value, name: &str) -> Result<String, String> {
    req.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing string field {name}"))
}

fn bot_err(e: BotError) -> String {
    e.to_string()
}

pub struct Ctl {
    bot: Option<Bot>,
    opts: CtlOptions,
    quit: bool,
}

impl Ctl {
    pub fn new(opts: CtlOptions) -> Ctl {
        Ctl {
            bot: None,
            opts,
            quit: false,
        }
    }

    fn bot(&mut self) -> Result<&mut Bot, String> {
        self.bot
            .as_mut()
            .ok_or_else(|| "not connected: send `connect` first".to_owned())
    }

    /// Runs one request; the object returned is merged into the answer.
    pub fn exec(&mut self, req: &Value) -> Result<Value, String> {
        let op = req
            .get("op")
            .and_then(Value::as_str)
            .ok_or("missing `op`")?
            .to_owned();
        match op.as_str() {
            "connect" => {
                let url = str_field(req, "url")?;
                let nick = str_field(req, "nick")?;
                let password = match req.get("password_env").and_then(Value::as_str) {
                    Some(var) => std::env::var(var).map_err(|_| format!("{var} is not set"))?,
                    None => String::new(),
                };
                let mut cfg = BotConfig::new(&url, &nick, &password);
                cfg.token_file = req
                    .get("token_file")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                cfg.timeline = self.opts.timeline.clone();
                // `"register": false`: no account over HTTP first (the gateway is a fake that speaks WebSocket only).
                cfg.register = req.get("register").and_then(Value::as_bool).unwrap_or(true);
                // The runner decides when a bot comes back: no surprise reconnects in `ctl`.
                cfg.auto_reconnect = req
                    .get("auto_reconnect")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let mut bot = Bot::new(cfg);
                bot.connect(false, None).map_err(bot_err)?;
                self.bot = Some(bot);
                Ok(json!({}))
            }
            "enter" => {
                // The lobby part of a mission (`lobby`: host / guest / join / load): the answer comes when the
                // bot sits in a game with its first view. The other fields are those of a mission.
                let bot = self.bot()?;
                let mut m = req.clone();
                if let Some(o) = m.as_object_mut() {
                    o.insert("url".into(), json!(""));
                    o.insert("nick".into(), json!(bot.s.nick.clone()));
                }
                let m: Mission = serde_json::from_value(m).map_err(|e| format!("enter: {e}"))?;
                run_lobby(bot, &m).map_err(bot_err)?;
                Ok(json!({}))
            }
            "lobby" => {
                let cmd: LobbyCmd = field(req, "cmd")?;
                self.bot()?.send_lobby(cmd).map_err(bot_err)?;
                Ok(json!({}))
            }
            "cmd" => {
                let command: Command = field(req, "command")?;
                let id = req.get("id").and_then(Value::as_u64).map(|n| n as u32);
                let dup = req.get("dup").and_then(Value::as_u64).unwrap_or(0) as u32;
                let id = self.bot()?.send_cmd(command, id, dup).map_err(bot_err)?;
                Ok(json!({ "cmd_id": id }))
            }
            "focus" => {
                let focus: Focus = serde_json::from_value(json!({
                    "units": req.get("units").cloned().unwrap_or(json!([])),
                    "city": req.get("city").cloned().unwrap_or(Value::Null),
                }))
                .map_err(|e| format!("focus: {e}"))?;
                self.bot()?.send_focus(focus).map_err(bot_err)?;
                Ok(json!({}))
            }
            "query" => {
                let q: Query = field(req, "q")?;
                let id = self.bot()?.send_query(q).map_err(bot_err)?;
                Ok(json!({ "query_id": id }))
            }
            "chat" => {
                let chat: ChatSend = serde_json::from_value(json!({
                    "to": req.get("to").cloned().unwrap_or(json!("All")),
                    "text": req.get("text").cloned().unwrap_or(json!("")),
                }))
                .map_err(|e| format!("chat: {e}"))?;
                self.bot()?.send_chat(chat).map_err(bot_err)?;
                Ok(json!({}))
            }
            "gamectl" => {
                let ctl: GameCtl = field(req, "ctl")?;
                self.bot()?.send_game_ctl(ctl).map_err(bot_err)?;
                Ok(json!({}))
            }
            "need_keyframe" => {
                let reason: KeyframeReason = serde_json::from_value(
                    req.get("reason").cloned().unwrap_or(json!("HashMismatch")),
                )
                .map_err(|e| format!("need_keyframe: {e}"))?;
                self.bot()?.send_need_keyframe(reason).map_err(bot_err)?;
                Ok(json!({}))
            }
            "wait" => {
                let cond = parse_until(req.get("until").ok_or("missing `until`")?)?;
                let ms = req
                    .get("timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(30_000);
                let bot = self.bot()?;
                let met = bot
                    .wait_until(Duration::from_millis(ms), |b| cond(b))
                    .map_err(bot_err)?;
                Ok(json!({ "met": met, "state": bot.s.summary() }))
            }
            "state" => Ok(json!({ "state": self.bot()?.s.summary() })),
            "knob" => {
                let name = str_field(req, "name")?;
                let value = req.get("value").cloned().unwrap_or(Value::Null);
                let bot = self.bot()?;
                if name == "freeze_ms" {
                    let ms = value.as_u64().ok_or("knob freeze_ms: expected a number")?;
                    bot.freeze(ms);
                } else {
                    bot.s.knobs.set(&name, &value)?;
                }
                Ok(json!({}))
            }
            "autoplay" => {
                let turns = req.get("turns").and_then(Value::as_i64).unwrap_or(20) as i32;
                let pace: Pace = match req.get("pace") {
                    Some(p) => {
                        serde_json::from_value(p.clone()).map_err(|e| format!("pace: {e}"))?
                    }
                    None => Pace::default(),
                };
                let name = req
                    .get("persona")
                    .and_then(Value::as_str)
                    .unwrap_or("autoplay");
                let args = req.get("persona_args").cloned().unwrap_or(Value::Null);
                let persona = Persona::parse(name, &args)?;
                let seed = req.get("seed").and_then(Value::as_u64).unwrap_or(1);
                let timeout = req
                    .get("turn_timeout_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(120_000);
                let bot = self.bot()?;
                bot.play_done = None;
                bot.driver = Some(Box::new(Policy::new(
                    persona,
                    pace,
                    turns,
                    seed,
                    Duration::from_millis(timeout),
                )));
                Ok(json!({}))
            }
            "drop" => {
                self.bot()?.drop_link();
                Ok(json!({}))
            }
            "reconnect" => {
                let resume = req.get("resume").and_then(Value::as_bool).unwrap_or(true);
                self.bot()?
                    .reconnect(resume, "operator", true)
                    .map_err(bot_err)?;
                Ok(json!({}))
            }
            "stats" => Ok(json!({ "result": self.bot()?.result_json() })),
            "quit" => {
                self.quit = true;
                let result = self.bot.as_ref().map(Bot::result_json);
                Ok(json!({ "result": result }))
            }
            other => Err(format!("unknown op {other}")),
        }
    }

    fn write_result(&self) {
        if let (Some(path), Some(bot)) = (&self.opts.result, &self.bot) {
            let _ = std::fs::write(
                path,
                serde_json::to_vec_pretty(&bot.result_json()).unwrap_or_default(),
            );
        }
    }

    /// Reads requests from `input` until `quit` or the end of the input; answers and events go to `out`.
    pub fn run(
        &mut self,
        input: impl BufRead + Send + 'static,
        out: &mut impl Write,
    ) -> Result<(), BotError> {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            for line in input.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        loop {
            match rx.try_recv() {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let reply = match serde_json::from_str::<Value>(&line) {
                        Err(e) => json!({ "ok": false, "error": format!("bad json: {e}") }),
                        Ok(req) => {
                            let id = req.get("req").cloned();
                            let mut r = match self.exec(&req) {
                                Ok(Value::Object(mut m)) => {
                                    m.insert("ok".into(), json!(true));
                                    Value::Object(m)
                                }
                                Ok(_) => json!({ "ok": true }),
                                Err(e) => json!({ "ok": false, "error": e }),
                            };
                            if let (Some(id), Some(o)) = (id, r.as_object_mut()) {
                                o.insert("req".into(), id);
                            }
                            r
                        }
                    };
                    self.flush_events(out)?;
                    writeln!(out, "{reply}")?;
                    out.flush()?;
                    if self.quit {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => {
                    if let Some(bot) = self.bot.as_mut() {
                        bot.pump()?;
                    } else {
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    self.flush_events(out)?;
                }
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if let Some(bot) = self.bot.as_mut() {
            bot.close();
        }
        self.flush_events(out)?;
        self.write_result();
        Ok(())
    }

    fn flush_events(&mut self, out: &mut impl Write) -> Result<(), BotError> {
        if let Some(bot) = self.bot.as_mut() {
            for ev in bot.drain_events() {
                writeln!(out, "{ev}")?;
            }
            out.flush()?;
        }
        Ok(())
    }
}
