// `make-late-save` (N5.4, net-test 4.5 and section 12 item 6): makes the save of a late Huge game for `ai-time`.
// Two human bots (the shared policy) and ten AI on Huge play `turns` turns (150 in the full preset); then the game is
// saved and stopped, and the state dir with the tokens of the bots goes to `assets-out/saves/late-huge/` (outside git,
// see lib/latesave.mjs). The save is good for the build of the game core it was made on; make it again when the core
// build changes. Real core only: `cargo xtask nettest --wsl --core real --scenario make-late-save --full`.
import path from "node:path";
import { botChecks, finishWatched, gatewayChecks, gatewayGame, nicksOf, startGame, waitForSave } from "../lib/play.mjs";
import { lateSaveDir, storeLateSave } from "../lib/latesave.mjs";

export const meta = {
  id: "make-late-save",
  ac: [],
  must: false,
  needs: ["admin"],
  core: ["real"],
  quick: { turns: 20 },
  full: { turns: 150 },
};

// NETTEST_LATE_HUMANS=1: one bot only. A save with two bots cannot be loaded with a seat in neither of their slots:
// the core refuses the second `Kick` (a human is in auto moves), and a save of a game where a bot died cannot be
// loaded at all (kicking a dead human).
const HUMANS = Number(process.env.NETTEST_LATE_HUMANS ?? 2);
const PLAYERS = 12;
// Some seeds of a Huge map of 12 players keep the map script searching for starts past the gateway's 30 s wait for the worker.
const GAME_SEED = Number(process.env.NETTEST_GAME_SEED ?? 11);
// A late game that is also developed (the "Late Huge demo" save): NETTEST_LATE_SPEED=GAMESPEED_QUICK,
// NETTEST_LATE_HANDICAP=HANDICAP_KING, NETTEST_LATE_HUMAN_SLOTS=10,11 (or one slot with NETTEST_LATE_HUMANS=1: the host moves there) put the bots in the last slots, so the
// first seats are AI civilizations that played every turn.
const LATE_SPEED = process.env.NETTEST_LATE_SPEED;
const LATE_HANDICAP = process.env.NETTEST_LATE_HANDICAP;
const HUMAN_SLOTS = (process.env.NETTEST_LATE_HUMAN_SLOTS ?? "").split(",").filter(Boolean).map(Number);

export async function run(ctx) {
  const { expect, server, opts } = ctx;
  const turns = opts.turns ?? 150;
  const names = nicksOf("late", HUMANS);
  const numbers = {};

  const running = startGame(ctx, names, {
    turns,
    humans: HUMANS,
    host_slot: HUMANS === 1 && HUMAN_SLOTS.length === 1 ? HUMAN_SLOTS[0] : undefined,
    ai_slots:
      HUMAN_SLOTS.length === HUMANS
        ? Array.from({ length: PLAYERS }, (_, i) => i).filter((i) => !HUMAN_SLOTS.includes(i))
        : Array.from({ length: PLAYERS - HUMANS }, (_, i) => HUMANS + i),
    settings: {
      world_size: "WORLDSIZE_HUGE",
      seed: GAME_SEED,
      map_seed: GAME_SEED,
      ...(process.env.NETTEST_LATE_MINOR_CIVS ? { minor_civs: Number(process.env.NETTEST_LATE_MINOR_CIVS) } : {}),
      ...(LATE_SPEED ? { speed: LATE_SPEED } : {}),
      ...(LATE_HANDICAP ? { handicap: LATE_HANDICAP } : {}),
    },
    persona: "autoplay",
    pace: "burst",
    lobby_timeout_ms: 300000,
    turn_timeout_ms: Number(process.env.NETTEST_LATE_TURN_TIMEOUT_MS ?? 600000),
  });
  const budget = 300000 + turns * 60000;
  const { finished, fault } = await finishWatched(ctx, running, budget);
  if (!expect("both bots finished in time", finished !== null, fault ?? `${budget / 1000} s budget`)) return { numbers };
  const results = botChecks(expect, names, finished, turns);
  // A game that did not get to the turn is no late game: nothing is stored.
  if (finished.some((f) => f.code !== 0)) return { numbers };
  const game = results[0].play?.game;
  const g = await gatewayGame(ctx, game, results);
  if (!expect("the gateway knows the game", !!g, `game ${game}`)) return { numbers };
  gatewayChecks(expect, g, results, names);
  const startTurn = results[0].first_view?.turn ?? 0;
  expect(`the game is at turn ${startTurn + turns}`, g.status === "Running" && g.turn >= startTurn + turns, `${g.status}, turn ${g.turn}`);

  const metrics = await server.admin.metrics();
  const coreBuild = metrics.core_build ?? null;
  const saved = await server.admin.save(game);
  expect("the admin API takes the save", saved.ok, `HTTP ${saved.status}`);
  // The saves of the turns before it are written first; the stop comes when the one of this turn is on disk.
  const written = await waitForSave(ctx, game, g.turn, 600000);
  expect(`the save of turn ${g.turn} is written`, !!written, written ? `seq ${written.seq}` : "not within 10 min");
  const stopped = await server.admin.stop(game);
  expect("the admin API stops the game", stopped.ok, `HTTP ${stopped.status}`);
  let onDisk = null;
  for (let i = 0; i < 150 && !onDisk; i++) {
    const detail = await server.admin.get(`/admin/games/${game}`).catch(() => null);
    const stopsDone = ((await server.admin.games()).games ?? []).find((x) => x.id === game)?.status;
    const latest = (detail?.saves ?? []).reduce((m, s) => (!m || s.seq > m.seq ? s : m), null);
    if (latest && latest.turn >= g.turn && stopsDone && stopsDone !== "Running") onDisk = latest;
    if (!onDisk) await ctx.sleep(200);
  }
  expect(`a save of turn ${g.turn} is on disk and the game has stopped`, !!onDisk, onDisk ? JSON.stringify(onDisk) : "not after 30 s");
  if (!onDisk) return { numbers };

  const dir = lateSaveDir(ctx.root);
  storeLateSave({
    dir,
    stateDir: server.stateDir,
    botsDir: path.join(ctx.logDir, "bots"),
    nicks: names,
    meta: { core_build: coreBuild, game, turn: g.turn, turns_played: turns, players: PLAYERS, humans: HUMANS, seats: results.map((r) => ({ nick: r.nick, player: r.player })) },
  });
  expect("the save is stored outside git", true, dir);
  numbers.stored = { dir, core_build: coreBuild, game, turn: g.turn, ai_phase_ms: g.hist?.ai_phase_ms ?? null };
  return { numbers };
}
