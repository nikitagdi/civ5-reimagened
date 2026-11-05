// `chat-peace` (N4.10, net-test 4.5): chat between humans and the peace between them, two humans on the test world, Hybrid.
//   Chat: A sends a line to everybody, B sends a personal line to A, A sends a 600-character line (cut to 500 by the
//      gateway in the spec), A sends a burst of 60 lines at once. Gateway counters: `chat.lines` grows by the lines sent
//      (at least 3 for the first three), `chat.dropped_rate` grows on the burst (the lines over the rate are dropped and
//      journaled in the spec).
//   War: A declares war on B (DeclareWar, team = B's player id: the fixture's numbering).
//   Peace, three rounds on the same war. In Hybrid the two seats move one after the other; an offer expires at the
//      next human phase, so it is made on the first move of a turn and answered on the second move of the same turn.
//      The server moves A before B in these turns, so the offers go from A to B (the scenario looks at who moves first).
//      Round 1, decline: ProposePeace is Applied; the answer `accept: false` is Applied; the answer `accept: true` after
//      it is Rejected(Invalid): the offer was removed. Round 2, expiry: the offer is not answered in its turn; in the next
//      turn the answer is Rejected(Invalid) (the offer expired). Round 3, accept: AnswerPeace { accept: true } is Applied;
//      on the first sample of the next turn A and B are simultaneous again (group 0) and both may act.
//   Receipt: a 3-human Simultaneous room first: A's line to All reaches B and C; B's personal line to A reaches A and B
//      and not C (third party). In the chat room: the 600-character line arrives cut to 500 characters.
//   Journal: the burst is refused with the System line "You are sending chat lines too fast" for A; each peace answer
//      writes lines: a decline is Diplomacy (both seats); an expiry is System ("expired", both seats).
//   Peace offers: the view of both seats lists the open offer after ProposePeace and no offer after the answer.
// What is checked is also the worker's answer to each order (`Applied` or `Rejected`).
export const meta = {
  id: "chat-peace",
  ac: ["AC-N4.10"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { burst: 60 },
  full: { burst: 60 },
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const seatT = (st, p) => st.turn_state?.seats?.find((x) => x.player === p) ?? null;
const outcomeOf = (h, id) => [...h.events].reverse().find((e) => e.ev === "result" && e.id === id && e.outcome !== "Queued")?.outcome ?? null;

/** Reads `read()` every 100 ms until `ok(value)` holds or `ms` is over; returns the last value. */
async function poll(read, ok, ms = 8000) {
  const t0 = Date.now();
  let v = await read();
  while (!ok(v) && Date.now() - t0 < ms) {
    await sleep(100);
    v = await read();
  }
  return v;
}
const chatOf = async (h) => (await h.state()).chat ?? [];
const offersOf = async (h) => (await h.state()).peace_offers ?? [];
const journalOf = async (h) => (await h.state()).journal ?? [];
const hasLine = (lines, text, fromPlayer, to) =>
  lines.some((l) => l.text === text && l.from_player === fromPlayer && (to === "All" ? l.to === "All" : l.to?.Player === to));
/** A journal line of `kind` that contains `sub`. */
const hasJournal = (entries, kind, sub) => entries.some((e) => e.kind === kind && e.text.includes(sub));

/** A 3-human room (Simultaneous): a line to All reaches C; a personal line from B to A does not reach C. */
async function thirdParty(ctx, tag) {
  const { expect } = ctx;
  const names = [`cpXa${tag}`, `cpXb${tag}`, `cpXc${tag}`];
  const [A, B, C] = names.map((nick) => ctx.bots.spawn({ nick }));
  for (const h of [A, B, C]) await h.connect();
  await Promise.all([
    A.enter({ role: "host", humans: 3, ai_slots: [], settings: { turn_mode: "Simultaneous" } }, { lobby_timeout_ms: 120000 }),
    B.enter({ role: "guest", host: names[0], slots: 3 }, { lobby_timeout_ms: 120000 }),
    C.enter({ role: "guest", host: names[0], slots: 3 }, { lobby_timeout_ms: 120000 }),
  ]);
  const pA = (await A.state()).attached.player;
  const pB = (await B.state()).attached.player;
  await A.chat("All", "all3 hello");
  await B.chat({ Player: pA }, "pers3 to A");
  await poll(
    async () => [await chatOf(B), await chatOf(C), await chatOf(A)],
    ([b, c, a]) => hasLine(b, "all3 hello", pA, "All") && hasLine(c, "all3 hello", pA, "All") && hasLine(a, "pers3 to A", pB, pA),
    8000,
  );
  const [b, c, a] = [await chatOf(B), await chatOf(C), await chatOf(A)];
  expect("third party: C got A's line to All", hasLine(c, "all3 hello", pA, "All"), `${c.length} lines`);
  expect("third party: B got A's line to All", hasLine(b, "all3 hello", pA, "All"), `${b.length} lines`);
  expect("personal: A got B's line addressed to A", hasLine(a, "pers3 to A", pB, pA), `${a.length} lines`);
  expect("personal: B has its own line too", hasLine(b, "pers3 to A", pB, pA), `${b.length} lines`);
  expect("personal: C did not get B's line to A", !c.some((l) => l.text === "pers3 to A"), `${c.length} lines`);
}

export async function run(ctx) {
  const { expect, server, opts } = ctx;
  const tag = String(Date.now() % 100000);
  await thirdParty(ctx, tag);
  const names = [`cpA${tag}`, `cpB${tag}`];
  const [A, B] = names.map((nick) => ctx.bots.spawn({ nick }));
  await A.connect();
  await B.connect();
  await Promise.all([
    A.enter({ role: "host", humans: 2, ai_slots: [], settings: { turn_mode: "Hybrid" } }, { lobby_timeout_ms: 120000 }),
    B.enter({ role: "guest", host: names[0], slots: 2 }, { lobby_timeout_ms: 120000 }),
  ]);
  const game = (await A.state()).attached.game;
  const pA = (await A.state()).attached.player;
  const pB = (await B.state()).attached.player;
  const gw = () => server.admin.game(game);
  const numbers = {};

  // ---- chat
  const c0 = (await gw()).chat;
  await A.chat("All", "hello all");
  await B.chat({ Player: pA }, "hi A");
  await A.chat("All", "x".repeat(600));
  await sleep(300);
  const c1 = (await gw()).chat;
  expect("chat: the gateway counted the lines (at least 3)", c1.lines - c0.lines >= 3, `${c0.lines} -> ${c1.lines}`);
  for (let i = 0; i < (opts.burst ?? 60); i++) await A.chat("All", `burst ${i}`);
  await sleep(500);
  const c2 = (await gw()).chat;
  numbers.chat = { lines: c2.lines - c0.lines, dropped_rate: c2.dropped_rate - c0.dropped_rate };
  expect("chat: the burst over the rate is dropped and counted (dropped_rate grew)", c2.dropped_rate - c0.dropped_rate > 0, `${c0.dropped_rate} -> ${c2.dropped_rate}`);

  // Receipt: B has A's line to All and the 600-character line cut to 500; A and B have the personal line.
  const [chA, chB] = await poll(
    async () => [await chatOf(A), await chatOf(B)],
    ([a, b]) => hasLine(b, "hello all", pA, "All") && hasLine(a, "hi A", pB, pA) && b.some((l) => l.from_player === pA && /^x+$/.test(l.text)),
    8000,
  );
  expect("receipt: B got A's line to All", hasLine(chB, "hello all", pA, "All"), `${chB.length} lines`);
  expect("receipt: A got B's personal line", hasLine(chA, "hi A", pB, pA), `${chA.length} lines`);
  expect("receipt: B got the 600-character line cut to 500 characters", chB.some((l) => l.from_player === pA && /^x+$/.test(l.text) && l.text.length === 500), `${chB.filter((l) => l.from_player === pA && /^x+$/.test(l.text)).map((l) => l.text.length).join(",")}`);
  // Journal: the burst is refused for A with a System line.
  const jBurst = await poll(() => journalOf(A), (j) => j.some((e) => e.kind === "System" && e.text.includes("chat lines too fast")), 5000);
  expect("journal: A has the System line 'chat lines too fast' after the burst", jBurst.some((e) => e.kind === "System" && e.text.includes("chat lines too fast")), `${jBurst.length} entries`);

  // ---- the war, then the rounds (driven turn by turn: a seat that may act and has nothing to do ends its turn)
  const rolesOf = (sa, sb) => {
    const finA = !!seatT(sa, pA)?.finished;
    const finB = !!seatT(sa, pB)?.finished;
    if (sa.may_act && !finB) return { P: A, Q: B, P_p: pA, Q_p: pB, P_st: "A", Q_st: "B" };
    if (sb.may_act && !finA) return { P: B, Q: A, P_p: pB, Q_p: pA, P_st: "B", Q_st: "A" };
    return null;
  };
  /** Ticks until `pred(sa, sb)` holds: seats that may act end their turn meanwhile. */
  async function driveTo(pred, ms = 30000) {
    const t0 = Date.now();
    for (;;) {
      const [sa, sb] = await Promise.all([A.state(), B.state()]);
      if (pred(sa, sb)) return [sa, sb];
      if (Date.now() - t0 > ms) throw new Error("driveTo: the condition was not met in 30 s");
      if (sa.may_act) await A.cmd({ EndTurn: { force: false } });
      if (sb.may_act) await B.cmd({ EndTurn: { force: false } });
      await sleep(30);
    }
  }

  const idW = await A.cmd({ DeclareWar: { team: pB } });
  await A.wait({ result_id: idW }, 10000);
  expect("war: A declared war on B (DeclareWar Applied)", outcomeOf(A, idW) === "Applied", `${outcomeOf(A, idW)}`);

  const round = async (kind, minTurn) => {
    const [sa0, sb0] = await driveTo((sa, sb) => sa.turn >= minTurn && rolesOf(sa, sb) !== null);
    const r = rolesOf(sa0, sb0);
    const turnT = sa0.turn;
    const idP = await r.P.cmd({ ProposePeace: { to: r.Q_p } });
    await r.P.wait({ result_id: idP }, 10000);
    const proposed = outcomeOf(r.P, idP);
    const out = { proposer: r.P_st, turn: turnT, proposed };
    const hasOffer = (list) => list.some((o) => o.from === r.P_p && o.to === r.Q_p);
    const bothViews = async () => [await offersOf(A), await offersOf(B)];
    if (proposed === "Applied") {
      const [oa, ob] = await poll(bothViews, ([a, b]) => hasOffer(a) && hasOffer(b), 5000);
      out.offer_in_views = hasOffer(oa) && hasOffer(ob);
    }
    if (kind === "expire") {
      // The offer is not answered in its turn; the next turn is the one of the answer.
      await driveTo((sa, sb) => sa.turn > turnT && (r.Q === A ? sa.may_act : sb.may_act));
    } else {
      await driveTo((sa, sb) => sa.turn === turnT && (r.Q === A ? sa.may_act : sb.may_act));
    }
    if (kind === "decline") {
      const idD = await r.Q.cmd({ AnswerPeace: { from: r.P_p, accept: false } });
      await r.Q.wait({ result_id: idD }, 10000);
      out.decline = outcomeOf(r.Q, idD);
      const idX = await r.Q.cmd({ AnswerPeace: { from: r.P_p, accept: true } });
      await r.Q.wait({ result_id: idX }, 10000);
      out.after_decline = outcomeOf(r.Q, idX);
    } else if (kind === "expire") {
      const idE = await r.Q.cmd({ AnswerPeace: { from: r.P_p, accept: true } });
      await r.Q.wait({ result_id: idE }, 10000);
      out.answer_next_turn = outcomeOf(r.Q, idE);
    } else {
      const idY = await r.Q.cmd({ AnswerPeace: { from: r.P_p, accept: true } });
      await r.Q.wait({ result_id: idY }, 10000);
      out.accept = outcomeOf(r.Q, idY);
      out.turn_accepted = sa0.turn;
    }
    // The offer is gone from both views once it is answered or has expired.
    const [ga, gb] = await poll(bothViews, ([a, b]) => !hasOffer(a) && !hasOffer(b), 5000);
    out.offer_gone_in_views = !hasOffer(ga) && !hasOffer(gb);
    // Journal lines: decline (proposer and answerer), expiry (both seats).
    if (kind !== "accept") {
      // Decline lines are Diplomacy; the expiry line is a System line (peace.rs: system_entry).
      const jKind = kind === "decline" ? "Diplomacy" : "System";
      const [wantP, wantQ] = kind === "decline" ? ["declined your peace offer", "You declined the peace offer"] : ["expired", "expired"];
      const [jp, jq] = await poll(
        async () => [await journalOf(r.P), await journalOf(r.Q)],
        ([p, q]) => hasJournal(p, jKind, wantP) && hasJournal(q, jKind, wantQ),
        5000,
      );
      out.journal = { kind: jKind, wantP, wantQ, P: hasJournal(jp, jKind, wantP), Q: hasJournal(jq, jKind, wantQ) };
    }
    return out;
  };

  const d = await round("decline", 1);
  expect("decline: ProposePeace Applied", d.proposed === "Applied", `${d.proposed}`);
  expect("decline: both views list the open offer after ProposePeace", d.offer_in_views === true, `${d.offer_in_views}`);
  expect("decline: no offer in either view after the answer", d.offer_gone_in_views === true, `${d.offer_gone_in_views}`);
  expect("decline: the Diplomacy journal has the decline (proposer and answerer)",d.journal?.P === true && d.journal?.Q === true, JSON.stringify(d.journal));
  expect("decline: AnswerPeace { accept: false } Applied", d.decline === "Applied", `${d.decline}`);
  expect("decline: the offer is gone (accept after decline: Rejected(Invalid))", d.after_decline === "Rejected(Invalid)", `${d.after_decline}`);

  const e = await round("expire", d.turn + 1);
  expect("expire: ProposePeace Applied", e.proposed === "Applied", `${e.proposed}`);
  expect("expire: the answer in the next turn is Rejected(Invalid): the offer expired", e.answer_next_turn === "Rejected(Invalid)", `${e.answer_next_turn}`);
  expect("expire: no offer in either view in the next turn", e.offer_gone_in_views === true, `${e.offer_gone_in_views}`);
  expect("expire: the journal has the System line 'expired' (both seats)", e.journal?.P === true && e.journal?.Q === true, JSON.stringify(e.journal));

  const y = await round("accept", e.turn + 1);
  expect("accept: ProposePeace Applied", y.proposed === "Applied", `${y.proposed}`);
  expect("accept: AnswerPeace { accept: true } Applied (the war is over)", y.accept === "Applied", `${y.accept}`);
  expect("accept: no offer in either view after the answer", y.offer_gone_in_views === true, `${y.offer_gone_in_views}`);
  // The next turn: simultaneous seats, both may act at its first sample.
  const [na, nb] = await driveTo((sa) => sa.turn > y.turn_accepted && sa.phase === "Humans");
  const simul = seatT(na, pA)?.group === 0 && seatT(na, pB)?.group === 0 && !!na.may_act && !!nb.may_act;
  expect("accept: on the next turn A and B are simultaneous (group 0) and may act together", simul, `groups ${seatT(na, pA)?.group}, ${seatT(na, pB)?.group}; may ${na.may_act}, ${nb.may_act}`);
  numbers.peace = { decline: d, expire: e, accept: y };

  return { numbers };
}
