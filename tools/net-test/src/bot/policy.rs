//! The autoplay policy (a port of `tools/playtest/lib/policy.mjs` to
//! the stream of protocol v3). One order at a time: it sends an order, waits for its final answer (the new
//! view comes in the same batch or earlier) and decides again from the view it holds.
//!
//! In a turn, while the seat may act:
//! 1. Focus like a client: the first 3 units that need orders, by `id`, and the city that blocks the turn. The
//!    context of the unit in focus comes by itself; the policy waits `RTT + 150 ms` for it and asks with a
//!    `Query` if it does not come (`queries_sent`).
//! 2. A unit gets the first enabled order of: found a city, automate, fortify, sleep, skip. Once per game a
//!    military unit takes a step along its move tree; a stacked unit steps aside.
//! 3. A city with no production gets a build from the list: Settler, Warrior, Monument, Worker, Scout.
//! 4. `EndTurn`. If the blocker does not change for three tries at it (orders for its unit or city, ends of
//!    turn; orders for other units do not count) or after 40 steps: `EndTurn { force: true }`; three forced
//!    ends refused in a row mean the game is stuck, and the error names the core's reason and the blocker.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use protocol::{
    Blocker, BlockerTarget, Catalog, CityBuildFlags, CityBuildView, CmdId, Codec, Command, Focus,
    Postcard, Query, QueryAnswer, ReachFlags, ReachNode, TurnPhase, TurnState, UnitActionView,
    UnitFlags, WorldView, xxh64,
};
use serde::Deserialize;
use serde_json::json;

use super::ledger::Outcome;
use super::persona::{Act, Persona};
use super::{Bot, BotError};

/// `gc-abi-v1` unit commands (`unit_panel::action_info`).
pub const FOUND_CITY: i16 = 3;
pub const FORTIFY: i16 = 4;
pub const SLEEP: i16 = 5;
pub const SKIP: i16 = 6;
pub const AUTOMATE: i16 = 8;
/// The orders tried for a unit that needs one, the first enabled one wins.
const ORDER_PREFERENCE: [i16; 5] = [FOUND_CITY, AUTOMATE, FORTIFY, SLEEP, SKIP];
/// The builds the policy prefers (the end of the catalog type name), else the first one that is not current.
const BUILD_PREFERENCE: [&str; 5] = ["SETTLER", "WARRIOR", "MONUMENT", "WORKER", "SCOUT"];
/// `EndTurnBlockingTypes`: a city without production, units stacked on one plot.
pub const BLOCKING_PRODUCTION: i32 = 2;
pub const BLOCKING_STACKED_UNITS: i32 = 7;
const MAX_FORCED_REFUSED: u32 = 3;
const STEPS_PER_TURN: u32 = 40;
/// An order with no final answer for this long is a stuck game.
const ORDER_TIMEOUT: Duration = Duration::from_secs(30);
/// A query with no answer for this long counts as failed.
const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// The pace of the bot (NT-D11): `burst` is without pauses; `human` pauses 150 to 800 ms between orders.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pace {
    #[default]
    Burst,
    Human,
}

/// Why the policy stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Done {
    /// The turns asked for were played.
    Turns,
    GameOver {
        turn: i32,
    },
    /// Nothing moves the turn on.
    Stuck {
        turn: i32,
    },
}

