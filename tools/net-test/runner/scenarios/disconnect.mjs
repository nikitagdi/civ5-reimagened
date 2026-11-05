// `disconnect` (N4.6, net-test 4.5): a player's link drops in the middle of a turn. Four games, each with its own nicks.
//   K (kick path): B drops; A ends its turn and the turn waits for B; the seat's timer is stopped (left_ms_when_stopped
//      is set and does not move over 20 s); the turn does not change in 30 s. A (host) kicks B: the seat becomes
//      Ai { kicked: B } and the turn goes on. B comes back and takes its own seat again (JoinGame within 10 s).
//      Refusals first: B's Kick of the host is NotHost, the host's Kick of itself is SelfKick (gateway counters).
//   R (resume and Leave): B drops and comes back by its token before any kick (Replay or Fresh); its order is applied;
//      the turn goes on. Then B sends GameCtl::Leave: the seat stays B's human seat, the connection is Lost, the turn
//      waits and the timer stops. A second nick on that seat gets LobbyError. B joins its own seat again; the host
//      does not change.
//   H (host lost): 3 humans, timer off. The host A drops. The host limit is `host_lost_s` (quick: 3 s; full: the real 60 s). Within the limit A is still the host and Kick from B is
//      GameCtlRejected NotHost. After it the rights go to B (C sees the same host), gateway host.changes == 1 and
//      last_change_after_lost_ms is within [limit, limit + 3 s]. B's Pause is accepted; C's Pause and Kick are NotHost.
//      A comes back as a plain player: its Kick and Pause are NotHost, the host stays B.
//   H2 (chain): A and B drop together: after the limit the host is C. A comes back; C drops: the rights go to A (lowest
//      number online). A drops again with nobody else online: no change (host.changes stays); B comes back: the rights
//      move to B.
//   Journal and lobby: the kick is a `Session` line "<host> kicked <nick>" in the journal of the seats; each host change
//      is a `Session` line "<new host> is the host now: ..."; the lobby list (`games[].host`) names the new host.
//      Checked with polling after each change.
// Not run: the kick of the last human (LastHuman).
import { endTurn } from "../lib/play.mjs";

export const meta = {
  id: "disconnect",
  ac: ["AC-N4.6"],
  needs: ["admin"],
  core: ["fixture", "real"],
  // Quick shortens the host limit to 3 s; full keeps the real 60 s.
  quick: { serverConfig: { hostLostS: 3 }, limit_ms: 3000 },
  full: { serverConfig: { hostLostS: 60 }, limit_ms: 60000 },
};

let LIMIT_MS = 3000; // host_lost_s of the preset's serverConfig (set in run)
let LOBBY = null; // an observer bot in the lobby (not in a game): the lobby list is not kept by a seated bot

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const isLost = (c) => !!c && typeof c === "object" && "Lost" in c;
const seatOf = (st, p) => st.seats?.find((s) => s.player === p) ?? null;
const hostOf = (st) => st.seats?.find((s) => s.host)?.player ?? null;

/** Reads `read()` every `step` ms until `ok(value)` holds or `ms` is over; returns the last value. */
async function poll(read, ok, ms = 10000, step = 100) {
  const t0 = Date.now();
  let v = await read();
  while (!ok(v) && Date.now() - t0 < ms) {
    await sleep(step);
    v = await read();
  }
  return v;
}

/** The final outcome text of an order, from the bot's `result` events. */
const outcomeOf = (h, id) => [...h.events].reverse().find((e) => e.ev === "result" && e.id === id && e.outcome !== "Queued")?.outcome ?? null;

const journalOf = async (h) => (await h.state()).journal ?? [];
/** A `Session` journal line that contains `sub`. */
const hasSession = (j, sub) => j.some((e) => e.kind === "Session" && e.text.includes(sub));
/** The host named by the lobby list for `game` (null when the bot shows no lobby list). */
const lobbyHostOf = (st, game) => st.games?.find((g) => g.id === game)?.host ?? null;

/** Connects the nicks in order, the first one hosts a room of `names.length` humans on Duel; returns the game. */
async function start(ctx, names, settings) {
  const H = names.map((nick) => ctx.bots.spawn({ nick }));
  for (const h of H) await h.connect();
  await Promise.all([
    H[0].enter({ role: "host", humans: H.length, ai_slots: [], settings }, { lobby_timeout_ms: 120000 }),
    ...H.slice(1).map((h) => h.enter({ role: "guest", host: names[0], slots: H.length }, { lobby_timeout_ms: 120000 })),
  ]);
  const game = (await H[0].state()).attached.game;
  return { H, game };
}

