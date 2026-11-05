// Finding (N4.10): "B offers peace, A accepts in the same turn" cannot happen with seat-order moves (A moves first);
// the observed order is kept: A offers, B accepts.
// `turnmodes` (N4.7): the two turn modes with three humans on the test world, one game each.
//   Hybrid: the turns are driven by this scenario (no autoplay: a seat's window stays open until the observer has
//      looked). Before the war all three seats act at once. On turn `war_turn` A declares war on B (DeclareWar with
//      team = B's player id, the fixture's team numbering). While A and B are at war their seats are sequential
//      (SeatTurn.group >= 1): the observer samples every bot's state about every tick. Invariant: never both may_act.
//      C acts together with one of them (seen in samples). An order from B while A moves is Rejected(NotYourTurn).
//      Peace (ProposePeace, then AnswerPeace { accept: true }) is made on the first move of a turn and answered on the
//      second move of the same turn (an offer expires at the next human phase). The server moves A before B in these
//      turns, so A offers and B accepts. From the turn after the peace the seats are simultaneous again (group 0) and
//      A and B may act together in the first sample of that turn.
//   Simultaneous: three bots, every unfinished seat may act in every sample.
// The observer reads each bot's own state (`state()`), so the timeline is the samples, not every TurnState message:
// a flip between two samples is not seen; the invariant is checked on the samples only (said in the report).
export const meta = {
  id: "turnmodes",
  ac: ["AC-N4.7"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { turns: 8, war_turn: 5 },
  full: { turns: 8, war_turn: 5 },
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const outcomeOf = (h, id) => [...h.events].reverse().find((e) => e.ev === "result" && e.id === id && e.outcome !== "Queued")?.outcome ?? null;
const seatT = (st, p) => st.turn_state?.seats?.find((x) => x.player === p) ?? null;

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

async function hybridGame(ctx, tag, turns, warTurn) {
  const { expect } = ctx;
  const names = [`tmHa${tag}`, `tmHb${tag}`, `tmHc${tag}`];
  const { H: [A, B, C], players: [pA, pB, pC] } = await connectAll(ctx, names, { turn_mode: "Hybrid" });
  const bots = [A, B, C];
  const seen = { samples: 0, war: false, violations: 0, cWith: 0, nyt: null, proposed: null, answered: null, peaceTurn: null, groupsAfterPeace: null, simulAfterPeace: false, declared: null };
  let declared = false;
  const deadline = Date.now() + 120000;
  while (Date.now() < deadline) {
    const [sa, sb, sc] = await Promise.all(bots.map((h) => h.state()));
    const turn = sa.turn;
    if (turn > turns) break;
    if (sa.phase !== "Humans") {
      await sleep(30);
      continue;
    }
    const mA = !!sa.may_act;
    const mB = !!sb.may_act;
    const mC = !!sc.may_act;
    // The war, on turn warTurn, by A (all seats act at once before it).
    if (!declared && turn >= warTurn && mA) {
      declared = true;
      const id = await A.cmd({ DeclareWar: { team: pB } });
      await A.wait({ result_id: id }, 10000);
      seen.declared = outcomeOf(A, id);
      continue;
    }
    const gA = seatT(sa, pA)?.group ?? 0;
    const gB = seatT(sa, pB)?.group ?? 0;
    const war = declared && (gA >= 1 || gB >= 1);
    seen.samples++;
    if (war) {
      seen.war = true;
      if (mA && mB) seen.violations++;
      if (mC && (mA || mB)) seen.cWith++;
    }
    // An order from the seat without the right, while the other one moves.
    if (war && mA && !mB && seen.nyt === null) {
      const id = await B.cmd({ UnitSkip: { unit: sb.mine[0] } });
      await B.wait({ result_id: id }, 10000);
      seen.nyt = outcomeOf(B, id);
    }
    // Peace: an offer expires at the next human phase, so it is made on the first move of a turn and answered on the
    // second move of the same turn. The server moves A before B in the war turns (the scenario saw it), so the offer
    // goes from A to B; the spec's direction (B offers) works only when B moves first. The direction is reported.
    const finA = !!seatT(sa, pA)?.finished;
    const finB = !!seatT(sa, pB)?.finished;
    if (war && seen.proposed === null && turn >= warTurn + 1) {
      if (mA && !finB) {
        seen.proposer = "A";
        const id = await A.cmd({ ProposePeace: { to: pB } });
        await A.wait({ result_id: id }, 10000);
        seen.proposed = outcomeOf(A, id);
      } else if (mB && !finA) {
        seen.proposer = "B";
        const id = await B.cmd({ ProposePeace: { to: pA } });
        await B.wait({ result_id: id }, 10000);
        seen.proposed = outcomeOf(B, id);
      }
    }
    if (seen.proposed === "Applied" && seen.answered === null) {
      const answerer = seen.proposer === "A" ? B : A;
      const from = seen.proposer === "A" ? pA : pB;
      const may = seen.proposer === "A" ? mB : mA;
      if (may) {
        const id = await answerer.cmd({ AnswerPeace: { from, accept: true } });
        await answerer.wait({ result_id: id }, 10000);
        seen.answered = outcomeOf(answerer, id);
        seen.peaceTurn = turn;
      }
    }
    // The first sample of the turn after the peace: the seats are simultaneous and A and B may act together.
    if (seen.answered === "Applied" && seen.simulAfterPeace === false && seen.groupsAfterPeace === null && turn > seen.peaceTurn) {
      seen.groupsAfterPeace = [seatT(sa, pA)?.group ?? null, seatT(sa, pB)?.group ?? null];
      seen.simulAfterPeace = mA && mB;
    }
    // The move on: every seat that may act ends its turn.
    for (const [h, m] of [[A, mA], [B, mB], [C, mC]]) if (m) await h.cmd({ EndTurn: { force: false } });
  }
  expect("hybrid: the war was declared (DeclareWar Applied)", seen.declared === "Applied", `${seen.declared}`);
  expect("hybrid: the seats of A and B became sequential (group >= 1) at war", seen.war, `${seen.samples} samples`);
  expect("hybrid: while at war A and B never both may act", seen.war && seen.violations === 0, `${seen.violations} samples with both`);
  expect("hybrid: C acts together with one of the two at war (seen)", seen.cWith > 0, `${seen.cWith} samples`);
  expect("hybrid: an order of the seat without the right is Rejected(NotYourTurn)", seen.nyt === "Rejected(NotYourTurn)", `${seen.nyt}`);
  expect("hybrid: ProposePeace accepted by the worker", seen.proposed === "Applied", `${seen.proposed}`);
  expect("hybrid: AnswerPeace { accept: true } accepted", seen.answered === "Applied", `${seen.answered}`);
  expect("hybrid: after peace the seats are simultaneous (group 0)", JSON.stringify(seen.groupsAfterPeace) === JSON.stringify([0, 0]), JSON.stringify(seen.groupsAfterPeace));
  expect("hybrid: after peace A and B may act together", seen.simulAfterPeace, `peace on turn ${seen.peaceTurn}`);
  return seen;
}

async function simultaneousGame(ctx, tag, turns) {
  const { expect } = ctx;
  const names = [`tmSa${tag}`, `tmSb${tag}`, `tmSc${tag}`];
  const { H: bots, players } = await connectAll(ctx, names, { turn_mode: "Simultaneous" });
  let samples = 0;
  let bad = 0;
  let allThree = 0;
  const deadline = Date.now() + 90000;
  while (Date.now() < deadline) {
    const sts = await Promise.all(bots.map((h) => h.state()));
    if (sts[0].turn > turns) break;
    if (sts[0].phase !== "Humans") {
      await sleep(30);
      continue;
    }
    samples++;
    const mays = sts.map((s) => !!s.may_act);
    // Each bot is checked against its own view (the three states are read one after another, a turn may change between).
    players.forEach((p, i) => {
      const finished = seatT(sts[i], p)?.finished;
      if (!finished && !mays[i]) bad++;
    });
    if (mays.every(Boolean)) allThree++;
    for (let i = 0; i < bots.length; i++) if (mays[i]) await bots[i].cmd({ EndTurn: { force: false } });
  }
  expect("simultaneous: every unfinished seat may act in every sample", samples > 0 && bad === 0, `${samples} samples, ${bad} without the right`);
  expect("simultaneous: all three may act at once (seen)", allThree > 0, `${allThree} samples`);
  return { samples, all_three: allThree };
}

export async function run(ctx) {
  const tag = String(Date.now() % 100000);
  const turns = ctx.opts.turns ?? 8;
  const warTurn = ctx.opts.war_turn ?? 5;
  const numbers = {};
  const h = await hybridGame(ctx, tag, turns, warTurn);
  numbers.hybrid = { samples: h.samples, cWith: h.cWith, nyt: h.nyt, peace_turn: h.peaceTurn, offered_by: h.proposer };
  numbers.simultaneous = await simultaneousGame(ctx, tag, 3);
  return { numbers };
}