impl Done {
    pub fn text(&self) -> String {
        match self {
            Done::Turns => "turns".into(),
            Done::GameOver { turn } => format!("game_over:{turn}"),
            Done::Stuck { turn } => format!("stuck:{turn}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// Nothing to do now (waiting for the server, the pace or the other seats).
    Idle,
    Acted,
    Done(Done),
}

// ---------------------------------------------------------------- pure choices

/// The command of an action number of `gc-abi-v1` 4.4, for a unit.
pub fn command_for(action: i16, unit: i32) -> Option<Command> {
    Some(match action {
        FOUND_CITY => Command::UnitFoundCity { unit },
        FORTIFY => Command::UnitFortify { unit },
        SLEEP => Command::UnitSleep { unit },
        SKIP => Command::UnitSkip { unit },
        AUTOMATE => Command::UnitAutomateBuild { unit },
        _ => return None,
    })
}

/// The first enabled action of the preference list.
pub fn pick_order(actions: &[UnitActionView]) -> Option<i16> {
    // `NETTEST_BOT_NO_AUTOMATE`: no unit is automated, fortified or put to sleep; they only skip, so that some
    // unit needs orders at the start of every turn. A human with nothing to do is "in auto moves" in the core, and
    // a save of such a human cannot hand its slot to the AI (`gc_slot_control` Kick refuses: "a human is in auto
    // moves"; pumping does not clear it), which a demo save needs.
    let plain = std::env::var_os("NETTEST_BOT_NO_AUTOMATE").is_some();
    ORDER_PREFERENCE
        .into_iter()
        .filter(|a| !plain || *a == FOUND_CITY || *a == SKIP)
        .find(|a| actions.iter().any(|x| x.action == *a && x.enabled))
}

/// The name of a build in the catalog (`UNIT_SETTLER`).
fn build_name<'a>(b: &CityBuildView, cat: &'a Catalog) -> Option<&'a str> {
    let list = if b.order_type == 0 {
        &cat.units
    } else {
        &cat.buildings
    };
    list.iter()
        .find(|e| e.id == b.item)
        .map(|e| e.type_name.as_str())
}

/// A build for a city: by the preference list when the catalog names them, else the first enabled one that
/// is not the current one.
pub fn pick_build<'a>(
    builds: &'a [CityBuildView],
    cat: Option<&Catalog>,
) -> Option<&'a CityBuildView> {
    let usable = |b: &&CityBuildView| {
        b.flags.contains(CityBuildFlags::ENABLED) && !b.flags.contains(CityBuildFlags::CURRENT)
    };
    if let Some(cat) = cat {
        for want in BUILD_PREFERENCE {
            let hit = builds
                .iter()
                .filter(usable)
                .find(|b| build_name(b, cat).is_some_and(|n| n.to_uppercase().ends_with(want)));
            if hit.is_some() {
                return hit;
            }
        }
    }
    builds.iter().find(usable)
}

/// What the client asks the server to prefetch: the first 3 units that need orders (not in `skip`), by
/// `id`, and the city of a production blocker.
pub fn want_focus(view: &WorldView, skip: &BTreeSet<i32>) -> Focus {
    let mut units: Vec<i32> = view
        .units
        .iter()
        .filter(|u| {
            u.owner == view.human
                && u.flags.contains(UnitFlags::NEEDS_ORDERS)
                && !skip.contains(&u.id)
        })
        .map(|u| u.id)
        .collect();
    units.sort_unstable();
    // A stack that blocks the end of the turn: the units on the plot of the named one that can still move come first
    // (the named one may be asleep, or out of moves; another of the stack steps aside), also when they need no orders.
    if let Some(at) = stack_plot(view) {
        let mut stack: Vec<i32> = view
            .units
            .iter()
            .filter(|u| {
                u.owner == view.human
                    && (u.x, u.y) == at
                    && u.moves_left > 0
                    && !skip.contains(&u.id)
            })
            .map(|u| u.id)
            .collect();
        stack.sort_unstable();
        units.retain(|id| !stack.contains(id));
        stack.extend(units);
        units = stack;
    }
    units.truncate(3);
    let city = match view.blocker {
        Some(Blocker {
            kind: BLOCKING_PRODUCTION,
            target: BlockerTarget::City(id),
        }) => Some(id),
        _ => None,
    };
    Focus { units, city }
}

/// The plot of the unit that the "stacked units" blocker names, if the turn is blocked by it.
fn stack_plot(view: &WorldView) -> Option<(i16, i16)> {
    match view.blocker {
        Some(Blocker {
            kind: BLOCKING_STACKED_UNITS,
            target: BlockerTarget::Unit(id),
        }) => view
            .units
            .iter()
            .find(|u| u.id == id && u.owner == view.human)
            .map(|u| (u.x, u.y)),
        _ => None,
    }
}

/// The first node of the move tree that is a plain step this turn (not an attack), if any.
pub fn step_node(reach: &[ReachNode]) -> Option<&ReachNode> {
    reach
        .iter()
        .skip(1)
        .find(|n| n.turn == 0 && !n.flags.contains(ReachFlags::ATTACK))
}