async function kickGame(ctx, tag) {
  const { expect, server } = ctx;
  const names = [`dcKa${tag}`, `dcKb${tag}`];
  const {
    H: [A, B],
    game,
  } = await start(ctx, names, { turn_mode: "Simultaneous", timer_seconds: 60 });
  const pA = (await A.state()).attached.player;
  const pB = (await B.state()).attached.player;
  const turn0 = (await A.state()).turn;
  const rej = async () => (await server.admin.game(game)).gamectl_rejected;

  // Refusals: a guest kicks the host; the host kicks itself.
  const r0 = await rej();
  await B.gamectl({ Kick: { player: pA } });
  await A.gamectl({ Kick: { player: pA } });
  await sleep(400);
  const r1 = await rej();
  expect("kick by a seat that is not the host is refused (NotHost)", r1.NotHost - r0.NotHost === 1, `${r0.NotHost} -> ${r1.NotHost}`);
  expect("kick of oneself by the host is refused (SelfKick)", r1.SelfKick - r0.SelfKick === 1, `${r0.SelfKick} -> ${r1.SelfKick}`);

  // (1) the drop shows at once: the seat stays human, its connection is Lost.
  await B.drop();
  const seatB = await poll(async () => seatOf(await A.state(), pB), (s) => isLost(s?.conn), 5000);
  expect("(1) B's seat stays human and its connection is Lost", !!seatB?.control?.Human && isLost(seatB.conn), JSON.stringify(seatB));

  // (2) A ends its turn: the turn does not change for 30 s. (3) The timer of the seat is stopped and does not move.
  await endTurn(A);
  const tEnd = Date.now();
  const leftOf = async () => (await A.state()).turn_state?.timers?.[0]?.left_ms_when_stopped ?? null;
  const left1 = await poll(leftOf, (v) => v !== null, 5000);
  await sleep(Math.max(0, 20000 - (Date.now() - tEnd)));
  const left2 = await leftOf();
  await sleep(Math.max(0, 30000 - (Date.now() - tEnd)));
  const turnNow = (await A.state()).turn;
  expect("(3) the timer is stopped: left_ms_when_stopped is set", left1 !== null, `${left1}`);
  expect("(3) the value did not go down over 20 s", left1 !== null && left2 === left1, `${left1} -> ${left2}`);
  expect("(2) the turn did not change in 30 s", turnNow === turn0, `turn ${turn0} -> ${turnNow}`);

  // (4) A kicks B: the seat is Ai { kicked: B }; the turn goes on.
  await A.gamectl({ Kick: { player: pB } });
  let kickedTurn = true;
  try {
    await A.wait({ turn: turn0 + 1 }, 20000);
  } catch {
    kickedTurn = false;
  }
  const seatK = seatOf(await A.state(), pB);
  expect("(4) after the kick the seat is Ai { kicked: B }", seatK?.control?.Ai?.kicked === names[1], JSON.stringify(seatK?.control));
  expect("(4) the turn went on after the kick", kickedTurn, `turn now ${(await A.state()).turn}`);

  // (5) B comes back and takes its own seat again: within 10 s.
  let backErr = null;
  try {
    await B.reconnect(true);
  } catch (e) {
    backErr = String(e.message);
  }
  const t5 = Date.now();
  let joinErr = null;
  try {
    await B.enter({ role: "join", game, player: pB }, { lobby_timeout_ms: 10000 });
  } catch (e) {
    joinErr = String(e.message);
  }
  const joinMs = Date.now() - t5;
  const backOnSeat = (await B.state()).attached?.player === pB;
  expect("(5) B takes its own seat again within 10 s", !joinErr && backOnSeat && joinMs <= 10000, `join ${joinMs} ms${joinErr ? `: ${joinErr}` : ""}${backErr ? `; reconnect: ${backErr}` : ""}`);

  // Journal: the kick is a Session line naming B in the journal of the host's seat.
  const jKick = await poll(() => journalOf(A), (j) => hasSession(j, `kicked ${names[1]}`), 5000);
  expect("(4) the kick is a Session line in the journal (naming B)", hasSession(jKick, `kicked ${names[1]}`), `${jKick.filter((e) => e.kind === "Session").map((e) => e.text).join(" | ")}`);
  return { game, left_ms_when_stopped: left1, join_ms: joinMs };
}

