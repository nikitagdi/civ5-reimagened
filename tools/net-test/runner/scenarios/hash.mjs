// `hash` (N1.2, net-test 4.5): 4 bots, simultaneous mode, `burst`, 200 turns, 0 hash mismatches. At a quarter, half and
// three quarters of the game (turns 50, 100, 150) bot 2 gets a corruption, by three ways in turn:
//   (a) `corrupt_view`: the bot spoils its own view and recomputes the hash;
//   (b) the gateway hook `corrupt_hash` for its seat (a bad diff reaches the client);
//   (c) the gateway hook `drop_batch` (a `seq` is skipped: the client reconnects with resume).
// Bot 2 must ask for a keyframe after (a) and (b) and heal within the measured RTT + 60 ms (a channel of 100 ms in
// the full preset); after (c) it reconnects. Bots 1, 3, 4 see nothing: no keyframe requests, no mismatch, no extra
// keyframes, `seq` in a row; every view equals the gateway's. The control (NT-D10): the same `corrupt_view` run
// must be noticed by the bot (a blind bot is a failure of the stand).
import path from "node:path";
import { hex } from "../lib/json.mjs";
import { hist } from "../lib/report.mjs";
import { budgetFor, finishAll, gatewayChecks, gatewayGame, nicksOf, startGame, sum } from "../lib/play.mjs";

