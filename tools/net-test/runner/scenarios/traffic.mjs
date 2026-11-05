// `traffic` (N3.5, net-test 4.5, decision NT-D11): how many bytes a player gets and sends per turn, and how big the
// first view (the keyframe), the map basis and the cold batch are. Two games one after the other, `human` pace (the
// bot pauses like a person, so these are not burst numbers): Duel with 2 human bots and 1 AI, Huge with 3 human bots
// and 1 AI (at most 10 turns). For bot 1 of each game the mean per turn is taken over the turns from the second on:
// the first turn is the cold entry (keyframe, basis, cold batch), reported apart. The keyframe threshold is checked on the gateway's
// wire size (compressed, decision 2026-10-22); the bot's raw size is printed next to it. The thresholds are provisional (BUILD N3.5): a failing check
// is a measured number over the threshold, and is reported as it is. Nothing here is loosened.
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";

export const meta = {
  id: "traffic",
  ac: ["AC-N3.5"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { turns: 6 },
  full: { turns: 30 },
};

const KB = 1024;
const r1 = (x) => (Number.isFinite(x) ? Math.round(x * 10) / 10 : null);
const mean = (xs) => (xs.length ? xs.reduce((s, x) => s + x, 0) / xs.length : NaN);

async function game(ctx, o) {
  const { expect } = ctx;
  const names = nicksOf(o.prefix, o.humans);
  const budget = budgetFor(o.turns, 15000);
  const running = startGame(ctx, names, {
    turns: o.turns,
    humans: o.humans,
    ai_slots: o.aiSlots,
    settings: { world_size: o.size },
    pace: "human",
  });
  const finished = await finishAll(running, budget);
  if (!expect(`${o.label}: all bots finished in time`, finished !== null, `${budget / 1000} s budget`)) return null;
  const results = botChecks(expect, names, finished, o.turns, { tag: o.label });

  const r = results[0];
  const gameId = r.play?.game;
  const g = await gatewayGame(ctx, gameId, results);
  if (!expect(`${o.label}: the gateway knows the game`, !!g, `game ${gameId}`)) return null;

  // Per turn, from the second turn on (the cold entry is turn 1).
  const later = (r.bytes?.per_turn ?? []).filter((e) => e.turn >= 2);
  const downKb = mean(later.map((e) => e.down_body)) / KB;
  const upKb = mean(later.map((e) => e.up_body)) / KB;

  // The cold entry.
  const keyframeB = r.bytes?.keyframes?.max_bytes;
  const basisB = r.bytes?.basis?.max_bytes;
  const coldB = r.bytes?.cold_batches?.max_bytes;
  const keyframeKb = keyframeB / KB;
  const basisKb = basisB / KB;
  const coldKb = coldB / KB;

  // The gateway's counters of the same seat.
  const tx = g.seats?.[String(r.player)]?.tx ?? {};
  const gwKeyframeB = tx.keyframe_bytes_max;
  const gwBasisB = tx.basis_bytes_max;
  const gwColdB = tx.cold_batch_bytes_max;

  const kbKey = o.kbKey;
  const keyKey = o.keyKey;
  expect(`${o.label}: down per turn <= ${ctx.th.text(kbKey, " kB")}`, downKb <= ctx.th.value(kbKey), `${downKb.toFixed(1)} kB per turn (mean of turns 2..${o.turns})`);
  // The keyframe threshold is on the bytes on the wire (decision 2026-10-22): the gateway's compressed size of the
  // seat's biggest keyframe. The bot counts the raw (decoded) bytes: printed next to it as information.
  const gwKeyframeKb = (gwKeyframeB ?? 0) / KB;
  expect(`${o.label}: the gateway sent a keyframe`, gwKeyframeB > 0, `${gwKeyframeB} B on the wire, ${keyframeB} B raw at the bot`);
  expect(`${o.label}: keyframe on the wire <= ${ctx.th.text(keyKey, " kB")}`, gwKeyframeB > 0 && gwKeyframeKb <= ctx.th.value(keyKey), `${gwKeyframeKb.toFixed(1)} kB on the wire, ${keyframeKb.toFixed(1)} kB raw`);
  expect(`${o.label}: up per turn <= ${ctx.th.text("traffic.up_kb_turn", " kB")}`, upKb <= ctx.th.value("traffic.up_kb_turn"), `${upKb.toFixed(2)} kB per turn (mean of turns 2..${o.turns})`);

  return {
    game: gameId,
    turns: o.turns,
    down_kb_turn: r1(downKb),
    up_kb_turn: r1(upKb),
    keyframe_kb: r1(keyframeKb),
    basis_kb: r1(basisKb),
    keyframe_plus_basis_kb: r1((keyframeB + basisB) / KB),
    cold_batch_kb: r1(coldKb),
    gateway_keyframe_kb: r1(gwKeyframeB / KB),
    gateway_basis_kb: r1(gwBasisB / KB),
    gateway_cold_batch_kb: r1(gwColdB / KB),
  };
}

export async function run(ctx) {
  const turns = ctx.opts.turns ?? 30;
  const numbers = {};

  ctx.pending("projection of the bytes per turn to a 120 s turn: needs an idle-rate measurement");

  const duel = await game(ctx, {
    label: "Duel",
    prefix: "trafd",
    humans: 2,
    aiSlots: [2],
    size: "WORLDSIZE_DUEL",
    turns,
    kbKey: "traffic.duel.kb_turn",
    keyKey: "traffic.duel.keyframe_kb",
  });
  if (duel) numbers.duel = duel;

  const huge = await game(ctx, {
    label: "Huge",
    prefix: "trafh",
    humans: 3,
    aiSlots: [3],
    size: "WORLDSIZE_HUGE",
    turns: Math.min(turns, 10),
    kbKey: "traffic.huge.kb_turn",
    keyKey: "traffic.huge.keyframe_kb",
  });
  if (huge) numbers.huge = huge;

  return { numbers };
}