async function resumeGame(ctx, tag) {
  const { expect, server } = ctx;
  const names = [`dcRa${tag}`, `dcRb${tag}`];
  const {
    H: [A, B],
    game,
  } = await start(ctx, names, { turn_mode: "Simultaneous", timer_seconds: 60 });
  const pA = (await A.state()).attached.player;
  const pB = (await B.state()).attached.player;
  const gw = () => server.admin.game(game);
  const lost0 = (await gw()).cmds.lost;
  const turn0 = (await A.state()).turn;
  const unitB = (await B.state()).mine[0];

  // (6) B's link drops; A ends its turn; B comes back by its token before any kick.
  await B.drop();
  await poll(async () => seatOf(await A.state(), pB), (s) => isLost(s?.conn), 5000);
  await endTurn(A);
  await sleep(1500);
  await B.reconnect(true);
  await B.wait({ conn: "attached" }, 20000);
  const mode = (await B.state()).attached?.mode;
  expect("(6) B came back by its token (Replay or Fresh)", mode === "Replay" || mode === "Fresh", `${mode}`);
  const id = await B.cmd({ UnitSkip: { unit: unitB } });
  await B.wait({ result_id: id }, 15000);
  expect("(6) the order sent after the return is applied", outcomeOf(B, id) === "Applied", `${outcomeOf(B, id)}`);
  await endTurn(B);
  let advanced = true;
  try {
    await A.wait({ turn: turn0 + 1 }, 20000);
  } catch {
    advanced = false;
  }
  expect("(6) the turn went on once both had ended", advanced, `turn ${turn0} -> ${(await A.state()).turn}`);
  expect("(6) no order was lost", (await gw()).cmds.lost === lost0, `${lost0} -> ${(await gw()).cmds.lost}`);

  // (7) Leave: the seat stays B's, the connection is Lost, the turn waits, the timer stops.
  const t1 = (await A.state()).turn;
  await B.gamectl("Leave");
  await B.wait({ conn: "lobby" }, 15000);
  const seatL = await poll(async () => seatOf(await A.state(), pB), (s) => isLost(s?.conn), 5000);
  expect("(7) after Leave the seat stays B's human seat, connection Lost", seatL?.control?.Human?.nick === names[1] && isLost(seatL.conn), JSON.stringify(seatL));
  await endTurn(A);
  await sleep(3000);
  const stL = await A.state();
  expect("(7) the turn waits for B and the timer is stopped", stL.turn === t1 && stL.turn_state?.timers?.[0]?.left_ms_when_stopped != null, `turn ${t1} -> ${stL.turn}`);

  // A second nick on that seat: refused (LobbyError).
  const C = ctx.bots.spawn({ nick: `dcRc${tag}` });
  await C.connect();
  let refused = null;
  try {
    await C.enter({ role: "join", game, player: pB }, { lobby_timeout_ms: 8000 });
  } catch (e) {
    refused = String(e.message);
  }
  expect("(7) a second nick on B's seat is refused (LobbyError)", refused !== null, refused ?? "the second nick sat down");

  // B takes its seat back: the same seat, the host stays A.
  await B.enter({ role: "join", game, player: pB }, { lobby_timeout_ms: 20000 });
  const stB = await poll(async () => A.state(), (s) => seatOf(s, pB)?.conn === "Online", 8000);
  expect("(7) B sits on its own seat again", (await B.state()).attached?.player === pB, `${(await B.state()).attached?.player}`);
  expect("(7) the host did not change on Leave or JoinGame", hostOf(stB) === pA, `host ${hostOf(stB)}`);
  await endTurn(B);
  let after = true;
  try {
    await A.wait({ turn: t1 + 1 }, 20000);
  } catch {
    after = false;
  }
  expect("(7) the turn went on after B ended", after, `turn ${t1} -> ${(await A.state()).turn}`);
  return { game };
}