export const meta = {
  id: "hash",
  ac: ["AC-N1.2"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  hashMismatchAllowed: true,
  // A skipped `seq` is the corruption (c), the bot says so in its errors.
  allowErrors: [/a batch missing or repeated/],
  quick: { turns: 40, bots: 4 },
  full: { turns: 200, bots: 4 },
};

async function timeline(ctx, nick) {
  const text = await ctx.host.readFile(path.join(ctx.logDir, "bots", nick, "timeline.jsonl"));
  return text.split("\n").filter(Boolean).map((l) => JSON.parse(l));
}

/** Milliseconds from every `need_keyframe` of the bot to the batch with the `Keyframe` that answered it. */
function healTimes(events) {
  const out = [];
  events.forEach((e, i) => {
    if (e.ev === "error" && e.what === "need_keyframe") {
      const answer = events.slice(i + 1).find((x) => x.ev === "batch" && x.kinds?.includes("Keyframe"));
      out.push(answer ? answer.t_ms - e.t_ms : null);
    }
  });
  return out;
}

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns ?? 200;
  const n = opts.bots ?? 4;
  const names = nicksOf("hashbot", n);
  const [t1, t2, t3] = [Math.round(turns / 4), Math.round(turns / 2), Math.round((turns * 3) / 4)];

  // The full preset puts a channel of 100 ms between the bots and the server.
  let url;
  let channelRtt = 0;
  if (opts.preset === "full") {
    if (ctx.caps.proxy) {
      const link = await ctx.proxyLink();
      await link.set({ rtt_ms: 100 });
      url = link.wsUrl;
      channelRtt = 100;
    } else ctx.pending("channel 100 ms of the full preset: no netproxy built");
  }

  // A pause before each order slows the game to a few turns per 100 ms, so the runner's hooks land on their turns.
  const knobs = names.map((_, i) => (i === 1 ? { hold_ms: 20, corrupt_view: t1 } : { hold_ms: 20 }));
  const running = startGame(ctx, names, { turns, humans: n, settings: { turn_mode: "Simultaneous" }, knobs, url, turn_timeout_ms: 120000 });

  // The runner's part: the gateway hooks at turns t2 and t3 for the seat of bot 2.
  const fired = [];
  let watching = true;
  const watcher = (async () => {
    const todo = [
      { turn: t2, kind: "corrupt_hash" },
      { turn: t3, kind: "drop_batch" },
    ];
    while (watching && todo.length) {
      await ctx.sleep(15);
      const games = await server.admin.games().catch(() => null);
      const list = Array.isArray(games) ? games : Object.values(games?.games ?? games ?? {});
      const id = list[0]?.id ?? list[0]?.game ?? 1;
      const g = await server.admin.game(id).catch(() => null);
      const seat = g && Object.entries(g.seats ?? {}).find(([, s]) => s.nick === names[1]);
      if (!g || !seat || g.status !== "Running" || g.turn < todo[0].turn) continue;
      const f = todo.shift();
      const r = await server.admin.fault(id, Number(seat[0]), f.kind);
      fired.push({ ...f, turn_now: g.turn, status: r.status });
    }
  })();

  const finished = await finishAll(running, budgetFor(turns, 15000));
  watching = false;
  await watcher;
  if (!expect("all bots finished in time", finished !== null, `${budgetFor(turns, 15000) / 1000} s budget`)) return { numbers: { fired } };
  expect("the hooks fired at the turns asked (within 3 turns)", fired.length === 2 && fired.every((f) => f.status === 200 && f.turn_now <= f.turn + 3), JSON.stringify(fired));

  const results = finished.map((f) => f.result);
  finished.forEach((f, i) => {
    // Bot 2 notes the skipped seq among its errors on purpose and exits with 1 for it.
    const only = (results[i].errors ?? []).every((e) => meta.allowErrors.some((re) => re.test(e)));
    expect(`bot ${names[i]} exited cleanly`, f.code === 0 || (i === 1 && f.code === 1 && only), `exit ${f.code}`);
    expect(`bot ${names[i]} played ${turns} turns`, results[i].play?.done === "turns" && results[i].turns_played === turns, `done ${results[i].play?.done}, turns_played ${results[i].turns_played}`);
    expect(`bot ${names[i]} found no fog violation`, results[i].fog.violations === 0, `${results[i].fog.violations}`);
  });

  // Bots 1, 3, 4 saw nothing.
  const calm = results.filter((_, i) => i !== 1);
  calm.forEach((r) => {
    expect(`bot ${r.nick}: 0 hash mismatches of ${r.hash.checked}`, r.hash.checked > 0 && r.hash.mismatch === 0, `${r.hash.mismatch}`);
    expect(`bot ${r.nick}: asked for no keyframe`, sum(r.need_keyframe) === 0, JSON.stringify(r.need_keyframe));
    expect(`bot ${r.nick}: seq in a row, no errors`, (r.errors ?? []).length === 0, (r.errors ?? []).slice(0, 2).join("; "));
  });
  const kf = calm.map((r) => r.bytes.keyframes.n);
  expect("bots 1, 3, 4: no extra keyframe (the same count each)", new Set(kf).size === 1, kf.join(", "));

  // Bot 2 felt all three corruptions.
  const b = results[1];
  const asked = sum(b.need_keyframe);
  expect("bot 2 noticed the spoiled view and the bad diff (keyframe requests)", asked >= 2 && asked <= 6, `${asked}: ${JSON.stringify(b.need_keyframe)}`);
  expect("bot 2 saw hash mismatches", b.hash.mismatch >= 2, `${b.hash.mismatch}`);
  expect("bot 2 reconnected after the skipped seq", (b.reconnects ?? []).length >= 1, `${(b.reconnects ?? []).length} reconnects`);
  expect("bot 2 ended with no ledger trouble", b.ledger.double_outcome === 0, `${b.ledger.double_outcome}`);
  const heals = healTimes(await timeline(ctx, names[1]));
  const rtt = Math.max(b.rtt_median ?? 0, channelRtt);
  const bound = rtt + ctx.th.value("hash.heal_over_rtt_ms");
  expect(`bot 2 healed every request within RTT ${rtt.toFixed(0)} + ${ctx.th.value("hash.heal_over_rtt_ms")} ms`, heals.length >= 2 && heals.every((h) => h !== null && h <= bound), `${JSON.stringify(heals)} ms, bound ${bound.toFixed(0)} ms (${ctx.th.text("hash.heal_over_rtt_ms", " ms")})`);

  // The gateway agrees with every copy at the end.
  const game = results[0].play.game;
  const g = await gatewayGame(ctx, game, results);
  if (!expect("the gateway knows the game", !!g, `game ${game}`)) return { numbers: { fired } };
  gatewayChecks(expect, g, results, names, { dirty: true });
  const gwAsked = sum(g.views.need_keyframe);
  expect("gateway: the keyframe requests are exactly those of bot 2", gwAsked === asked, `gateway ${gwAsked}, bot 2 ${asked}`);
  expect("gateway: no worker hash failure", g.views.hash_fail_worker === 0, `${g.views.hash_fail_worker}`);

  // The control: the corruption has to be noticed, or the bot is blind.
  expect("control: a spoiled view is noticed (the bot's mismatches are not zero)", b.hash.mismatch > 0 && asked > 0, `mismatch ${b.hash.mismatch}, requests ${asked}`);

  return {
    numbers: {
      game,
      turns,
      corrupted_at: { corrupt_view: t1, corrupt_hash: t2, drop_batch: t3 },
      fired,
      heal_ms: hist(heals.filter((h) => h !== null)),
      heal_each_ms: heals,
      bot2: { need_keyframe: b.need_keyframe, mismatch: b.hash.mismatch, reconnects: b.reconnects, view_hash: hex(b.final.view_hash) },
      gateway_views: g.views,
    },
  };
}
