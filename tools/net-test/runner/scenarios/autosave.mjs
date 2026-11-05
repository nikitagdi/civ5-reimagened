// `autosave` (N5.3, game-host-mp 4.11, net-test 4.5): when the worker saves and where the files go. One human bot and
// one AI.
//   1. Turn-start saves: the bot plays `turns` turns; for every one of the last turns there is a save of its start,
//      the `seq` numbers rise by one, the ring holds at most 10 files, the `keep/` directory holds the start of every
//      10th turn (and only those) and is never trimmed.
//   2. The timed save: the server runs with `[worker.autosave] min_interval_s = <interval>`. In a human phase with no
//      order given, no new save appears for longer than the interval. After one applied order a new save of the
//      same turn appears within the interval + 3 s.
// (The real minimum is 60 s; the quick preset shortens it to 4 s, the full one keeps 60 s.)
import { savesOnDisk, until } from "../lib/chaos.mjs";

export const meta = {
  id: "autosave",
  ac: ["AC-N5.3"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  quick: { turns: 24, interval_s: 4, serverConfig: { autosaveMinIntervalS: 4 } },
  full: { turns: 34, interval_s: 60, serverConfig: { autosaveMinIntervalS: 60 } },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const interval = opts.interval_s;
  const H = ctx.bots.spawn({ nick: "savebot" });
  await H.connect();
  await H.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const st0 = await H.state();
  const game = st0.attached.game;
  const unit = st0.mine[0];

  // ---- 1. turn-start saves, the ring and keep
  await H.autoplay({ turns: opts.turns, persona: "autoplay", pace: "burst" });
  await H.wait({ autoplay: 1 }, 240000);
  await H.wait({ phase: "Humans" }, 60000);
  await ctx.sleep(500);
  const now = (await server.admin.game(game)).turn;
  // The save thread of the worker works behind the turns: with the real core a save is 1.5 MB that is packed and
  // `fsync`ed (seconds on a WSL mount), while a burst of turns takes less. The files of the last turns are there a
  // little later; the scenario judges the ring once the thread has caught up (the newest file is the start of `now`).
  await until(async () => savesOnDisk(server, game).some((s) => s.turn === now) && (await server.admin.game(game)).saves.ok >= now - 1, 180000, 250);
  await ctx.sleep(500);
  let saves = savesOnDisk(server, game);
  const ring = saves.filter((s) => s.kind === "Ring");
  const keep = saves.filter((s) => s.kind === "Keep");
  expect("the ring holds at most 10 saves", ring.length > 0 && ring.length <= 10, `${ring.length} files`);
  expect("the ring is full after more than 10 saves", ring.length === 10, `${ring.length} files`);
  // The start of every 10th turn goes to keep/ and not into the ring (saves.rs `start_kind`), so a turn of the last 5
  // that 10 divides is found there.
  const ringTurns = new Set(ring.map((s) => s.turn));
  const keptTurns = new Set(keep.map((s) => s.turn));
  const lastTen = Array.from({ length: 5 }, (_, i) => now - i);
  expect(`the start of each of the last 5 turns (${lastTen.join(", ")}) has a save in the ring (a 10th turn: in keep/)`, lastTen.every((t) => ringTurns.has(t) || (t % 10 === 0 && keptTurns.has(t))), `ring turns ${[...ringTurns].sort((a, b) => a - b).join(",")}`);
  const seqs = saves.map((s) => s.seq).sort((a, b) => a - b);
  expect("the seq numbers are consecutive over ring and keep together (no hole at the newest end)", seqs.slice(-ring.length).every((q, i, a) => i === 0 || q === a[i - 1] + 1), `${seqs.slice(-12).join(",")}`);
  const expectedKeep = [];
  for (let t = 10; t <= now; t += 10) expectedKeep.push(t);
  const keepTurns = [...new Set(keep.map((s) => s.turn))].sort((a, b) => a - b);
  expect(`keep/ holds the start of every 10th turn (${expectedKeep.join(", ")}) and nothing else`, keepTurns.join(",") === expectedKeep.join(","), `keep turns ${keepTurns.join(",")}`);
  const m = (await server.admin.game(game)).saves;
  expect("the gateway counted the saves and none failed", m.ok >= now - 1 && m.failed === 0, `ok ${m.ok}, failed ${m.failed}, turn ${now}`);

  // ---- 2. the timed save
  const seqNow = () => savesOnDisk(server, game)[0]?.seq ?? 0;
  const base = seqNow();
  const turnNow = (await H.state()).turn;
  await ctx.sleep((interval + 2) * 1000);
  expect(`no order, no timed save: no new save in ${interval + 2} s of a human phase`, seqNow() === base, `seq ${base} -> ${seqNow()}`);
  // The unit of the start may be gone after 30 turns of autoplay (the real core has wars): take one that is there.
  const unitNow = (await H.state()).mine[0] ?? unit;
  const id = await H.cmd({ UnitSkip: { unit: unitNow } });
  await H.wait({ result_id: id }, 15000);
  const t0 = Date.now();
  const fresh = await until(() => (seqNow() > base ? savesOnDisk(server, game)[0] : null), (interval + 3) * 1000, 100);
  expect(`after an applied order a new save appears within ${interval} s + 3 s`, !!fresh, fresh ? `${((Date.now() - t0) / 1000).toFixed(1)} s` : "none");
  expect("the timed save belongs to the same turn", !!fresh && fresh.turn === turnNow, `save of turn ${fresh?.turn}, the turn is ${turnNow}`);
  return { numbers: { turn: now, ring: ring.length, keep_turns: keepTurns, saves_ok: m.ok, timed_save_s: fresh ? Math.round(Date.now() - t0) / 1000 : null } };
}