async function hostLostGame(ctx, tag) {
  const { expect, server } = ctx;
  const names = [`dcHa${tag}`, `dcHb${tag}`, `dcHc${tag}`];
  const {
    H: [A, B, C],
    game,
  } = await start(ctx, names, { turn_mode: "Simultaneous" });
  const pA = (await A.state()).attached.player;
  const pB = (await B.state()).attached.player;
  const pC = (await C.state()).attached.player;
  const gw = () => server.admin.game(game);
  expect("start: A is the host", hostOf(await B.state()) === pA, `host ${hostOf(await B.state())}`);

  await A.drop();
  // (a) within the limit: A is still the host; Kick from B is NotHost.
  const nh0 = (await gw()).gamectl_rejected.NotHost;
  await B.gamectl({ Kick: { player: pC } });
  await sleep(LIMIT_MS / 2);
  expect("(a) within the limit the host is still A", hostOf(await B.state()) === pA, `host ${hostOf(await B.state())}`);
  expect("(a) Kick from B is refused: NotHost", (await gw()).gamectl_rejected.NotHost - nh0 === 1, `${nh0} -> ${(await gw()).gamectl_rejected.NotHost}`);

  // The successor: the next human in seat order after the old host's seat, on the line (the circle of seats).
  const byPlayer = { [pA]: A, [pB]: B, [pC]: C };
  const nextAfter = (p, set) => [...set].sort((x, y) => x - y).find((x) => x > p) ?? Math.min(...set);
  const newHost = nextAfter(pA, [pB, pC]);
  const other = newHost === pB ? pC : pB;
  const HOST = byPlayer[newHost];
  const OTHER = byPlayer[other];
  expect("the successor is the next human in seat order (one of B, C)", newHost === pB || newHost === pC, `${newHost}`);

  // (b) after the limit the rights go to the successor; the other sees the same; one change; after the limit.
  const bHost = await poll(async () => hostOf(await HOST.state()), (v) => v === newHost, LIMIT_MS + 6000);
  expect(`(b) after the limit the host is the successor (player ${newHost})`, bHost === newHost, `host ${bHost}`);
  const cHost = await poll(async () => hostOf(await OTHER.state()), (v) => v === newHost, 3000);
  expect("(b) the other human sees the same host", cHost === newHost, `host ${cHost}`);
  const g1 = await gw();
  expect("(b) gateway host.changes == 1", g1.host.changes === 1, `${g1.host.changes}`);
  const lm = g1.host.last_change_after_lost_ms;
  expect(`(b) the change came after the limit (within [${LIMIT_MS}, ${LIMIT_MS + 3000}] ms)`, lm >= LIMIT_MS && lm <= LIMIT_MS + 3000, `${lm} ms`);
  const nickOf = { [pA]: names[0], [pB]: names[1], [pC]: names[2] };
  const jHost = await poll(() => journalOf(OTHER), (j) => hasSession(j, `${nickOf[newHost]} is the host now`), 5000);
  expect("(b) the journal names the new host (Session line)", hasSession(jHost, `${nickOf[newHost]} is the host now`), `${jHost.filter((e) => e.kind === "Session").map((e) => e.text).join(" | ")}`);
  const gHost = await poll(async () => lobbyHostOf(await LOBBY.state(), game),(v) => v === nickOf[newHost], 5000);
  expect("(b) the lobby list names the new host (games[].host)", gHost === nickOf[newHost], `games host ${gHost}, want ${nickOf[newHost]}`);

  // (c) the new host pauses; the other human can neither pause nor kick.
  await HOST.gamectl({ Pause: true });
  const paused = await poll(async () => (await OTHER.state()).turn_state?.paused, (v) => v === true, 3000);
  expect("(c) the new host pauses the game: the other sees paused", paused === true, `${paused}`);
  const nh1 = (await gw()).gamectl_rejected.NotHost;
  await OTHER.gamectl({ Pause: false });
  await OTHER.gamectl({ Kick: { player: pA } });
  await sleep(300);
  expect("(c) Pause and Kick from the other human are refused: NotHost", (await gw()).gamectl_rejected.NotHost - nh1 === 2, `${nh1} -> ${(await gw()).gamectl_rejected.NotHost}`);
  await HOST.gamectl({ Pause: false });

  // (d) A comes back as a plain player; its Kick and Pause are refused; the host stays the successor.
  await A.reconnect(true);
  await A.wait({ conn: "attached" }, 20000);
  const modeA = (await A.state()).attached?.mode;
  expect("(d) A came back by its token (Replay or Fresh)", modeA === "Replay" || modeA === "Fresh", `${modeA}`);
  const hostAfterBack = await poll(async () => hostOf(await A.state()), (v) => v === newHost, 5000);
  expect("(d) the host stays the successor after A came back", hostAfterBack === newHost, `host ${hostAfterBack}`);
  const nh2 = (await gw()).gamectl_rejected.NotHost;
  await A.gamectl({ Pause: true });
  await A.gamectl({ Kick: { player: pC } });
  await sleep(300);
  expect("(d) A's Pause and Kick are refused: NotHost", (await gw()).gamectl_rejected.NotHost - nh2 === 2, `${nh2} -> ${(await gw()).gamectl_rejected.NotHost}`);
  expect("(d) host.changes did not grow (still 1)", (await gw()).host.changes === 1, `${(await gw()).host.changes}`);
  return { game, host_change_after_lost_ms: lm };
}

