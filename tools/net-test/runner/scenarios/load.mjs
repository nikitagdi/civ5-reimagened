// `load` (N4.4, net-test 4.5): a game in progress is saved and stopped, the gateway is restarted, and the game comes
// back from the save with its humans on their own seats. Duel, two human bots (A host, B guest), two AIs.
//   1. A and B play `turns` turns (autoplay); their view hashes are the last known ones; the game is saved, stopped
//      (admin API), the gateway is restarted: no game runs;
//   2. a bot with no seat (C) makes a room from the save (`LoadSave`, id `<game>-<seq:08>`): the room appears;
//   3. a seated nick (A, back with its token) cannot make a room from the save: LobbyError "you have a seat in a game";
//   4. A and B (seated in the sleeping game; their logins do not wake it, the room keeps their slots) join the room,
//      take their reserved slots; C (in the save's AI slot) leaves, A, the host now, gives that slot back to the AI and starts;
//      the first view of each seat equals its last known hash.
// Pending: the reserved slots themselves (`reserved_for` is in RoomState, which the bot does not report) and the
// "Starting" room status (only `Running` is seen, through the admin API).
import path from "node:path";
import { asBig, hex } from "../lib/json.mjs";

export const meta = {
  id: "load",
  ac: ["AC-N4.4"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  allowErrors: [/^rejected: /],
  quick: { turns: 4 },
  full: { turns: 12 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns ?? 4;
  const numbers = {};
  ctx.restarts = 1; // one gateway restart below: the common check knows it
  const tokenOf = (nick) => path.join(ctx.logDir, "bots", nick, "token");
  const sleep = ctx.sleep;
  const until = async (pred, ms) => {
    const t0 = Date.now();
    for (;;) {
      const v = await pred();
      if (v) return v;
      if (Date.now() - t0 > ms) return v;
      await sleep(100);
    }
  };
  const lobbyErrors = async (h) => (await h.stats()).lobby_errors;
  /** The `LobbyError` texts that came after `cmd`. */
  const sendAndCollect = async (h, cmd) => {
    const before = (await lobbyErrors(h)).length;
    await h.lobby(cmd);
    await sleep(700);
    return (await lobbyErrors(h)).slice(before);
  };
  /** The room slot `nick` sits in, from a bot state. */
  const slotOf = (st, nick) => (st.room_slots ?? []).find((s) => s.occupant === `Human:${nick}`)?.index ?? null;
  /** `nick` (bot `h`, in the room) ends in slot `index`: the join may have put it there already. */
  const takeOwnSlot = async (h, nick, index) => {
    await until(async () => slotOf(await h.state(), nick) !== null, 5000);
    if (slotOf(await h.state(), nick) !== index && (await sendAndCollect(h, { TakeSlot: { index } })).length > 0) return false;
    return !!(await until(async () => slotOf(await h.state(), nick) === index, 5000));
  };

  // ---- 1. the game: two humans play `turns` turns, then save and stop
  const A = ctx.bots.spawn({ nick: "loadA" });
  const B = ctx.bots.spawn({ nick: "loadB" });
  await A.connect();
  await B.connect();
  // The smallest map of the server's list that holds two humans and two AIs (Duel on the test world, the real
  // core's Duel holds two players).
  await A.wait({ conn: "lobby" }, 20000);
  const sizes = (await A.state()).world_sizes ?? [];
  const size = sizes.find((s) => s.max_players >= 4);
  if (!expect("the server lists a map for four players", !!size, JSON.stringify(sizes))) return { numbers };
  numbers.world_size = size.id;
  await Promise.all([
    A.enter({ role: "host", humans: 2, ai_slots: [2, 3], settings: { world_size: size.id } }, { lobby_timeout_ms: 120000 }),
    B.enter({ role: "guest", host: "loadA", slots: 4 }, { lobby_timeout_ms: 120000 }),
  ]);
  const sa = await A.state();
  const sb = await B.state();
  const game = sa.attached.game;
  const playerA = sa.attached.player;
  const playerB = sb.attached.player;
  await A.autoplay({ turns, persona: "autoplay", pace: "burst" });
  await B.autoplay({ turns, persona: "autoplay", pace: "burst" });
  await A.wait({ autoplay: 1 }, 180000);
  await B.wait({ autoplay: 1 }, 180000);
  // Let the AI phase after the last end of turn run out (the real core plays it after the bots stop): the game stands
  // at the start of a human turn, and the views are the ones the save will hold.
  const startTurn = sa.turn ?? 0;
  await until(async () => {
    const [x, y] = [await A.state(), await B.state()];
    return x.phase === "Humans" && y.phase === "Humans" && x.turn >= startTurn + turns && x.turn === y.turn;
  }, 120000);
  await sleep(500);
  const hashA = (await A.state()).view.hash;
  const hashB = (await B.state()).view.hash;
  const turnAtStop = (await A.state()).turn;
  numbers.before_stop = { game, turn: turnAtStop, view_hash_A: hex(asBig(hashA)), view_hash_B: hex(asBig(hashB)) };

  const saved = await server.admin.save(game);
  expect("save: the admin API takes the save", saved.ok, `HTTP ${saved.status}`);
  const stopped = await server.admin.stop(game);
  expect("stop: the admin API stops the game", stopped.ok, `HTTP ${stopped.status}`);
  const status = await until(async () => {
    const st = ((await server.admin.games()).games ?? []).find((x) => x.id === game)?.status;
    return st && st !== "Running" ? st : null;
  }, 15000);
  expect("stop: the game is not running any more", !!status, `status ${status}`);
  // The newest save after the stop: the lobby's index keeps only the newest ring save, so an older seq is not loadable.
  const detail = await server.admin.get(`/admin/games/${game}`).catch(() => null);
  const latest = (detail?.saves ?? []).reduce((m, s) => (!m || s.seq > m.seq ? s : m), null);
  const seq = latest?.seq ?? null;
  expect("save: the newest save of the game is on disk", seq !== null, `seq ${seq}, turn ${latest?.turn}`);
  await A.stop();
  await B.stop();
  await server.restart();
  const after = await server.admin.metrics();
  expect("restart: no game runs in the new gateway process", after.server.games.running === 0, `${after.server.games.running}`);

  // ---- 2. a bot with no seat makes a room from the save
  const saveId = `${game}-${String(seq).padStart(8, "0")}`;
  const C = ctx.bots.spawn({ nick: "loadC" });
  await C.connect();
  await C.wait({ conn: "lobby" }, 20000);
  const made = await sendAndCollect(C, { LoadSave: { save: saveId } });
  const roomId = (await until(async () => (await C.state()).room, 15000)) ?? null;
  const inRoom = await until(async () => (await C.state()).in_room, 10000);
  expect("a bot with no seat makes a room from the save (LoadSave)", !!inRoom && made.length === 0, `errors ${JSON.stringify(made)}`);

  // ---- 3. a seated nick cannot make a room from the save
  const A2 = ctx.bots.spawn({ nick: "loadA2" });
  await A2.connect({ nick: "loadA", token_file: tokenOf("loadA") });
  await A2.wait({ conn: "lobby" }, 20000);
  const seatErr = await sendAndCollect(A2, { LoadSave: { save: saveId } });
  expect("a seated nick cannot make a room from the save: LobbyError (you have a seat in a game)", seatErr.length === 1 && /seat/.test(seatErr[0]), JSON.stringify(seatErr));

  // ---- 4. the humans take their reserved slots: JoinRoom by a seated nick
  const B2 = ctx.bots.spawn({ nick: "loadB2" });
  await B2.connect({ nick: "loadB", token_file: tokenOf("loadB") });
  await B2.wait({ conn: "lobby" }, 20000);
  // The room keeps their slots, so their logins did not wake the sleeping game.
  const asleep = ((await server.admin.games()).games ?? []).find((x) => x.id === game)?.status;
  expect("the logins of the seated nicks leave their game asleep (the room keeps their slots)", asleep !== "Running" && asleep !== "Starting", `status ${asleep}`);
  const joinErr = await sendAndCollect(A2, { JoinRoom: { room: roomId } });
  const roomPath = joinErr.length === 0 && (await until(async () => (await A2.state()).in_room, 5000));
  numbers.join_room = joinErr;
  if (!expect("a seated nick joins the room made from its game's save (JoinRoom)", !!roomPath, JSON.stringify(joinErr))) return { numbers };

  // Take the reserved slots. C sits in an AI slot of the save (a room's maker has a slot); it leaves the room, the
  // host's part goes to A, and A gives the slot back to the AI: the players are the save's, and so are the views.
  expect("A sits in its reserved slot", await takeOwnSlot(A2, "loadA", playerA), JSON.stringify((await A2.state()).room_slots));
  expect("B joins the room", (await sendAndCollect(B2, { JoinRoom: { room: roomId } })).length === 0, "");
  expect("B sits in its reserved slot", await takeOwnSlot(B2, "loadB", playerB), JSON.stringify((await B2.state()).room_slots));
  const cSlot = slotOf(await C.state(), "loadC");
  expect("C leaves the room", (await sendAndCollect(C, "LeaveRoom")).length === 0 && cSlot !== null, `C was in slot ${cSlot}`);
  expect("A gives C's slot back to the AI", (await sendAndCollect(A2, { Slot: { index: cSlot, occupant: "Ai", civ: "Random" } })).length === 0, "");
  await B2.lobby({ Ready: true });
  await sleep(500);
  expect("A (the host now) starts the room made from the save", (await sendAndCollect(A2, "Start")).length === 0, "");
  await A2.wait({ conn: "attached" }, 60000);
  await B2.wait({ conn: "attached" }, 60000);

  // ---- 5. the first view of each seat is the last known one
  const firstA = (await A2.stats()).first_view;
  const firstB = (await B2.stats()).first_view;
  expect("A's first view equals its last known hash", !!firstA && asBig(firstA.view_hash) === asBig(hashA), `before ${hex(asBig(hashA))}, after ${firstA ? hex(asBig(firstA.view_hash)) : "none"}`);
  expect("B's first view equals its last known hash", !!firstB && asBig(firstB.view_hash) === asBig(hashB), `before ${hex(asBig(hashB))}, after ${firstB ? hex(asBig(firstB.view_hash)) : "none"}`);
  const gameNow = (await A2.state()).attached?.game;
  const g = gameNow !== undefined ? await server.admin.game(gameNow) : null;
  expect("the game runs again", g?.status === "Running", `${g?.status} (turn ${g?.turn})`);
  expect("the game from the save is a new game", gameNow !== undefined && gameNow !== game, `${game} -> ${gameNow}`);
  numbers.after = { game: gameNow, turn: g?.turn };
  return { numbers };
}
