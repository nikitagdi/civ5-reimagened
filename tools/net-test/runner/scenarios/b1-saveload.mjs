// `b1-saveload` (N6.2, AC-N6.2, net-test 4.5, NT-D14): the B1 `saveload` scenario through the server, on the real core.
// A human bot plays a Duel (one AI) for 30 turns with the shared policy; the game is saved and stopped, the gateway is
// restarted; a second nick with no seat makes a room from the save (the lobby's `LoadSave`), the human takes its
// reserved slot, the maker leaves (its slot goes back to the AI) and the human starts the room. The first view of the human equals the last one before the stop (hash equal);
// then it plays 20 more turns with every hash checked. At the end the newest save is damaged on disk (cut short): the
// lobby answers a `LoadSave` of it with a plain `LobbyError` (or the start of that room fails with a plain reason),
// and the lobby stays alive (the same bot still gets its lists, `/healthz` answers).
// Real core: `cargo xtask nettest --wsl --core real --scenario b1-saveload`.
import { readdirSync, statSync, truncateSync } from "node:fs";
import path from "node:path";
import { asBig, hex } from "../lib/json.mjs";
import { waitForSave } from "../lib/play.mjs";

export const meta = {
  id: "b1-saveload",
  ac: ["AC-N6.2"],
  needs: ["admin", "hooks"],
  core: ["real"],
  allowErrors: [/^rejected: /],
  quick: { turns: 6, after: 4 },
  full: { turns: 30, after: 20 },
};

