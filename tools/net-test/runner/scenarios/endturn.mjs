// `endturn` (N4.9, net-test 4.5): the end of a turn is seen by the others, can be taken back, and an order after one's
// own end of turn takes it back (NP-D23, NT-D18). Three humans on the test world, Simultaneous, no timer. Steps, with A
// the actor and B, C the watchers:
//   a. A sends EndTurn: B and C see TurnState.seats[A].finished == true (within 2 s).
//   b. A sends an order (UnitSkip) after its own end: Applied, and in the same batch as its `Results`, a `TurnState`
//      with finished == false for A; B and C see finished == false for A as well; the turn does not change.
//   c. A ends again; then CancelEndTurn: B and C see finished == false again.
//   d. CancelEndTurn was applied, so A is not finished: an invalid move (x, y far off the map) is Rejected(Core), and
//      finished stays false.
//   e. A, B and C end: the turn goes on (turn + 1).
//   f. All three end again; the AI phase is held open (fixture_test.ai_step_delay_ms, 2 x 1 s): A's CancelEndTurn in that
//      window is queued (the worker is busy) and, with the turn the bot names (the old one), ends Rejected(TurnChanged),
//      and the turn goes on. Finding: Rejected(TurnEnded) for a cancel after "all have ended" cannot be seen from a
//      bot: an order in the AI phase is always Queued and meets the new turn, so the answer is TurnChanged.
// The browser part (button "Waiting for N players" becomes "Next turn" in one frame, and the player panel shows
// "Thinking") is the manual acceptance of the screen; there is no browser step here.
// The real core refuses the end of a turn while a unit waits for an order: ndTurn skips the units first.
import { endTurn } from "../lib/play.mjs";