/// `GET /catalog/{hash}` of the gateway the bot talks to (NP-D14), with a plain HTTP/1.0 request.
fn fetch_catalog(url: &str, hash: u64, len: u32) -> Result<Catalog, String> {
    let host = url
        .strip_prefix("ws://")
        .and_then(|r| r.split('/').next())
        .ok_or("the catalog needs a ws:// url")?;
    let mut s = TcpStream::connect(host).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    write!(
        s,
        "GET /catalog/{hash:016x} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("no HTTP header")?;
    let status = String::from_utf8_lossy(&raw[..split.min(32)]).to_string();
    if !status.contains(" 200") {
        return Err(format!("HTTP: {}", status.lines().next().unwrap_or("")));
    }
    let body = &raw[split + 4..];
    if xxh64(0, body) != hash {
        return Err("the catalog does not match its hash".into());
    }
    if body.len() != len as usize {
        return Err(format!(
            "the catalog is {} bytes, {len} announced",
            body.len()
        ));
    }
    let rest = wire::catalog::strip_catalog_header(body).map_err(|e| e.to_string())?;
    Postcard.decode::<Catalog>(rest).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- the policy

#[derive(Debug, Clone, Copy)]
enum Kind {
    /// An order that must not be refused: a refusal puts the unit aside for the turn.
    Regular(i32),
    /// A move (exploring, stepping aside), a build, a persona's act.
    Side,
    EndTurn {
        force: bool,
    },
}

#[derive(Debug)]
struct Waiting {
    id: CmdId,
    kind: Kind,
    since: Instant,
    /// The blocker of the turn when the order went out.
    blocker: Option<Blocker>,
    /// The order was for that blocker (its unit or city, an end of turn): one that leaves it counts as a try.
    counts: bool,
}

fn blocker_of(bot: &Bot) -> Option<Blocker> {
    bot.s.view.as_ref().and_then(|(v, _)| v.blocker)
}

#[derive(Debug)]
struct PendingQuery {
    id: u32,
    /// The unit or the city the answer is for.
    unit: Option<i32>,
    since: Instant,
}

/// What the policy decided to do.
enum Step {
    Wait,
    Focus(Focus),
    Query(Query, Option<i32>),
    Order {
        cmd: Command,
        kind: Kind,
        from_context: bool,
        /// The unit or city the order is for.
        subject: BlockerTarget,
    },
    Act(Act),
    EndTurn {
        force: bool,
    },
}

/// A small generator: the policy needs no randomness but the pauses of the `human` pace.
#[derive(Debug)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

pub struct Policy {
    pub persona: Persona,
    pub pace: Pace,
    turns: i32,
    turn_timeout: Duration,
    rng: Rng,
    start_turn: Option<i32>,
    cur_turn: i32,
    turn_started: Instant,
    steps: u32,
    refused_forced: u32,
    /// Orders in a row that left the blocker as it was.
    same_blocker: u32,
    explored: bool,
    bad_units: BTreeSet<i32>,
    waiting: Option<Waiting>,
    next_at: Instant,
    focus_sent: Option<Focus>,
    focus_at: Instant,
    query: Option<PendingQuery>,
    catalog: Option<Option<Catalog>>,
}

impl Policy {
    /// Plays `turns` turns from the first one the seat acts in.
    pub fn new(
        persona: Persona,
        pace: Pace,
        turns: i32,
        seed: u64,
        turn_timeout: Duration,
    ) -> Policy {
        let now = Instant::now();
        Policy {
            persona,
            pace,
            turns,
            turn_timeout,
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            start_turn: None,
            cur_turn: i32::MIN,
            turn_started: now,
            steps: 0,
            refused_forced: 0,
            same_blocker: 0,
            explored: false,
            bad_units: BTreeSet::new(),
            waiting: None,
            next_at: now,
            focus_sent: None,
            focus_at: now,
            query: None,
            catalog: None,
        }
    }

    fn begin_turn(&mut self, turn: i32, now: Instant) {
        self.cur_turn = turn;
        self.turn_started = now;
        self.steps = 0;
        self.refused_forced = 0;
        self.same_blocker = 0;
        self.bad_units.clear();
        self.waiting = None;
        self.focus_sent = None;
        self.query = None;
    }

    fn pause(&mut self, bot: &Bot) -> Duration {
        let human = match self.pace {
            Pace::Burst => 0,
            Pace::Human => self.rng.range(150, 800),
        };
        Duration::from_millis(human + bot.s.knobs.hold_ms)
    }

    /// One step of the policy: at most one message goes out.
    pub fn tick(&mut self, bot: &mut Bot) -> Result<Progress, BotError> {
        let now = Instant::now();
        if bot.s.attach.is_none() {
            return Ok(Progress::Idle);
        }
        let Some(ts) = bot.s.turn.clone() else {
            return Ok(Progress::Idle);
        };
        if let TurnPhase::GameOver { .. } = ts.phase {
            return Ok(Progress::Done(Done::GameOver { turn: ts.turn }));
        }
        let start = match self.start_turn {
            Some(s) => s,
            None if ts.phase == TurnPhase::Humans => {
                self.start_turn = Some(ts.turn);
                ts.turn
            }
            None => return Ok(Progress::Idle),
        };
        if ts.turn >= start + self.turns {
            // The turn counter moves when the humans end the turn, before the AI phase runs (the turn timeline,
            // AiTurn queue): the round is over only when the next human phase has begun. Stopping in the AI
            // phase left the gateway's copy of the view one AI phase ahead of the bot's final hash.
            if ts.phase != TurnPhase::Humans {
                return Ok(Progress::Idle);
            }
            return Ok(Progress::Done(Done::Turns));
        }
        if ts.turn != self.cur_turn {
            self.begin_turn(ts.turn, now);
        }
        if now.duration_since(self.turn_started) > self.turn_timeout {
            bot.s.stats.error(format!(
                "turn {} did not end in {} s",
                ts.turn,
                self.turn_timeout.as_secs()
            ));
            return Ok(Progress::Done(Done::Stuck { turn: ts.turn }));
        }
        // The answer to the last order first: it is in the view already.
        if let Some(w) = &self.waiting {
            match bot.s.ledger.outcome(w.id).cloned() {
                Some(out) => {
                    let w = self.waiting.take().expect("checked above");
                    // An order for the blocker (or an end of turn) that leaves it as it was is one more try of the
                    // same. An order for another unit does not count: a third start unit's orders under a
                    // production blocker would otherwise force an end of turn the core refuses (NP28).
                    if blocker_of(bot) == w.blocker {
                        if w.counts {
                            self.same_blocker += 1;
                        }
                    } else {
                        self.same_blocker = 0;
                    }
                    if let Outcome::Rejected(_) = out {
                        match w.kind {
                            Kind::Regular(unit) => {
                                self.bad_units.insert(unit);
                            }
                            Kind::EndTurn { force: true } => self.refused_forced += 1,
                            _ => {}
                        }
                    }
                    let p = self.pause(bot);
                    self.next_at = now + p;
                }
                None => {
                    if w.since.elapsed() > ORDER_TIMEOUT {
                        bot.s
                            .stats
                            .error(format!("order {} has no answer after 30 s", w.id));
                        return Ok(Progress::Done(Done::Stuck { turn: ts.turn }));
                    }
                    return Ok(Progress::Idle);
                }
            }
        }
        if self.refused_forced >= MAX_FORCED_REFUSED {
            let why = bot
                .s
                .last_reject
                .as_ref()
                .map_or("no reason given", |(_, r)| r.as_str());
            bot.s.stats.error(format!(
                "stuck in turn {}: forced ends of turn are refused: {why} (blocker {:?})",
                ts.turn,
                blocker_of(bot)
            ));
            return Ok(Progress::Done(Done::Stuck { turn: ts.turn }));
        }
        if ts.phase != TurnPhase::Humans
            || !bot.s.may_act()
            || bot.s.awaiting_keyframe
            || now < self.next_at
        {
            return Ok(Progress::Idle);
        }
        let step = self.plan(bot, &ts, now);
        self.execute(bot, step, now)
    }

    fn plan(&mut self, bot: &Bot, ts: &TurnState, now: Instant) -> Step {
        let Some((view, _)) = bot.s.view.as_ref() else {
            return Step::Wait;
        };
        if self.persona.is_idle() {
            return Step::EndTurn { force: false };
        }
        if let Some(act) = self.persona.plan(view, ts, &bot.cfg.nick) {
            return Step::Act(act);
        }
        // Last resort: a forced end was refused because of a stack that cannot step aside (all its units are
        // out of moves or boxed in). One unit of the stack is disbanded so that a long game goes on.
        if self.refused_forced >= 1
            && let Some(at) = stack_plot(view)
            && let Some(u) = view
                .units
                .iter()
                .filter(|u| u.owner == view.human && (u.x, u.y) == at)
                .max_by_key(|u| u.id)
        {
            return Step::Order {
                cmd: Command::UnitDisband { unit: u.id },
                kind: Kind::Side,
                from_context: false,
                subject: BlockerTarget::Unit(u.id),
            };
        }
        // The blocker that does not change: a forced end after three tries or 40 steps.
        if self.steps >= STEPS_PER_TURN || self.same_blocker >= 3 {
            return Step::EndTurn { force: true };
        }
        let want = want_focus(view, &self.bad_units);
        if self.focus_sent.as_ref() != Some(&want) {
            return Step::Focus(want);
        }
        if let Some(unit) = want.units.first().copied() {
            return self.plan_unit(bot, view, unit, now);
        }
        if let Some(city) = want.city {
            return self.plan_city(bot, city, now);
        }
        Step::EndTurn { force: false }
    }

    /// How long the context of the unit in focus is waited for: `RTT + 150 ms`.
    fn context_wait(&self, bot: &Bot) -> Duration {
        let rtt = bot.s.stats.rtt_ms.summary().p50.unwrap_or(0.0);
        Duration::from_millis(150) + Duration::from_secs_f64(rtt / 1000.0)
    }

    fn plan_unit(&mut self, bot: &Bot, view: &WorldView, unit: i32, now: Instant) -> Step {
        let Some(u) = view
            .units
            .iter()
            .find(|u| u.owner == view.human && u.id == unit)
        else {
            return Step::Wait;
        };
        let ctx = bot.s.contexts.unit(unit, bot.s.view_seq);
        let (actions, reach, from_context) = match ctx {
            Some(c) => (c.actions.clone(), Some(c.reach.clone()), true),
            None => match self.answer_or_ask(bot, Query::UnitActions { unit }, Some(unit), now) {
                Asked::Wait => return Step::Wait,
                Asked::Send(q) => return Step::Query(q, Some(unit)),
                Asked::Answer(QueryAnswer::UnitActions(a)) => (a, None, false),
                // A failed or odd answer: the order goes without a list (skip is always possible).
                Asked::Answer(_) | Asked::Failed => (Vec::new(), None, false),
            },
        };
        let order = pick_order(&actions);
        let military = order.is_some_and(|a| a != FOUND_CITY && a != AUTOMATE);
        // The unit stands in the stack that blocks the turn (it need not be the one the blocker names).
        let stacked = stack_plot(view) == Some((u.x, u.y));
        let step = reach.as_deref().and_then(step_node);
        if stacked
            && (u.moves_left <= 0 || step.is_none())
            && !u.flags.contains(UnitFlags::NEEDS_ORDERS)
        {
            // It cannot step aside and wants no order: another of the stack is tried.
            self.bad_units.insert(unit);
            return Step::Wait;
        }
        if u.moves_left > 0
            && (stacked || (!self.explored && military))
            && let Some(n) = step
        {
            // Stacked units: one steps aside. Once per game a military unit walks a step too.
            self.explored |= !stacked;
            return Step::Order {
                cmd: Command::UnitMoveTo {
                    unit,
                    x: n.x,
                    y: n.y,
                },
                kind: Kind::Side,
                from_context,
                subject: BlockerTarget::Unit(unit),
            };
        }
        let cmd = order
            .and_then(|a| command_for(a, unit))
            .unwrap_or(Command::UnitSkip { unit });
        Step::Order {
            cmd,
            kind: Kind::Regular(unit),
            from_context,
            subject: BlockerTarget::Unit(unit),
        }
    }

    fn plan_city(&mut self, bot: &Bot, city: i32, now: Instant) -> Step {
        let ctx = bot.s.contexts.city(city, bot.s.view_seq);
        let (builds, from_context) = match ctx {
            Some(c) => (c.builds.clone(), true),
            None => match self.answer_or_ask(bot, Query::CityBuilds { city }, Some(city), now) {
                Asked::Wait => return Step::Wait,
                Asked::Send(q) => return Step::Query(q, Some(city)),
                Asked::Answer(QueryAnswer::CityBuilds(b)) => (b, false),
                Asked::Answer(_) | Asked::Failed => return Step::EndTurn { force: false },
            },
        };
        let catalog = self.catalog_of(bot);
        match pick_build(&builds, catalog) {
            Some(b) => Step::Order {
                cmd: Command::CitySetProduction {
                    city,
                    order: b.order_type,
                    item: b.item,
                },
                kind: Kind::Side,
                from_context,
                subject: BlockerTarget::City(city),
            },
            None => Step::EndTurn { force: false },
        }
    }

    /// The catalog of the game, fetched once (`None` when the gateway does not give it).
    fn catalog_of(&mut self, bot: &Bot) -> Option<&Catalog> {
        if self.catalog.is_none() {
            let got = bot
                .s
                .attach
                .as_ref()
                .map(|a| fetch_catalog(&bot.cfg.url, a.catalog.hash, a.catalog.len));
            self.catalog = Some(match got {
                Some(Ok(c)) => Some(c),
                _ => None,
            });
        }
        self.catalog.as_ref().and_then(Option::as_ref)
    }

    /// The answer to the query for `subject`, or the query to send, or wait.
    fn answer_or_ask(&mut self, bot: &Bot, q: Query, subject: Option<i32>, now: Instant) -> Asked {
        if let Some(p) = &self.query
            && p.unit == subject
        {
            return match bot.s.query_answers.get(&p.id) {
                Some(QueryAnswer::Error { .. }) => Asked::Failed,
                Some(a) => Asked::Answer(a.clone()),
                None if p.since.elapsed() > QUERY_TIMEOUT => Asked::Failed,
                None => Asked::Wait,
            };
        }
        // The context may still come: wait for it before asking.
        if now.duration_since(self.focus_at) < self.context_wait(bot) {
            return Asked::Wait;
        }
        Asked::Send(q)
    }

    fn execute(&mut self, bot: &mut Bot, step: Step, now: Instant) -> Result<Progress, BotError> {
        match step {
            Step::Wait => Ok(Progress::Idle),
            Step::Focus(f) => {
                bot.send_focus(f.clone())?;
                self.focus_sent = Some(f);
                self.focus_at = now;
                self.query = None;
                Ok(Progress::Acted)
            }
            Step::Query(q, subject) => {
                let id = bot.send_query(q)?;
                self.query = Some(PendingQuery {
                    id,
                    unit: subject,
                    since: now,
                });
                Ok(Progress::Acted)
            }
            Step::Order {
                cmd,
                kind,
                from_context,
                subject,
            } => {
                if !bot.can_send() {
                    return Ok(Progress::Idle);
                }
                if from_context {
                    bot.s.stats.contexts_used += 1;
                }
                let id = bot.send_cmd(cmd, None, 0)?;
                let blocker = blocker_of(bot);
                self.waiting = Some(Waiting {
                    id,
                    kind,
                    since: now,
                    blocker,
                    counts: blocker.is_none_or(|b| b.target == subject),
                });
                self.steps += 1;
                Ok(Progress::Acted)
            }
            Step::Act(act) => {
                match act {
                    Act::Cmd(cmd) => {
                        if !bot.can_send() {
                            return Ok(Progress::Idle);
                        }
                        let id = bot.send_cmd(cmd, None, 0)?;
                        self.waiting = Some(Waiting {
                            id,
                            kind: Kind::Side,
                            since: now,
                            blocker: blocker_of(bot),
                            counts: true,
                        });
                    }
                    Act::Chat(chat) => bot.send_chat(chat)?,
                    Act::Burst(cmds) => {
                        // A spammer does not wait and does not mind the window.
                        bot.s.knobs.window = true;
                        for c in cmds {
                            bot.send_cmd(c, None, 0)?;
                        }
                    }
                }
                self.steps += 1;
                Ok(Progress::Acted)
            }
            Step::EndTurn { force } => {
                let id = bot.send_cmd(Command::EndTurn { force }, None, 0)?;
                self.waiting = Some(Waiting {
                    id,
                    kind: Kind::EndTurn { force },
                    since: now,
                    blocker: blocker_of(bot),
                    counts: true,
                });
                self.steps += 1;
                Ok(Progress::Acted)
            }
        }
    }

    /// For the result file.
    pub fn summary(&self) -> serde_json::Value {
        json!({ "persona": format!("{:?}", self.persona), "pace": format!("{:?}", self.pace), "explored": self.explored })
    }
}

enum Asked {
    Wait,
    Send(Query),
    Answer(QueryAnswer),
    Failed,
}