async function hostChainGame(ctx, tag) {
  const { expect, server } = ctx;
  const names = [`dcCa${tag}`, `dcCb${tag}`, `dcCc${tag}`];
  const {
    H: [A, B, C],
    game,
  } = await start(ctx, names, { turn_mode: "Simultaneous" });
  const pA = (await A.state()).attached.player;
  const pB = (await B.state()).attached.player;
  const pC = (await C.state()).attached.player;
  const gw = () => server.admin.game(game);

  // (d) A and B drop together: after the limit the host is C.
  await A.drop();
  await B.drop();
  const cHost = await poll(async () => hostOf(await C.state()), (v) => v === pC, LIMIT_MS + 8000);
  expect("(d) A and B dropped together: after the limit the host is C", cHost === pC, `host ${cHost}`);
  const gC = await poll(async () => lobbyHostOf(await LOBBY.state(), game),(v) => v === names[2], 5000);
  expect("(d) the lobby list names C as the host", gC === names[2], `games host ${gC}`);

  // (e) A comes back; the host C drops: the rights go to A (the lowest number online).
  await A.reconnect(true);
  await A.wait({ conn: "attached" }, 20000);
  // The seats of the Replay come a few ms after the attach; until then A's view still says that A is the host and the
  // poll below would pass before the host has changed at all.
  await poll(async () => hostOf(await A.state()), (v) => v === pC, 5000);
  await C.drop();
  const aHost = await poll(async () => hostOf(await A.state()), (v) => v === pA, LIMIT_MS + 8000);
  expect("(e) the host C dropped: the rights go to A", aHost === pA, `host ${aHost}`);
  expect("(e) gateway host.changes == 2 (A to C, C to A)", (await gw()).host.changes === 2, `${(await gw()).host.changes}`);
  const gA = await poll(async () => lobbyHostOf(await LOBBY.state(), game),(v) => v === names[0], 5000);
  expect("(e) the lobby list names A as the host", gA === names[0], `games host ${gA}`);

  // (f) only the host is on the line: no change; B comes back: the rights move to B.
  await A.drop();
  await sleep(LIMIT_MS + 1500);
  expect("(f) nobody else on the line: host.changes stays 2", (await gw()).host.changes === 2, `${(await gw()).host.changes}`);
  await B.reconnect(true);
  const bHost = await poll(async () => hostOf(await B.state()), (v) => v === pB, LIMIT_MS + 8000);
  expect("(f) when B comes back the rights move to B", bHost === pB, `host ${bHost}`);
  const gB = await poll(async () => lobbyHostOf(await LOBBY.state(), game),(v) => v === names[1], 5000);
  expect("(f) the lobby list names B as the host", gB === names[1], `games host ${gB}`);
  expect("(f) gateway host.changes == 3", (await gw()).host.changes === 3, `${(await gw()).host.changes}`);
  return { game };
}

export async function run(ctx) {
  LIMIT_MS = ctx.opts.limit_ms ?? LIMIT_MS;
  const tag = String(Date.now() % 100000);
  const numbers = {};
  LOBBY = ctx.bots.spawn({ nick: `dcLobby${tag}` });
  await LOBBY.connect();
  numbers.kick = await kickGame(ctx, tag);
  numbers.resume = await resumeGame(ctx, tag);
  numbers.host = await hostLostGame(ctx, tag);
  numbers.chain = await hostChainGame(ctx, tag);
  return { numbers };
}
