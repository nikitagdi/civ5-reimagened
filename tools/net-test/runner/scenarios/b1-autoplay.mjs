// `b1-autoplay` (N6.2, AC-N6.2, net-test 4.5, NT-D14): the B1 `autoplay` matrix through the server. Seeds {1, 2, 3} times AI
// {1, 2}, `turns` turns each (100 in the full preset), played by one human bot with the shared policy (`autoplay`).
// One AI is a Duel, two AI need a bigger map (Tiny: the real core lets in as many players as the map size's default,
// so Duel holds two). Per cell: the bot plays every turn (a turn that does not come is a stuck turn: the bot times
// out), no error of the bot, no hash mismatch, no fog violation, no Lua error in the game's journal; the time the
// AI phase takes (`hist.ai_phase_ms`) is printed. Real core: `cargo xtask nettest --wsl --core real --scenario b1-autoplay`.
import { botChecks, finishWatched, gatewayChecks, gatewayGame, startGame } from "../lib/play.mjs";

export const meta = {
  id: "b1-autoplay",
  ac: ["AC-N6.2"],
  needs: ["admin"],
  core: ["real"],
  quick: { seeds: [1], ais: [1, 2], turns: 10 },
  full: { seeds: [1, 2, 3], ais: [1, 2], turns: 100 },
};

const r3 = (x) => Math.round(x * 1000) / 1000;

export async function run(ctx) {
  const { expect, server, opts } = ctx;
  const seeds = opts.seeds ?? [1, 2, 3];
  const ais = opts.ais ?? [1, 2];
  const turns = opts.turns ?? 100;
  const numbers = { cells: [] };

  for (const seed of seeds) {
    for (const ai of ais) {
      const label = `seed ${seed}, AI ${ai}`;
      const nick = `b1as${seed}a${ai}`;
      const size = ai <= 1 ? "WORLDSIZE_DUEL" : "WORLDSIZE_TINY";
      const budget = 120000 + turns * 30000;
      const running = startGame(ctx, [nick], {
        turns,
        humans: 1,
        ai_slots: Array.from({ length: ai }, (_, i) => 1 + i),
        settings: { world_size: size, seed, map_seed: seed },
        persona: "autoplay",
        pace: "burst",
        lobby_timeout_ms: 300000,
        turn_timeout_ms: 300000,
      });
      const { finished, fault } = await finishWatched(ctx, running, budget);
      if (!expect(`${label}: the bot finished in time (no stuck turn)`, finished !== null, fault ?? `${budget / 1000} s budget`)) {
        numbers.cells.push({ seed, ai, ok: false });
        continue;
      }
      const results = botChecks(expect, [nick], finished, turns, { tag: label });
      const r = results[0];
      const game = r.play?.game;
      const g = await gatewayGame(ctx, game, results);
      if (!expect(`${label}: the gateway knows the game`, !!g, `game ${game}`)) continue;
      gatewayChecks(expect, g, results, [nick]);
      const startTurn = r.first_view?.turn ?? 0;
      const gn = g;
      expect(`${label}: the game is at turn ${startTurn + turns}`, gn.status === "Running" && gn.turn >= startTurn + turns, `${gn.status}, turn ${gn.turn}`);
      const lua = r.journal?.lua_errors ?? null;
      expect(`${label}: no Lua error in the game's journal`, lua === 0, lua === null ? "the bot reports no journal" : `${lua} ${(r.journal.lua_error_examples ?? []).slice(0, 2).join(" | ")}`);
      const a = gn.hist?.ai_phase_ms ?? {};
      numbers.cells.push({ seed, ai, game, size, journal_lines: r.journal?.lines ?? null, ai_phase_ms: { n: a.n, p50: r3(a.p50 ?? 0), p95: r3(a.p95 ?? 0), max: r3(a.max ?? 0) }, step_ms_p95: gn.hist?.step_ms?.p95 ?? null });
      // The game is done: stop it, so that its worker does not stay for the cells that follow.
      await server.admin.stop(game);
    }
  }
  return { numbers };
}