export const meta = {
  id: "endturn",
  ac: ["AC-N4.9"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  serverConfig: { fixtureTest: { ai_step_delay_ms: 1000 } },
  quick: {},
  full: {},
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const seatT = (st, p) => st.turn_state?.seats?.find((x) => x.player === p) ?? null;
const outcomeOf = (h, id) => [...h.events].reverse().find((e) => e.ev === "result" && e.id === id && e.outcome !== "Queued")?.outcome ?? null;

async function poll(read, ok, ms = 2000, step = 40) {
  const t0 = Date.now();
  let v = await read();
  while (!ok(v) && Date.now() - t0 < ms) {
    await sleep(step);
    v = await read();
  }
  return { v, ms: Date.now() - t0 };
}

export async function run(ctx) {
  const { expect } = ctx;
  const tag = String(Date.now() % 100000);
  const names = [`etA${tag}`, `etB${tag}`, `etC${tag}`];
  const bots = names.map((nick) => ctx.bots.spawn({ nick }));
  for (const h of bots) await h.connect();
  await Promise.all([
    bots[0].enter({ role: "host", humans: 3, ai_slots: [], settings: { turn_mode: "Simultaneous" } }, { lobby_timeout_ms: 120000 }),
    ...bots.slice(1).map((h) => h.enter({ role: "guest", host: names[0], slots: 3 }, { lobby_timeout_ms: 120000 })),
  ]);
  const [A, B, C] = bots;
  const pA = (await A.state()).attached.player;
  const turn0 = (await A.state()).turn;
  const unitA = (await A.state()).mine[0];
  const finishedOf = async (h, p) => seatT(await h.state(), p)?.finished ?? null;
  const numbers = {};

  // a. A ends: B and C see it.
  await endTurn(A);
  const seenB = await poll(() => finishedOf(B, pA), (v) => v === true);
  const seenC = await poll(() => finishedOf(C, pA), (v) => v === true);
  expect("a: B sees A's end of turn (finished) within 2 s", seenB.v === true, `${seenB.ms} ms`);
  expect("a: C sees A's end of turn (finished) within 2 s", seenC.v === true, `${seenC.ms} ms`);
  numbers.end_seen_ms = { B: seenB.ms, C: seenC.ms };

  // b. An order after the own end: Applied; the batch with `Results` carries a TurnState with finished == false for A.
  const mark = A.events.length;
  const idOrder = await A.cmd({ UnitSkip: { unit: unitA } });
  await A.wait({ result_id: idOrder }, 10000);
  expect("b: the order after A's own end is Applied", outcomeOf(A, idOrder) === "Applied", `${outcomeOf(A, idOrder)}`);
  const batch = A.events.slice(mark).find((e) => e.ev === "batch" && e.kinds?.includes("Results"));
  expect("b: the batch with the Results carries a TurnState", !!batch && batch.kinds.includes("TurnState"), JSON.stringify(batch?.kinds));
  expect("b: A is not finished any more (its own view)", (await finishedOf(A, pA)) === false, `${await finishedOf(A, pA)}`);
  const seenB2 = await poll(() => finishedOf(B, pA), (v) => v === false);
  const seenC2 = await poll(() => finishedOf(C, pA), (v) => v === false);
  expect("b: B sees A not finished (within 2 s)", seenB2.v === false, `${seenB2.ms} ms`);
  expect("b: C sees A not finished (within 2 s)", seenC2.v === false, `${seenC2.ms} ms`);
  expect("b: the turn did not change", (await A.state()).turn === turn0, `turn ${turn0} -> ${(await A.state()).turn}`);

  // c. A ends again, then takes it back.
  await endTurn(A);
  const ended = await poll(() => finishedOf(B, pA), (v) => v === true);
  expect("c: A ends again: B sees it", ended.v === true, `${ended.ms} ms`);
  const idCancel = await A.cmd("CancelEndTurn");
  await A.wait({ result_id: idCancel }, 10000);
  expect("c: CancelEndTurn is Applied", outcomeOf(A, idCancel) === "Applied", `${outcomeOf(A, idCancel)}`);
  const back = await poll(() => finishedOf(C, pA), (v) => v === false);
  expect("c: after CancelEndTurn C sees A not finished", back.v === false, `${back.ms} ms`);

  // d. Not finished: an invalid move is Rejected(Core); the seat stays not finished.
  const idBad = await A.cmd({ UnitMoveTo: { unit: unitA, x: 32000, y: 32000 } });
  await A.wait({ result_id: idBad }, 10000);
  const badOutcome = outcomeOf(A, idBad);
  numbers.invalid_move = badOutcome;
  expect("d: an invalid move is Rejected(Core)", badOutcome === "Rejected(Core)", `${badOutcome}`);
  expect("d: the invalid move leaves A not finished", (await finishedOf(A, pA)) === false, `${await finishedOf(A, pA)}`);

  // e. Everybody ends: the turn goes on.
  await endTurn(A);
  await endTurn(B);
  await endTurn(C);
  let advanced = true;
  try {
    await A.wait({ turn: turn0 + 1 }, 20000);
  } catch {
    advanced = false;
  }
  expect("e: all ended: the turn goes on", advanced, `turn ${turn0} -> ${(await A.state()).turn}`);

  // f. Everybody ends and the AI phase is open: the cancel comes too late. The test world holds its AI phase open for
  // this (fixture_test.ai_step_delay_ms); the real core's AI phase of a Tiny map is over in a blink, nothing to catch.
  if (ctx.opts.core === "real") {
    ctx.pending("f: the cancel in an open AI phase needs the test world's held-open AI phase (fixture only)");
  } else {
    await A.wait({ phase: "Humans" }, 20000);
    const turn1 = (await A.state()).turn;
    await endTurn(A);
    await endTurn(B);
    await endTurn(C);
    await A.wait({ phase: "Ai" }, 10000);
    const idLate = await A.cmd("CancelEndTurn");
    await A.wait({ result_id: idLate }, 10000);
    const late = outcomeOf(A, idLate);
    numbers.cancel_in_ai = late;
    const sawQueued = A.events.some((e) => e.ev === "result" && e.id === idLate && e.outcome === "Queued");
    expect("f: CancelEndTurn in the AI phase is Queued, then Rejected(TurnChanged)", sawQueued && late === "Rejected(TurnChanged)", `queued ${sawQueued}, ${late}`);
    await A.wait({ turn: turn1 + 1 }, 20000);
    expect("f: the turn went on", (await A.state()).turn === turn1 + 1, `turn ${turn1} -> ${(await A.state()).turn}`);
  }

  ctx.pending("the browser part (N4.9 with B): the button 'Waiting for N players' turns into 'Next turn' in one frame, and the player panel shows 'Thinking': manual acceptance, no browser step here");
  return { numbers };
}