/** The paths of all `*.c5sv` files below `dir`. */
function saveFiles(dir) {
  const out = [];
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      if (e.isDirectory()) walk(p);
      else if (e.name.endsWith(".c5sv")) out.push(p);
    }
  };
  try {
    walk(dir);
  } catch {
    // no saves yet
  }
  return out;
}

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns ?? 30;
  const after = opts.after ?? 20;
  const numbers = {};
  ctx.restarts = 1; // the one restart of the gateway below
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
  const sendAndCollect = async (h, cmd) => {
    const before = (await lobbyErrors(h)).length;
    await h.lobby(cmd);
    await sleep(700);
    return (await lobbyErrors(h)).slice(before);
  };
  const tokenOf = (nick) => path.join(ctx.logDir, "bots", nick, "token");

  // ---- 1. the game: `turns` turns, then save and stop
  const A = ctx.bots.spawn({ nick: "slA" });
  await A.connect();
  await A.enter({ role: "host", humans: 1, ai_slots: [1], settings: { world_size: "WORLDSIZE_DUEL", seed: 1, map_seed: 1 } }, { lobby_timeout_ms: 300000 });
  const sa = await A.state();
  const game = sa.attached.game;
  const playerA = sa.attached.player;
  const startTurn = sa.turn ?? 0;
  await A.autoplay({ turns, persona: "autoplay", pace: "burst" });
  await A.wait({ autoplay: 1 }, 120000 + turns * 30000);
  // Let the AI phase of the last turn end: the game stands at the start of a human turn.
  const turnAtStop = (await until(async () => {
    const s = await A.state();
    return s.turn >= startTurn + turns && s.phase === "Humans" ? s.turn : 0;
  }, 120000)) || (await A.state()).turn;
  const hashA = (await A.state()).view.hash;
  numbers.before_stop = { game, turn: turnAtStop, view_hash: hex(asBig(hashA)) };
  expect(`played ${turns} turns before the save`, turnAtStop >= startTurn + turns, `turn ${turnAtStop}`);

  const saved = await server.admin.save(game);
  expect("save: the admin API takes the save", saved.ok, `HTTP ${saved.status}`);
  expect(`save: the save of turn ${turnAtStop} is written`, !!(await waitForSave(ctx, game, turnAtStop, 300000)), "not within 5 min");
  const stopped = await server.admin.stop(game);
  expect("stop: the admin API stops the game", stopped.ok, `HTTP ${stopped.status}`);
  const status = await until(async () => {
    const st = ((await server.admin.games()).games ?? []).find((x) => x.id === game)?.status;
    return st && st !== "Running" ? st : null;
  }, 30000);
  expect("stop: the game is not running any more", !!status, `status ${status}`);
  const detail = await server.admin.get(`/admin/games/${game}`).catch(() => null);
  const latest = (detail?.saves ?? []).reduce((m, s) => (!m || s.seq > m.seq ? s : m), null);
  const seq = latest?.seq ?? null;
  expect("save: the newest save of the game is on disk", seq !== null, `seq ${seq}, turn ${latest?.turn}`);
  await A.stop();
  await server.restart();

  // ---- 2. a nick with no seat makes a room from the save in the lobby
  const saveId = `${game}-${String(seq).padStart(8, "0")}`;
  const C = ctx.bots.spawn({ nick: "slC" });
  await C.connect();
  await C.wait({ conn: "lobby" }, 30000);
  const made = await sendAndCollect(C, { LoadSave: { save: saveId } });
  const roomId = (await until(async () => (await C.state()).room, 15000)) ?? null;
  const inRoom = await until(async () => (await C.state()).in_room, 15000);
  expect("a bot with no seat makes a room from the save (LoadSave)", !!inRoom && made.length === 0, `errors ${JSON.stringify(made)}`);

  // ---- 3. the human takes its reserved slot, the room starts
  const A2 = ctx.bots.spawn({ nick: "slA2" });
  await A2.connect({ nick: "slA", token_file: tokenOf("slA") });
  await A2.wait({ conn: "lobby" }, 30000);
  const joinErr = await sendAndCollect(A2, { JoinRoom: { room: roomId } });
  const roomPath = joinErr.length === 0 && (await until(async () => (await A2.state()).in_room, 5000));
  if (!expect("the seated human joins the room made from its game's save (JoinRoom)", !!roomPath, JSON.stringify(joinErr))) return { numbers };
  const slotOf = (st, nick) => (st.room_slots ?? []).find((s) => s.occupant === `Human:${nick}`)?.index ?? null;
  await until(async () => slotOf(await A2.state(), "slA") !== null, 5000);
  if (slotOf(await A2.state(), "slA") !== playerA) await sendAndCollect(A2, { TakeSlot: { index: playerA } });
  expect("the human sits in its reserved slot", (await until(async () => slotOf(await A2.state(), "slA") === playerA, 5000)) === true, JSON.stringify((await A2.state()).room_slots));
  // C sits in the AI slot of the save (a room's maker has a slot): it leaves, the host's part goes to the human, who
  // gives the slot back to the AI. The players are the save's, and so is the view.
  const cSlot = slotOf(await C.state(), "slC");
  expect("C leaves the room", (await sendAndCollect(C, "LeaveRoom")).length === 0 && cSlot !== null, `C was in slot ${cSlot}`);
  expect("the human gives C's slot back to the AI", (await sendAndCollect(A2, { Slot: { index: cSlot, occupant: "Ai", civ: "Random" } })).length === 0, "");
  expect("the human (the host now) starts the room made from the save", (await sendAndCollect(A2, "Start")).length === 0, "");
  await A2.wait({ conn: "attached" }, 120000);
  const first = (await A2.stats()).first_view;
  expect("the first view equals the last one before the stop", !!first && asBig(first.view_hash) === asBig(hashA), `before ${hex(asBig(hashA))}, after ${first ? hex(asBig(first.view_hash)) : "none"}`);
  expect("the first view is the turn of the save", !!first && first.turn === turnAtStop, `view turn ${first?.turn}, save turn ${turnAtStop}`);

  // ---- 4. `after` more turns, every hash checked
  await A2.autoplay({ turns: after, persona: "autoplay", pace: "burst" });
  await A2.wait({ autoplay: 1 }, 120000 + after * 30000);
  const st2 = await A2.stats();
  expect(`the loaded game plays ${after} turns`, st2.turns_played === after, `turns_played ${st2.turns_played}`);
  expect("every hash of the loaded game agrees (checked, none differed)", st2.hash.checked > 0 && st2.hash.mismatch === 0, `checked ${st2.hash.checked}, mismatch ${st2.hash.mismatch}`);
  expect("the loaded game found no fog violation", st2.fog.violations === 0, `${st2.fog.violations}`);
  const gameNow = (await A2.state()).attached?.game;
  let g = null;
  for (let i = 0; i < 50; i++) {
    g = gameNow === undefined ? null : await server.admin.game(gameNow);
    if (g && String(asBig(g.seats?.[String(playerA)]?.view_hash)) === String(asBig(st2.final.view_hash))) break;
    await sleep(100);
  }
  expect("the gateway's hash of the seat equals the bot's", !!g && String(asBig(g.seats?.[String(playerA)]?.view_hash)) === String(asBig(st2.final.view_hash)), `bot ${hex(asBig(st2.final.view_hash))}`);
  expect("the loaded game runs", g?.status === "Running" && g.turn >= turnAtStop + after, `${g?.status}, turn ${g?.turn}`);
  expect("the worker did not crash", g?.worker?.crashes === 0, `${g?.worker?.crashes}`);
  numbers.after = { game: gameNow, turn: g?.turn };

  // ---- 5. a damaged save: a plain error, the lobby lives
  await server.admin.save(gameNow);
  await waitForSave(ctx, gameNow, g?.turn ?? 0, 300000);
  await server.admin.stop(gameNow);
  await until(async () => {
    const s = ((await server.admin.games()).games ?? []).find((x) => x.id === gameNow)?.status;
    return s && s !== "Running" ? s : null;
  }, 30000);
  const detail2 = await server.admin.get(`/admin/games/${gameNow}`).catch(() => null);
  const latest2 = (detail2?.saves ?? []).reduce((m, s) => (!m || s.seq > m.seq ? s : m), null);
  await A2.stop();
  await C.stop();
  const files = saveFiles(server.stateDir).filter((f) => path.basename(f).startsWith(String(latest2?.seq ?? "").padStart(8, "0")));
  expect("the newest save file is found on disk", files.length > 0, `seq ${latest2?.seq}`);
  if (files.length === 0) return { numbers };
  const sizeBefore = statSync(files[0]).size;
  for (const f of files) truncateSync(f, 40);
  await server.restart();
  ctx.restarts = 2;
  const D = ctx.bots.spawn({ nick: "slD" });
  await D.connect();
  await D.wait({ conn: "lobby" }, 30000);
  const badId = `${gameNow}-${String(latest2.seq).padStart(8, "0")}`;
  const badErr = await sendAndCollect(D, { LoadSave: { save: badId } });
  const roomBad = await until(async () => (await D.state()).in_room, 3000);
  let reason = badErr[0] ?? null;
  if (!reason && roomBad) {
    // The room was made; the damage shows when the game is built from the file.
    const startErr = await sendAndCollect(D, "Start");
    reason = startErr[0] ?? null;
    if (!reason) {
      await sleep(3000);
      const gs = (await server.admin.games()).games ?? [];
      reason = gs.find((x) => x.id === gameNow && x.status !== "Running")?.status ? `the game did not run: ${JSON.stringify(gs.find((x) => x.id === gameNow))}` : null;
    }
  }
  numbers.damaged = { file: path.basename(files[0]), size_before: sizeBefore, reason, room_made: !!roomBad };
  expect("a damaged save gives a plain reason (LobbyError or a refused start)", typeof reason === "string" && reason.length > 0, JSON.stringify(badErr));
  const health = await fetch(`${server.url}/healthz`).then((r) => r.status).catch(() => 0);
  expect("the gateway is alive after the damaged save: /healthz answers", health === 200, `HTTP ${health}`);
  // The lobby is alive: the same connection still answers (a command gets an answer, the bot is in the lobby).
  const alive = await D.wait({ conn: "lobby" }, 10000).then(() => true, () => false);
  const again = await sendAndCollect(D, { LoadSave: { save: "no-such-save" } });
  expect("the lobby is alive: the bot is still in the lobby and a new command gets its answer", alive && again.length === 1, `alive ${alive}, answers ${JSON.stringify(again)}`);
  return { numbers };
}
