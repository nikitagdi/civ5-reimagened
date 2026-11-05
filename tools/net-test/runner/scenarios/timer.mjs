// `timer` (N4.8, net-test 4.5): the turn timer on the test world, `Fixed { seconds: 10 }` unless said otherwise.
//   (6) timer Off: TurnState.timers is empty (its own game, Simultaneous, two humans).
//   (1) the time left is seen by everybody: the deadline (gateway clock) and the length in each bot's TurnState are equal.
//       The bots do not read Pong, so the clock offset is not compared: the same deadline in every view is the check.
//   (4) a pause by a seat that is not the host is GameCtlRejected NotHost (gateway counter); the host pauses 4 s into the
//       turn: `paused` is set and `left_ms_when_stopped` is set; an order during the pause is Applied; the left time does
//       not move over 3 s.
//   (3) the resume: the deadline moves with the pause. A ends its turn after the resume and B never ends it: the turn
//       changes when the time left has run out (ForceEndTurn for B; the gateway counts `timer.expired`). Checked:
//       not earlier than left - 1 s, not later than left + 1.5 s + 0.7 s of polling slack.
//   (7) Hybrid with a war: while one of the two at war acts, its timer group is in the timers and differs from the other
//       one's group (A declares war on B on turn 1 by DeclareWar with B's player id, team numbering of the fixture).
//       Not run: that the expiry of group 0 does not end the sequential seat's turn and the other way round (no expiry
//       of a group is forced here).
//   (5) the expiry is in the journal: a System line of the seats (the server's text, see the check).
// Pending: (8) the extension of the timer on war between humans needs the `Original` timer with the test row of
// `TurnTimers`: the mission can only set `Fixed` (net-bot RoomSpec.timer_seconds), so it is not reached. The
// `timer_row` key exists now; the check needs the expected extension of the rule, which is not written here.
export const meta = {
  id: "timer",
  ac: ["AC-N4.8"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { seconds: 10 },
  full: { seconds: 10 },
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const seatT = (st, p) => st.turn_state?.seats?.find((x) => x.player === p) ?? null;
const outcomeOf = (h, id) => [...h.events].reverse().find((e) => e.ev === "result" && e.id === id && e.outcome !== "Queued")?.outcome ?? null;

async function poll(read, ok, ms = 10000, step = 50) {
  const t0 = Date.now();
  let v = await read();
  while (!ok(v) && Date.now() - t0 < ms) {
    await sleep(step);
    v = await read();
  }
  return v;
}

async function connectAll(ctx, names, settings) {
  const H = names.map((nick) => ctx.bots.spawn({ nick }));
  for (const h of H) await h.connect();
  await Promise.all([
    H[0].enter({ role: "host", humans: H.length, ai_slots: [], settings }, { lobby_timeout_ms: 120000 }),
    ...H.slice(1).map((h) => h.enter({ role: "guest", host: names[0], slots: H.length }, { lobby_timeout_ms: 120000 })),
  ]);
  const players = [];
  for (const h of H) players.push((await h.state()).attached.player);
  return { H, players };
}

async function offGame(ctx, tag) {
  const { expect } = ctx;
  const { H: [A] } = await connectAll(ctx, [`tiOa${tag}`, `tiOb${tag}`], { turn_mode: "Simultaneous" });
  const st = await poll(() => A.state(), (s) => s.phase === "Humans", 10000);
  expect("(6) timer Off: TurnState.timers is empty", Array.isArray(st.turn_state?.timers) && st.turn_state.timers.length === 0, JSON.stringify(st.turn_state?.timers));
}

async function fixedGame(ctx, tag, seconds) {
  const { expect, server } = ctx;
  const names = [`tiFa${tag}`, `tiFb${tag}`];
  const { H: [A, B], players: [pA, pB] } = await connectAll(ctx, names, { turn_mode: "Simultaneous", timer_seconds: seconds });
  const gw = () => server.admin.game(gameId);
  const gameId = (await A.state()).attached.game;
  const len = seconds * 1000;
  const timerOf = async (h) => (await h.state()).turn_state?.timers?.[0] ?? null;

  const t1 = await poll(() => timerOf(A), (t) => !!t, 10000);
  const t1b = await poll(() => timerOf(B), (t) => !!t, 10000);
  expect("(1) the timer runs: TurnState.timers has its group", !!t1 && !!t1b, JSON.stringify(t1));
  expect("(1) the same deadline in every bot's view", !!t1 && !!t1b && t1.deadline_ms === t1b.deadline_ms, `${t1?.deadline_ms} vs ${t1b?.deadline_ms}`);
  expect(`(1) the length is ${seconds} s`, t1?.len_ms === len, `${t1?.len_ms}`);
  const turn0 = (await A.state()).turn;
  const t0 = Date.now();

  // (4) a pause by a seat that is not the host is refused; the host pauses 4 s into the turn.
  await sleep(Math.max(0, 4000 - (Date.now() - t0)));
  const nh0 = (await gw()).gamectl_rejected.NotHost;
  await B.gamectl({ Pause: true });
  await sleep(300);
  expect("(4) a pause by a seat that is not the host is GameCtlRejected NotHost", (await gw()).gamectl_rejected.NotHost - nh0 === 1, `${nh0} -> ${(await gw()).gamectl_rejected.NotHost}`);
  await A.gamectl({ Pause: true });
  const paused = await poll(async () => (await B.state()).turn_state?.paused, (v) => v === true, 3000);
  expect("(4) the host's pause is seen by B: paused", paused === true, `${paused}`);
  const leftAtPause = await poll(async () => (await B.state()).turn_state?.timers?.[0]?.left_ms_when_stopped ?? null, (v) => v !== null, 3000);
  expect("(4) left_ms_when_stopped is set while paused", leftAtPause !== null, `${leftAtPause}`);
  const oid = await A.cmd({ UnitSkip: { unit: (await A.state()).mine[0] } });
  await A.wait({ result_id: oid }, 10000);
  expect("(4) an order during the pause is applied", outcomeOf(A, oid) === "Applied", `${outcomeOf(A, oid)}`);
  await sleep(3000);
  const leftLater = (await B.state()).turn_state?.timers?.[0]?.left_ms_when_stopped ?? null;
  expect("(4) the left time did not move over 3 s of pause", leftLater === leftAtPause, `${leftAtPause} -> ${leftLater}`);

  // (3) the resume: A ends its turn, B does not; the turn changes when the left time has run out.
  const tr = Date.now();
  await A.gamectl({ Pause: false });
  await A.cmd({ EndTurn: { force: false } });
  let advanced = true;
  try {
    await A.wait({ turn: turn0 + 1 }, (leftAtPause ?? len) + 15000);
  } catch {
    advanced = false;
  }
  const delta = advanced ? Date.now() - tr : Infinity;
  const left = leftAtPause ?? 0;
  expect("(3) the turn changed when the time left ran out (not before, not later than +1.5 s)", delta >= left - 1000 && delta <= left + 2200, `${delta} ms after the resume, left ${left} ms`);
  const expired = (await gw()).timer.expired;
  expect("(3) gateway timer.expired counted the expiry", expired >= 1, `${expired}`);
  // The expiry line of the seats: a System line, the server's text (actor.rs: "The turn timer ran out; ...");
  // the spec writes it in Russian, so both wordings are accepted.
  const isExpiry = (e) => e.kind === "System" && (e.text.includes("turn timer ran out") || e.text.includes("\u0412\u0440\u0435\u043c\u044f \u0445\u043e\u0434\u0430 \u0432\u044b\u0448\u043b\u043e"));
  const jExp = await poll(async () => (await A.state()).journal ?? [], (j) => j.some(isExpiry), 5000);
  const expLine = jExp.find(isExpiry);
  expect("(5) the expiry is in the journal of the seats (System line)", !!expLine, `${JSON.stringify(jExp.slice(-3))}`);
  return { deadline_seen_by_all: t1?.deadline_ms ?? null, left_ms_at_pause: leftAtPause, turn_after_expiry_ms: delta };
}

async function hybridGame(ctx, tag) {
  const { expect } = ctx;
  const names = [`tiHa${tag}`, `tiHb${tag}`, `tiHc${tag}`];
  const { H: [A, B, C], players: [pA, pB] } = await connectAll(ctx, names, { turn_mode: "Hybrid", timer_seconds: 10 });
  await poll(() => A.state(), (s) => s.phase === "Humans" && (s.turn_state?.timers?.length ?? 0) > 0, 10000);
  const id = await A.cmd({ DeclareWar: { team: pB } });
  await A.wait({ result_id: id }, 10000);
  expect("(7) the war is declared on turn 1", outcomeOf(A, id) === "Applied", `${outcomeOf(A, id)}`);
  let sawA = false;
  let sawB = false;
  let differ = false;
  const t0 = Date.now();
  while (Date.now() - t0 < 2500) {
    const [sa, sb] = await Promise.all([A.state(), B.state()]);
    const gA = seatT(sa, pA)?.group ?? 0;
    const gB = seatT(sa, pB)?.group ?? 0;
    const groups = (sa.turn_state?.timers ?? []).map((t) => t.group);
    if (sa.may_act && gA >= 1 && groups.includes(gA)) sawA = true;
    if (sb.may_act && gB >= 1 && groups.includes(gB)) sawB = true;
    if (gA >= 1 && gB >= 1 && gA !== gB) differ = true;
    await sleep(40);
  }
  expect("(7) the acting sequential seat A has its own timer group", sawA, "no sample with A acting and its group timed");
  expect("(7) the sequential seats of A and B have different timer groups", differ, "");
  return { group_checked: sawA, b_group_checked: sawB };
}

export async function run(ctx) {
  const tag = String(Date.now() % 100000);
  const seconds = ctx.opts.seconds ?? 10;
  const numbers = {};
  await offGame(ctx, tag);
  numbers.fixed = await fixedGame(ctx, tag, seconds);
  numbers.hybrid = await hybridGame(ctx, tag);
  ctx.pending("(8) the extension of the timer on war between humans (Original with the TurnTimers test row): the mission can only set Fixed (RoomSpec.timer_seconds); the row is not reached");
  return { numbers };
}
